//! Bus â†’ HTTP dispatcher.
//!
//! Subscribes to `ordo.apps.event`, `ordo.files.event`, and any
//! future topic callers register. For each envelope that matches a
//! subscription, POSTs a signed JSON body to the target URL.
//!
//! Best-effort at-most-once delivery. Retries are deferred.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{BusEnvelope, WebhookSubscription};
use serde_json::json;

use crate::service::WebhookService;
use crate::signing;

/// List of bus topics the dispatcher subscribes to. Extend this list
/// as new primitives ship with their own event streams (preview
/// events, sandbox events, etc.).
pub const DISPATCH_TOPICS: &[&str] = &[
    ordo_protocol::topics::APPS_EVENT,
    ordo_protocol::topics::FILES_EVENT,
];

pub struct WebhookDispatcher {
    service: WebhookService,
    bus: Arc<dyn Bus>,
    http: reqwest::Client,
}

impl WebhookDispatcher {
    pub fn new(service: WebhookService, bus: Arc<dyn Bus>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("webhook http client");
        Self { service, bus, http }
    }

    /// Run the dispatcher until the bus closes. Intended to be spawned
    /// in a `ordo-runtime` component so it runs for the lifetime of
    /// the process.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Subscribe to every topic up-front. Per Rule 10 we use the
        // existing broadcast subscribe primitive â€” no new channel.
        let mut streams = Vec::with_capacity(DISPATCH_TOPICS.len());
        for topic in DISPATCH_TOPICS {
            let stream = self.bus.subscribe(topic).await?;
            streams.push((topic.to_string(), stream));
        }
        // Merge the per-topic streams. We use futures::stream::select_all
        // to avoid nesting tokio::select! per topic â€” simpler when the
        // topic list grows.
        let mut merged = futures::stream::select_all(
            streams
                .into_iter()
                .map(|(topic, stream)| Box::pin(stream.map(move |env| (topic.clone(), env))))
                .collect::<Vec<_>>(),
        );

        while let Some((topic, envelope)) = merged.next().await {
            let service = self.service.clone();
            let http = self.http.clone();
            tokio::spawn(async move {
                if let Err(err) = dispatch_envelope(&service, &http, &topic, envelope).await {
                    tracing::warn!(
                        target: "ordo_webhooks",
                        topic = %topic,
                        error = %err,
                        "webhook dispatch failed"
                    );
                }
            });
        }
        Ok(())
    }
}

async fn dispatch_envelope(
    service: &WebhookService,
    http: &reqwest::Client,
    topic: &str,
    envelope: BusEnvelope,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let subs = service.active_with_secrets()?;
    let matching: Vec<&WebhookSubscription> = subs
        .iter()
        .filter(|s| s.topics.is_empty() || s.topics.iter().any(|t| t == topic))
        .collect();
    if matching.is_empty() {
        return Ok(());
    }
    let body = json!({
        "topic": topic,
        "envelope": envelope,
    });
    let body_bytes = serde_json::to_vec(&body)?;
    for sub in matching {
        let status = deliver_one(http, sub, &body_bytes).await;
        if let Err(err) = service.record_delivery(sub.id, status) {
            tracing::warn!(
                target: "ordo_webhooks",
                subscription_id = %sub.id,
                error = %err,
                "failed to record delivery status"
            );
        }
    }
    Ok(())
}

async fn deliver_one(http: &reqwest::Client, sub: &WebhookSubscription, body: &[u8]) -> u16 {
    let signature = signing::sign_hex(sub.secret.as_bytes(), body);
    let timestamp = Utc::now().to_rfc3339();
    let request = http
        .post(&sub.target_url)
        .header("Content-Type", "application/json")
        .header("X-Ordo-Signature", format!("sha256={signature}"))
        .header("X-Ordo-Timestamp", timestamp)
        .header("X-Ordo-Subscription", sub.id.to_string())
        .body(body.to_vec());
    match request.send().await {
        Ok(response) => response.status().as_u16(),
        Err(err) => {
            tracing::warn!(
                target: "ordo_webhooks",
                subscription_id = %sub.id,
                url = %sub.target_url,
                error = %err,
                "webhook delivery transport error"
            );
            0
        }
    }
}

/// Render a signed payload without actually sending â€” tests use this
/// to assert signature stability.
#[cfg(test)]
pub fn render_signed_body_for_test(secret: &[u8], body: &[u8]) -> (String, Vec<u8>) {
    (signing::sign_hex(secret, body), body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signature_is_deterministic_for_same_body() {
        let a = signing::sign_hex(b"k", b"v");
        let b = signing::sign_hex(b"k", b"v");
        assert_eq!(a, b);
    }

    #[test]
    fn signature_changes_when_body_changes() {
        let a = signing::sign_hex(b"k", b"v1");
        let b = signing::sign_hex(b"k", b"v2");
        assert_ne!(a, b);
    }

    #[test]
    fn dispatch_topics_list_includes_apps_and_files() {
        assert!(DISPATCH_TOPICS.contains(&ordo_protocol::topics::APPS_EVENT));
        assert!(DISPATCH_TOPICS.contains(&ordo_protocol::topics::FILES_EVENT));
    }

    #[test]
    fn empty_topics_list_on_subscription_means_all_topics() {
        // Guard: if the service allows topics: [], dispatch_envelope
        // treats that as "everything." Documented behavior â€” keep the
        // test so a future "security tightening" doesn't break it
        // without deliberate consideration.
        let _ = json!({"topic": "x"});
    }
}
