//! Pure state-derivation logic for the supervisor.
//!
//! Designed to be exercised in unit tests without a live bus or
//! tokio runtime: every input arrives via [`SystemModel::record`]
//! or the constructors, and [`SystemModel::derive`] is a pure
//! function of `(self, now, &SupervisorConfig)`.
//!
//! The state machine is deliberately small. Two axes:
//!
//! - **Health** is slow-moving and reflects whether the runtime
//!   can be trusted right now. Driven by `*.degraded` events,
//!   stale heartbeats, and self-heal urgency.
//! - **Activity** is fast-moving and reflects whether the runtime
//!   is currently doing work for the operator. Driven by run
//!   lifecycle.
//!
//! ## Self-heal mapping (V1, supervisor-internal)
//!
//! Per-incident `SelfHealUrgency` maps to system-wide `HealthState`
//! as follows:
//!
//! | Incident urgency | System health while open |
//! |------------------|--------------------------|
//! | `Critical`       | `Critical`               |
//! | `High`           | `Rescue`                 |
//! | `Medium`         | `Rescue`                 |
//! | `Low`            | (no health change)       |
//!
//! Rationale: `SelfHealUrgency::Critical` is the strongest
//! per-component signal in the protocol — components don't fire
//! it lightly. For an operator-facing readout, false negatives
//! (hiding a real Critical) are worse than the brief flicker
//! from a transient incident; the [`SupervisorConfig::incident_ttl`]
//! window absorbs short-lived blips. If the team later decides
//! Critical should require multiple concurrent signals, the
//! threshold lives here, not on the wire.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ordo_protocol::{ActivityState, HealthState, NodeId, SelfHealUrgency};
use uuid::Uuid;

/// Configuration knobs for state derivation. Lives in the
/// supervisor crate, never on the protocol wire — the protocol
/// carries state, the supervisor decides when to transition.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// How often the run loop re-evaluates state. Cheaper than
    /// per-message evaluation and gives heartbeat staleness a
    /// chance to age in. Default 1s.
    pub eval_interval: Duration,
    /// How long a recorded incident contributes to derived state
    /// before being aged out. Default 30s — long enough to keep
    /// the UI sticky on a transient blip, short enough that a
    /// genuinely-recovered subsystem returns to Healthy without
    /// operator intervention.
    pub incident_ttl: Duration,
    /// Heartbeat older than this (per node) raises health to
    /// `Rescue`. Default 5s — heartbeats fire every 2s in
    /// `ordo-mcp-host`, so 5s ≈ two missed ticks.
    pub rescue_heartbeat_threshold: Duration,
    /// Heartbeat older than this (per node) raises health to
    /// `Critical`. Default 10s — five missed ticks; the node is
    /// almost certainly dead, not just slow.
    pub critical_heartbeat_threshold: Duration,
    /// How long after the last in-flight run finishes before
    /// activity drops to `Idle`. Prevents the indicator flapping
    /// between Processing and Idle on rapid back-to-back runs.
    /// Default 3s.
    pub idle_debounce: Duration,
    /// Runs older than this without a finish event are assumed
    /// abandoned and dropped from the in-flight set. Default 600s
    /// (10 minutes) — covers normal long-running operations
    /// without the supervisor leaking memory on lost finishes.
    pub run_ttl: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            eval_interval: Duration::from_secs(1),
            incident_ttl: Duration::from_secs(30),
            rescue_heartbeat_threshold: Duration::from_secs(5),
            critical_heartbeat_threshold: Duration::from_secs(10),
            idle_debounce: Duration::from_secs(3),
            run_ttl: Duration::from_secs(600),
        }
    }
}

/// Why an incident was recorded — used both for log narration
/// and for deciding the contribution to health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncidentKind {
    /// Self-heal request observed; carries the per-incident
    /// urgency. Critical urgency raises system health to
    /// Critical; High/Medium raise it to Rescue.
    SelfHeal { urgency: SelfHealUrgency },
    /// `ordo.memory.log.health.degraded` envelope observed.
    MemoryLogDegraded,
    /// `ordo.memory.projection.replay_degraded` envelope
    /// observed.
    MemoryProjectionDegraded,
    /// `ordo.secrets.vault.seal_tier_degraded` envelope observed.
    SecretsSealDegraded,
    /// `ordo.mcp.client.auth.degraded` envelope observed.
    McpAuthDegraded,
}

