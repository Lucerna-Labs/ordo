//! Correlation helpers layered on top of the pub/sub `Bus` trait.
//!
//! Respects Rule 1 (bus is pub/sub, not req/resp). The `Bus` trait
//! stays unchanged; request/response and scatter-gather are
//! *composed* from the broadcast primitive:
//!
//!   - Caller generates a `CorrelationId` and stamps it on the
//!     envelope it publishes.
//!   - Responder publishes its reply on a well-known reply topic
//!     with the same `CorrelationId`.
//!   - The correlator subscribes to the reply topic, filters by id,
//!     returns the first matching envelope (or N of them for
//!     scatter-gather) within the deadline.
//!
//! This is the same pattern every bus-first request/response system
//! in Rust uses; we consolidate it here so memory crates never
//! reinvent it.

use std::time::Duration;

use futures::StreamExt;
use ordo_protocol::{BusEnvelope, CorrelationId, NodeId, OrdoMessage};
use tokio::time::Instant;

use crate::Bus;

#[derive(Debug, thiserror::Error)]
pub enum CorrelatorError {
    #[error("timeout after {}ms waiting for reply", .0.as_millis())]
    Timeout(Duration),
    #[error("bus transport: {0}")]
    Transport(String),
    #[error("lagged â€” the broadcast channel dropped {0} messages before we could read them; reply may be lost")]
    Lagged(u64),
}

pub struct BusCorrelator<'a> {
    bus: &'a dyn Bus,
}

impl<'a> BusCorrelator<'a> {
    pub fn new(bus: &'a dyn Bus) -> Self {
        Self { bus }
    }

