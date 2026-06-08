//! Audit log — a bounded in-memory ring of every scan decision.
//!
//! The log is write-mostly, read-rarely: operators inspect it through
//! the control API or the studio when something looks off. We keep it
//! in memory to avoid another SQLite migration; if the runtime is
//! asked to persist audits in the future, a PersistentAuditSink trait
//! can plug in here without changing call sites.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::classifier::Phase;
use crate::policy::{FindingDecision, Verdict};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub phase: Phase,
    pub plugin: String,
    pub capability: String,
    pub verdict: Verdict,
    pub findings: Vec<FindingDecision>,
}

pub struct AuditLog {
    capacity: usize,
    events: Mutex<Vec<AuditEvent>>,
    next_id: Mutex<u64>,
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            events: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
        }
    }

    pub fn record(
        &self,
        phase: Phase,
        plugin: impl Into<String>,
        capability: impl Into<String>,
        verdict: Verdict,
        findings: Vec<FindingDecision>,
    ) -> AuditEvent {
        let mut id_slot = self.next_id.lock();
        let id = *id_slot;
        *id_slot = id_slot.wrapping_add(1);
        drop(id_slot);

        let event = AuditEvent {
            id,
            timestamp: Utc::now(),
            phase,
            plugin: plugin.into(),
            capability: capability.into(),
            verdict,
            findings,
        };
        let mut events = self.events.lock();
        if events.len() >= self.capacity {
            // Drop oldest to stay within capacity. The ring is small
            // (default 512) so the cost of the shift is negligible and
            // we never reorder ids.
            let drop_count = events.len() + 1 - self.capacity;
            events.drain(0..drop_count);
        }
        events.push(event.clone());
        event
    }

    pub fn recent(&self, limit: usize) -> Vec<AuditEvent> {
        let events = self.events.lock();
        let take = limit.min(events.len());
        events[events.len() - take..].to_vec()
    }

    pub fn len(&self) -> usize {
        self.events.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.lock().is_empty()
    }
}

pub type SharedAuditLog = Arc<AuditLog>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::{Finding, FindingLocation, Severity};

    fn dummy_finding() -> FindingDecision {
        FindingDecision {
            finding: Finding {
                rule_id: "test.rule".into(),
                severity: Severity::Warn,
                message: "noop".into(),
                match_preview: "***".into(),
                location: FindingLocation {
                    pointer: "/".into(),
                },
            },
            verdict: Verdict::Warn,
        }
    }

    #[test]
    fn ring_buffer_drops_oldest() {
        let log = AuditLog::new(3);
        for i in 0..6 {
            log.record(
                Phase::PreCall,
                format!("plugin-{i}"),
                "example.tool",
                Verdict::Warn,
                vec![dummy_finding()],
            );
        }
        let recent = log.recent(10);
        assert_eq!(recent.len(), 3);
        // The last three events should be plugin-3..=plugin-5.
        assert_eq!(recent[0].plugin, "plugin-3");
        assert_eq!(recent[2].plugin, "plugin-5");
    }
}