impl IncidentKind {
    /// Health contribution of this incident kind. Self-heal
    /// urgency is the only kind that can directly produce
    /// `Critical`; all other degraded events produce `Rescue`
    /// (they're warnings, not failures).
    fn health_contribution(&self) -> HealthState {
        match self {
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::Critical,
            } => HealthState::Critical,
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::High | SelfHealUrgency::Medium,
            } => HealthState::Rescue,
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::Low,
            } => HealthState::Healthy,
            IncidentKind::MemoryLogDegraded
            | IncidentKind::MemoryProjectionDegraded
            | IncidentKind::SecretsSealDegraded
            | IncidentKind::McpAuthDegraded => HealthState::Rescue,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            IncidentKind::SelfHeal { .. } => "self-heal",
            IncidentKind::MemoryLogDegraded => "memory log degraded",
            IncidentKind::MemoryProjectionDegraded => "memory projection degraded",
            IncidentKind::SecretsSealDegraded => "secrets seal tier degraded",
            IncidentKind::McpAuthDegraded => "mcp auth degraded",
        }
    }
}

#[derive(Debug, Clone)]
struct Incident {
    seen_at: Instant,
    kind: IncidentKind,
}

/// In-memory model the supervisor maintains as bus envelopes
/// arrive. Pure data — no async, no I/O, no clock dependency.
/// Methods either record an input or read out derived state.
#[derive(Debug, Default)]
pub struct SystemModel {
    incidents: Vec<Incident>,
    last_heartbeat: HashMap<NodeId, Instant>,
    in_flight_runs: HashMap<Uuid, Instant>,
    /// When the last in-flight run finished. Used to debounce the
    /// transition to `Idle` so back-to-back runs don't cause
    /// flapping.
    last_run_finish: Option<Instant>,
}

impl SystemModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an incident at `now`.
    pub fn record_incident(&mut self, kind: IncidentKind, now: Instant) {
        self.incidents.push(Incident { seen_at: now, kind });
    }

    /// Record a heartbeat from `node` at `now`.
    pub fn record_heartbeat(&mut self, node: NodeId, now: Instant) {
        self.last_heartbeat.insert(node, now);
    }

    /// Record that a run with `id` started at `now`.
    pub fn record_run_started(&mut self, id: Uuid, now: Instant) {
        self.in_flight_runs.insert(id, now);
    }

    /// Record that a run with `id` finished at `now`. No-op if
    /// the run wasn't tracked (we can join the bus mid-stream and
    /// see a finish for a run we never saw start).
    pub fn record_run_finished(&mut self, id: Uuid, now: Instant) {
        if self.in_flight_runs.remove(&id).is_some() {
            self.last_run_finish = Some(now);
        }
    }

    /// Drop incidents older than `config.incident_ttl` and runs
    /// older than `config.run_ttl`. Cheap; called on every eval
    /// tick.
    pub fn age_out(&mut self, now: Instant, config: &SupervisorConfig) {
        self.incidents
            .retain(|i| now.duration_since(i.seen_at) <= config.incident_ttl);
        self.in_flight_runs
            .retain(|_, started| now.duration_since(*started) <= config.run_ttl);
    }

    /// Derive the current `(health, activity, reason)` from the
    /// model state at `now`. Pure: same inputs always yield the
    /// same output.
    pub fn derive(
        &self,
        now: Instant,
        config: &SupervisorConfig,
    ) -> (HealthState, ActivityState, Option<String>) {
        let (health, health_reason) = self.derive_health(now, config);
        let (activity, activity_reason) = self.derive_activity(now, config);
        let reason = compose_reason(health_reason, activity_reason);
        (health, activity, reason)
    }

    fn derive_health(
        &self,
        now: Instant,
        config: &SupervisorConfig,
    ) -> (HealthState, Option<String>) {
        // Highest contribution wins. Walk incidents and
        // heartbeats, tracking the worst observation seen and a
        // human-readable narration of *why*.
        let mut worst = HealthState::Healthy;
        let mut reason: Option<String> = None;

        let bump = |level: HealthState, narration: String, worst: &mut HealthState, reason: &mut Option<String>| {
            if level_rank(level) > level_rank(*worst) {
                *worst = level;
                *reason = Some(narration);
            }
        };

        for incident in &self.incidents {
            if now.duration_since(incident.seen_at) > config.incident_ttl {
                continue; // age_out should have removed this, but be safe
            }
            let contrib = incident.kind.health_contribution();
            if contrib != HealthState::Healthy {
                bump(
                    contrib,
                    format!("{} incident open", incident.kind.label()),
                    &mut worst,
                    &mut reason,
                );
            }
        }

        for (node, last) in &self.last_heartbeat {
            let stale = now.duration_since(*last);
            if stale >= config.critical_heartbeat_threshold {
                bump(
                    HealthState::Critical,
                    format!("heartbeat from {} stale by {:?}", node.0, stale),
                    &mut worst,
                    &mut reason,
                );
            } else if stale >= config.rescue_heartbeat_threshold {
                bump(
                    HealthState::Rescue,
                    format!("heartbeat from {} stale by {:?}", node.0, stale),
                    &mut worst,
                    &mut reason,
                );
            }
        }

        (worst, reason)
    }

    fn derive_activity(
        &self,
        now: Instant,
        config: &SupervisorConfig,
    ) -> (ActivityState, Option<String>) {
        if !self.in_flight_runs.is_empty() {
            return (
                ActivityState::Processing,
                Some(format!("{} in-flight run(s)", self.in_flight_runs.len())),
            );
        }
        // No live runs. Apply debounce so back-to-back runs don't
        // flap the indicator.
        if let Some(finished) = self.last_run_finish {
            if now.duration_since(finished) < config.idle_debounce {
                return (ActivityState::Processing, Some("idle debounce".into()));
            }
        }
        (ActivityState::Idle, None)
    }
}