    /// Publish `request` on `request_topic`, then await exactly one
    /// envelope on `reply_topic` whose `correlation_id` matches
    /// `request.correlation_id`. Returns the first matching envelope
    /// or `Timeout` if none arrives in `timeout`.
    ///
    /// `request` MUST already have its `correlation_id` populated â€”
    /// the correlator does not mutate the envelope. This keeps the
    /// caller in control of id semantics (e.g., deriving the id from
    /// a saga root).
    pub async fn call(
        &self,
        request_topic: &str,
        reply_topic: &str,
        request: BusEnvelope,
        timeout: Duration,
    ) -> Result<BusEnvelope, CorrelatorError> {
        let expected = request
            .correlation_id
            .clone()
            .ok_or_else(|| CorrelatorError::Transport("request missing correlation_id".into()))?;
        // Subscribe FIRST, then publish. If we publish before
        // subscribing, the responder might reply before our
        // subscription is live and we'd wait forever.
        let mut stream = self
            .bus
            .subscribe(reply_topic)
            .await
            .map_err(|err| CorrelatorError::Transport(err.to_string()))?;
        self.bus
            .publish(request_topic, request)
            .await
            .map_err(|err| CorrelatorError::Transport(err.to_string()))?;

        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                _ = &mut deadline => return Err(CorrelatorError::Timeout(timeout)),
                next = stream.next() => {
                    match next {
                        Some(envelope) => {
                            if envelope.correlation_id.as_ref() == Some(&expected) {
                                return Ok(envelope);
                            }
                        }
                        None => return Err(CorrelatorError::Transport(
                            "reply subscription ended before timeout".into(),
                        )),
                    }
                }
            }
        }
    }

    /// Scatter/gather: publish once, collect up to `max_replies`
    /// matching envelopes on `reply_topic` within `deadline`.
    /// Returns whatever arrived â€” partial results are first-class,
    /// not an error, because providers can be slow.
    pub async fn scatter_gather(
        &self,
        request_topic: &str,
        reply_topic: &str,
        request: BusEnvelope,
        deadline: Duration,
        max_replies: usize,
    ) -> Result<Vec<BusEnvelope>, CorrelatorError> {
        if max_replies == 0 {
            return Ok(Vec::new());
        }
        let expected = request
            .correlation_id
            .clone()
            .ok_or_else(|| CorrelatorError::Transport("request missing correlation_id".into()))?;
        let mut stream = self
            .bus
            .subscribe(reply_topic)
            .await
            .map_err(|err| CorrelatorError::Transport(err.to_string()))?;
        self.bus
            .publish(request_topic, request)
            .await
            .map_err(|err| CorrelatorError::Transport(err.to_string()))?;

        let mut collected = Vec::with_capacity(max_replies);
        let start = Instant::now();
        while collected.len() < max_replies {
            let remaining = deadline.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            let sleeper = tokio::time::sleep(remaining);
            tokio::pin!(sleeper);
            tokio::select! {
                _ = &mut sleeper => break,
                next = stream.next() => {
                    match next {
                        Some(envelope) => {
                            if envelope.correlation_id.as_ref() == Some(&expected) {
                                collected.push(envelope);
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        Ok(collected)
    }
}

/// Convenience: stamp a fresh `CorrelationId` on an envelope for
/// callers that don't need to pre-generate one. Preserves `sender`
/// and `payload`; rewrites `correlation_id`.
pub fn with_fresh_correlation(mut envelope: BusEnvelope) -> (BusEnvelope, CorrelationId) {
    let id = CorrelationId::new();
    envelope.correlation_id = Some(id.clone());
    (envelope, id)
}

/// Build a reply envelope mirroring the correlation id of the
/// request â€” responders use this to ensure matches.
pub fn reply_envelope(sender: NodeId, to: &BusEnvelope, payload: OrdoMessage) -> BusEnvelope {
    let mut reply = BusEnvelope::new(sender, payload);
    reply.correlation_id = to.correlation_id.clone();
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InProcessBus;
    use ordo_protocol::NodeStatus;
    use std::sync::Arc;

    fn request_envelope() -> BusEnvelope {
        let sender = NodeId::new();
        let mut env = BusEnvelope::new(
            sender,
            OrdoMessage::Heartbeat(NodeStatus {
                id: NodeId::new(),
                name: "test".into(),
                uptime_secs: 0,
                version: "0.1".into(),
                capabilities: vec![],
            }),
        );
        env.correlation_id = Some(CorrelationId::new());
        env
    }

    #[tokio::test]
    async fn call_awaits_matching_reply() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let request = request_envelope();
        let correlation = request.correlation_id.clone().unwrap();

        // Spawn a responder that echoes heartbeat back on the reply topic
        // with the same correlation id.
        let bus_responder = bus.clone();
        tokio::spawn(async move {
            let mut sub = bus_responder.subscribe("test.request").await.unwrap();
            if let Some(received) = sub.next().await {
                let reply = reply_envelope(NodeId::new(), &received, OrdoMessage::HealthProbe);
                let _ = bus_responder.publish("test.reply", reply).await;
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let correlator = BusCorrelator::new(bus.as_ref());
        let reply = correlator
            .call(
                "test.request",
                "test.reply",
                request,
                Duration::from_secs(2),
            )
            .await
            .expect("reply");
        assert_eq!(reply.correlation_id.as_ref(), Some(&correlation));
    }

    #[tokio::test]
    async fn call_times_out_cleanly_when_no_reply() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let correlator = BusCorrelator::new(bus.as_ref());
        let err = correlator
            .call(
                "nobody.listens",
                "nobody.replies",
                request_envelope(),
                Duration::from_millis(60),
            )
            .await
            .expect_err("should time out");
        assert!(matches!(err, CorrelatorError::Timeout(_)));
    }

    #[tokio::test]
    async fn scatter_gather_collects_multiple_replies() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let request = request_envelope();
        let correlation = request.correlation_id.clone().unwrap();

        // Fan out 3 responders.
        for i in 0..3 {
            let bus_responder = bus.clone();
            let expected = correlation.clone();
            tokio::spawn(async move {
                let mut sub = bus_responder.subscribe("scatter.request").await.unwrap();
                while let Some(received) = sub.next().await {
                    if received.correlation_id.as_ref() == Some(&expected) {
                        let reply =
                            reply_envelope(NodeId::new(), &received, OrdoMessage::HealthProbe);
                        tokio::time::sleep(Duration::from_millis(10 * i)).await;
                        let _ = bus_responder.publish("scatter.reply", reply).await;
                        return;
                    }
                }
            });
        }

        tokio::time::sleep(Duration::from_millis(30)).await;
        let correlator = BusCorrelator::new(bus.as_ref());
        let replies = correlator
            .scatter_gather(
                "scatter.request",
                "scatter.reply",
                request,
                Duration::from_millis(500),
                3,
            )
            .await
            .expect("replies");
        assert_eq!(replies.len(), 3);
        for reply in replies {
            assert_eq!(reply.correlation_id.as_ref(), Some(&correlation));
        }
    }

    #[tokio::test]
    async fn scatter_gather_returns_partial_on_deadline() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let correlator = BusCorrelator::new(bus.as_ref());
        let replies = correlator
            .scatter_gather(
                "nobody.asks",
                "nobody.answers",
                request_envelope(),
                Duration::from_millis(40),
                5,
            )
            .await
            .expect("partial");
        assert!(replies.is_empty());
    }

    #[tokio::test]
    async fn call_without_correlation_id_returns_transport_error() {
        let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
        let mut env = request_envelope();
        env.correlation_id = None;
        let correlator = BusCorrelator::new(bus.as_ref());
        let err = correlator
            .call("t", "r", env, Duration::from_millis(50))
            .await
            .expect_err("should error");
        assert!(matches!(err, CorrelatorError::Transport(_)));
    }
}
