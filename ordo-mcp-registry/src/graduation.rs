//! Trust graduation policy + anomaly ledger.
//!
//! Separated from the main registry so the thresholds live in one
//! place and can be swapped for testing or per-deployment tuning.

use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};
use ordo_protocol::ServerTrustState;
use parking_lot::Mutex;

use crate::AnomalySeverity;

#[derive(Debug, Clone)]
pub struct GraduationPolicy {
    /// Successes required + minimum time in state to advance.
    pub untrusted_to_observed: (u32, Duration),
    pub observed_to_validated: (u32, Duration),
    pub validated_to_trusted: (u32, Duration),
}

impl GraduationPolicy {
    pub fn testing() -> Self {
        Self {
            untrusted_to_observed: (2, Duration::zero()),
            observed_to_validated: (2, Duration::zero()),
            validated_to_trusted: (2, Duration::zero()),
        }
    }

    pub fn graduation_target(
        &self,
        current: ServerTrustState,
        clean_count: u32,
        installed_at: DateTime<Utc>,
    ) -> Option<ServerTrustState> {
        let elapsed = Utc::now().signed_duration_since(installed_at);
        match current {
            ServerTrustState::Untrusted
                if clean_count >= self.untrusted_to_observed.0
                    && elapsed >= self.untrusted_to_observed.1 =>
            {
                Some(ServerTrustState::Observed)
            }
            ServerTrustState::Observed
                if clean_count >= self.observed_to_validated.0
                    && elapsed >= self.observed_to_validated.1 =>
            {
                Some(ServerTrustState::Validated)
            }
            ServerTrustState::Validated
                if clean_count >= self.validated_to_trusted.0
                    && elapsed >= self.validated_to_trusted.1 =>
            {
                Some(ServerTrustState::Trusted)
            }
            _ => None,
        }
    }

    pub fn demote_one(&self, current: ServerTrustState) -> ServerTrustState {
        match current {
            ServerTrustState::Trusted => ServerTrustState::Validated,
            ServerTrustState::Validated => ServerTrustState::Observed,
            ServerTrustState::Observed => ServerTrustState::Untrusted,
            ServerTrustState::Untrusted => ServerTrustState::Quarantined,
            ServerTrustState::Quarantined => ServerTrustState::Quarantined,
        }
    }
}

impl Default for GraduationPolicy {
    fn default() -> Self {
        // Blueprint suggested values.
        Self {
            untrusted_to_observed: (20, Duration::days(7)),
            observed_to_validated: (50, Duration::days(30)),
            validated_to_trusted: (200, Duration::days(90)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnomalyRecord {
    pub server_id: String,
    pub severity: AnomalySeverity,
    pub reason: String,
    pub occurred_at: DateTime<Utc>,
}

/// Ring buffer of recent anomalies â€” exposed for telemetry +
/// audit without polluting the main registry state.
pub struct TrustLedger {
    entries: Mutex<VecDeque<AnomalyRecord>>,
    max_entries: usize,
}

impl Default for TrustLedger {
    fn default() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(256)),
            max_entries: 256,
        }
    }
}

impl TrustLedger {
    pub fn record_anomaly(
        &self,
        server_id: &str,
        severity: AnomalySeverity,
        reason: impl Into<String>,
    ) {
        let mut entries = self.entries.lock();
        if entries.len() >= self.max_entries {
            entries.pop_front();
        }
        entries.push_back(AnomalyRecord {
            server_id: server_id.to_string(),
            severity,
            reason: reason.into(),
            occurred_at: Utc::now(),
        });
    }

    pub fn recent(&self, limit: usize) -> Vec<AnomalyRecord> {
        let entries = self.entries.lock();
        entries.iter().rev().take(limit).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demote_ladder_terminates_at_quarantine() {
        let p = GraduationPolicy::default();
        assert_eq!(
            p.demote_one(ServerTrustState::Trusted),
            ServerTrustState::Validated
        );
        assert_eq!(
            p.demote_one(ServerTrustState::Validated),
            ServerTrustState::Observed
        );
        assert_eq!(
            p.demote_one(ServerTrustState::Observed),
            ServerTrustState::Untrusted
        );
        assert_eq!(
            p.demote_one(ServerTrustState::Untrusted),
            ServerTrustState::Quarantined
        );
        assert_eq!(
            p.demote_one(ServerTrustState::Quarantined),
            ServerTrustState::Quarantined
        );
    }

    #[test]
    fn graduation_requires_both_count_and_time() {
        let p = GraduationPolicy::default();
        // Count met but too new.
        let t = p.graduation_target(ServerTrustState::Untrusted, 100, Utc::now());
        assert!(t.is_none());
    }
}
