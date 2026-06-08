//! Periodic health-probe + canary-sweep task (blueprint concern 1).
//!
//! Spawned by the runtime. Every `interval` (default 60s):
//!   1. Appends a canary event via `MemoryLogService::canary_probe`.
//!   2. On success: emits `ordo.memory.log.health.ok` with the
//!      current `MemoryLogHealth` snapshot.
//!   3. On failure: emits `ordo.memory.log.health.degraded` with the
//!      failure reason and the same snapshot. This is the signal
//!      Rescue Mode subscribes to â€” once the degraded event fires,
//!      rescue can decide to quarantine writes, alert the operator,
//!      or trigger a manual remediation.
//!   4. Calls `sweep_stale_canaries` so 24h-old canary rows get
//!      soft-deleted and don't dominate the table.
//!
//! Failure emission is bus-published even if the canary itself
//! failed â€” the bus is expected to be available even when the log
//! isn't.
//!
//! The task runs until the `shutdown` future resolves or the bus
//! vanishes. Intended lifetime = runtime lifetime.

use std::sync::Arc;
use std::time::Duration;

use ordo_bus::Bus;
use ordo_protocol::{memory_topics, Envelope, NodeId, OrdoMessage};

use crate::service::MemoryLogService;

/// Default probe interval. Small enough that operator-facing
/// degraded events show up within 90s per the acceptance criterion
/// (interval + canary timeout margin); large enough that canary
/// rows grow at ~1,440 per day.
pub const DEFAULT_PROBE_INTERVAL_SECS: u64 = 60;

pub struct MemoryLogHealthTask {
    log: MemoryLogService,
    bus: Arc<dyn Bus>,
    interval: Duration,
    node_id: NodeId,
}

impl MemoryLogHealthTask {
    pub fn new(log: MemoryLogService, bus: Arc<dyn Bus>) -> Self {
        Self {
            log,
            bus,
            interval: Duration::from_secs(DEFAULT_PROBE_INTERVAL_SECS),
            node_id: NodeId::new(),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Run forever. Caller spawns this onto a tokio task via
    /// `spawn_component`; graceful shutdown is via task abort.
    pub async fn run(self) {
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // First tick fires immediately â€” give subscribers a chance
        // to wire up by skipping it.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            self.tick_once().await;
        }
    }

    /// Exposed for tests. Runs one iteration and returns. Safe to
    /// call at any cadence.
    pub async fn tick_once(&self) {
        let probe_outcome = self.log.canary_probe().await;
        let snapshot = self.log.health();
        match probe_outcome {
            Ok(_) => {
                let envelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::MemoryLogHealthOk(snapshot),
                );
                if let Err(err) = self
                    .bus
                    .publish(memory_topics::LOG_HEALTH_OK, envelope)
                    .await
                {
                    tracing::warn!(
                        target: "ordo_memory_log::health",
                        error = %err,
                        "health.ok publish failed"
                    );
                }
            }
            Err(err) => {
                let reason = err.to_string();
                tracing::warn!(
                    target: "ordo_memory_log::health",
                    reason = %reason,
                    "canary append failed â€” emitting degraded event"
                );
                let envelope = Envelope::new(
                    self.node_id.clone(),
                    OrdoMessage::MemoryLogHealthDegraded {
                        reason,
                        health: snapshot,
                    },
                );
                if let Err(err) = self
                    .bus
                    .publish(memory_topics::LOG_HEALTH_DEGRADED, envelope)
                    .await
                {
                    tracing::error!(
                        target: "ordo_memory_log::health",
                        error = %err,
                        "FAILED to publish degraded event (bus down?)"
                    );
                }
            }
        }
        // Always attempt the sweep â€” canary TTL shouldn't stall on
        // a degraded append path; if the sweep itself fails, log
        // and move on.
        if let Err(err) = self.log.sweep_stale_canaries() {
            tracing::warn!(
                target: "ordo_memory_log::health",
                error = %err,
                "canary sweep failed"
            );
        }
    }
}