fn level_rank(level: HealthState) -> u8 {
    match level {
        HealthState::Healthy => 0,
        HealthState::Rescue => 1,
        HealthState::Critical => 2,
    }
}

fn compose_reason(health: Option<String>, activity: Option<String>) -> Option<String> {
    match (health, activity) {
        (Some(h), Some(a)) => Some(format!("{}; {}", h, a)),
        (Some(h), None) => Some(h),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SupervisorConfig {
        SupervisorConfig::default()
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn empty_model_is_healthy_idle() {
        let model = SystemModel::new();
        let (health, activity, reason) = model.derive(t0(), &cfg());
        assert_eq!(health, HealthState::Healthy);
        assert_eq!(activity, ActivityState::Idle);
        assert!(reason.is_none());
    }

    #[test]
    fn self_heal_critical_flips_health_to_critical() {
        let mut model = SystemModel::new();
        let now = t0();
        model.record_incident(
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::Critical,
            },
            now,
        );
        let (health, _, reason) = model.derive(now, &cfg());
        assert_eq!(health, HealthState::Critical);
        assert!(reason.unwrap().contains("self-heal"));
    }

    #[test]
    fn self_heal_high_flips_health_to_rescue_not_critical() {
        let mut model = SystemModel::new();
        let now = t0();
        model.record_incident(
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::High,
            },
            now,
        );
        let (health, _, _) = model.derive(now, &cfg());
        assert_eq!(health, HealthState::Rescue);
    }

    #[test]
    fn self_heal_low_does_not_change_health() {
        let mut model = SystemModel::new();
        let now = t0();
        model.record_incident(
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::Low,
            },
            now,
        );
        let (health, _, _) = model.derive(now, &cfg());
        assert_eq!(health, HealthState::Healthy);
    }

    #[test]
    fn worst_incident_wins_when_multiple_open() {
        let mut model = SystemModel::new();
        let now = t0();
        model.record_incident(IncidentKind::MemoryLogDegraded, now); // Rescue
        model.record_incident(
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::Critical, // Critical
            },
            now,
        );
        let (health, _, _) = model.derive(now, &cfg());
        assert_eq!(health, HealthState::Critical);
    }

    #[test]
    fn aged_out_incident_returns_to_healthy() {
        let mut model = SystemModel::new();
        let config = SupervisorConfig {
            incident_ttl: Duration::from_secs(10),
            ..cfg()
        };
        let start = t0();
        model.record_incident(
            IncidentKind::SelfHeal {
                urgency: SelfHealUrgency::Critical,
            },
            start,
        );

        let later = start + Duration::from_secs(20);
        model.age_out(later, &config);
        let (health, _, _) = model.derive(later, &config);
        assert_eq!(health, HealthState::Healthy);
    }

    #[test]
    fn stale_heartbeat_promotes_to_rescue_then_critical() {
        let mut model = SystemModel::new();
        let config = SupervisorConfig {
            rescue_heartbeat_threshold: Duration::from_secs(5),
            critical_heartbeat_threshold: Duration::from_secs(10),
            ..cfg()
        };
        let node = NodeId(Uuid::new_v4());
        let start = t0();
        model.record_heartbeat(node.clone(), start);

        // 6 seconds later: stale enough for Rescue.
        let mid = start + Duration::from_secs(6);
        let (h, _, _) = model.derive(mid, &config);
        assert_eq!(h, HealthState::Rescue);

        // 11 seconds later: stale enough for Critical.
        let later = start + Duration::from_secs(11);
        let (h, _, _) = model.derive(later, &config);
        assert_eq!(h, HealthState::Critical);
    }

    #[test]
    fn fresh_heartbeat_keeps_node_healthy() {
        let mut model = SystemModel::new();
        let now = t0();
        model.record_heartbeat(NodeId(Uuid::new_v4()), now);
        let (h, _, _) = model.derive(now, &cfg());
        assert_eq!(h, HealthState::Healthy);
    }

    #[test]
    fn run_in_flight_marks_processing() {
        let mut model = SystemModel::new();
        let now = t0();
        model.record_run_started(Uuid::new_v4(), now);
        let (_, activity, reason) = model.derive(now, &cfg());
        assert_eq!(activity, ActivityState::Processing);
        assert!(reason.unwrap().contains("in-flight"));
    }

    #[test]
    fn run_finished_within_debounce_stays_processing() {
        let mut model = SystemModel::new();
        let config = SupervisorConfig {
            idle_debounce: Duration::from_secs(3),
            ..cfg()
        };
        let id = Uuid::new_v4();
        let start = t0();
        model.record_run_started(id, start);
        let finish = start + Duration::from_millis(500);
        model.record_run_finished(id, finish);

        // 1s after finish — still inside debounce.
        let now = finish + Duration::from_secs(1);
        let (_, activity, _) = model.derive(now, &config);
        assert_eq!(activity, ActivityState::Processing);
    }

    #[test]
    fn run_finished_past_debounce_drops_to_idle() {
        let mut model = SystemModel::new();
        let config = SupervisorConfig {
            idle_debounce: Duration::from_secs(3),
            ..cfg()
        };
        let id = Uuid::new_v4();
        let start = t0();
        model.record_run_started(id, start);
        let finish = start + Duration::from_millis(500);
        model.record_run_finished(id, finish);

        // 4s past finish — outside debounce.
        let now = finish + Duration::from_secs(4);
        let (_, activity, _) = model.derive(now, &config);
        assert_eq!(activity, ActivityState::Idle);
    }

    #[test]
    fn finish_for_unseen_run_is_silently_ignored() {
        let mut model = SystemModel::new();
        let now = t0();
        // We never recorded the start, but a finish arrives.
        // Should not panic, should not flag activity.
        model.record_run_finished(Uuid::new_v4(), now);
        let (_, activity, _) = model.derive(now, &cfg());
        assert_eq!(activity, ActivityState::Idle);
    }

    #[test]
    fn health_and_activity_compose_independently() {
        // Degraded while processing — both axes should reflect.
        let mut model = SystemModel::new();
        let now = t0();
        model.record_incident(IncidentKind::MemoryLogDegraded, now);
        model.record_run_started(Uuid::new_v4(), now);
        let (health, activity, reason) = model.derive(now, &cfg());
        assert_eq!(health, HealthState::Rescue);
        assert_eq!(activity, ActivityState::Processing);
        let r = reason.unwrap();
        assert!(r.contains("memory log degraded"));
        assert!(r.contains("in-flight"));
    }

    #[test]
    fn aged_out_runs_clear_in_flight_set() {
        let mut model = SystemModel::new();
        let config = SupervisorConfig {
            run_ttl: Duration::from_secs(60),
            idle_debounce: Duration::from_secs(0),
            ..cfg()
        };
        let start = t0();
        model.record_run_started(Uuid::new_v4(), start);
        // Long after run TTL, no finish arrived. Age out and
        // verify activity drops to Idle.
        let later = start + Duration::from_secs(120);
        model.age_out(later, &config);
        let (_, activity, _) = model.derive(later, &config);
        assert_eq!(activity, ActivityState::Idle);
    }
}
