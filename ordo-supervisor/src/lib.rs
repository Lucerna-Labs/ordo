//! Ordo Supervisor — derives system-wide health + activity from
//! bus signals and publishes [`OrdoMessage::SystemStateChanged`]
//! on transitions.
//!
//! ## What this crate is
//!
//! A single in-process tokio task that:
//!
//! 1. Subscribes to the bus (`ordo.*` wildcard).
//! 2. Records the inputs that matter — heartbeats, `*.degraded`
//!    events, self-heal incidents, run lifecycle — in a pure
//!    [`SystemModel`](state::SystemModel).
//! 3. On a fixed interval (default 1s), derives a `(HealthState,
//!    ActivityState, reason)` triple and publishes a new
//!    `SystemStateChanged` envelope **only when the derived
//!    state differs from the last publish**. The first
//!    derivation after start is always published so subscribers
//!    that join late aren't stuck displaying "unknown" forever.
//!
//! All derivation policy lives in [`state`]. The protocol carries
//! state; the supervisor decides when to transition. This crate
//! is the only place thresholds and self-heal mappings are
//! encoded.
//!
//! ## Boot integration
//!
//! Spawned by `ordo-runtime` exactly once, gated behind
//! `RuntimeConfig::enable_supervisor`:
//!
//! ```ignore
//! components.push(spawn_component("supervisor", async move {
//!     ordo_supervisor::run(bus, SupervisorConfig::default(), node_id).await;
//! }));
//! ```
//!
//! ## What this crate is *not*
//!
//! - Not a UI: `ordo-uxi` (separate PR) renders the state.
//! - Not a self-healer: it observes self-heal urgency, it doesn't
//!   plan or execute fixes.
//! - Not a metrics sink: state is operator-visible at the
//!   protocol level, not OTel histograms.

pub mod state;

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{
    mcp_topics, memory_topics, secrets_topics, topics, ActivityState, BusEnvelope, Envelope,
    HealthState, NodeId, OrdoMessage,
};
use tokio::time::{interval_at, Instant as TokioInstant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

pub use state::{IncidentKind, SupervisorConfig, SystemModel};

/// Run the supervisor. Returns when the bus subscription ends
/// (e.g. runtime shutdown) or fails.
///
/// `node_id` is the identity stamped on outgoing
/// `SystemStateChanged` envelopes — typically the runtime's own
/// `NodeId`.
///
/// # Known limitations (V1)
///
/// **Self-heal urgency mapping.** A single
/// [`SelfHealUrgency::Critical`](ordo_protocol::SelfHealUrgency::Critical)
/// incident is sufficient to promote system health to
/// [`HealthState::Critical`]. The rationale (see [`state`] module
/// docs) is that components don't fire `Critical` urgency lightly
/// and false negatives in an operator-facing readout are worse
/// than the brief flicker from a transient incident — the
/// 30-second [`SupervisorConfig::incident_ttl`] window absorbs
/// short blips. Reconsider this if production telemetry shows
/// transient `Critical` incidents are common; the alternative is
/// requiring multi-subsystem simultaneous degradation. The
/// threshold lives in [`SupervisorConfig`], **not on the wire** —
/// changing it is reversible without a protocol break.
///
/// **Initial-state publish window.** The supervisor publishes
/// its first state envelope at `t + eval_interval` after
/// subscribe (so subscribers spawned during the same boot have
/// a window to come online — see the `interval_at` call below).
/// **Subscribers that connect after the initial publish will not
/// see any state until the next genuine transition.** For
/// long-lived subscribers like `ordo-uxi` that boot alongside
/// the supervisor this is fine; for late joiners (a future UI
/// tab opening at runtime, an external operator dashboard
/// reconnecting) it isn't. Future work options:
/// - Periodic republish (every N seconds, regardless of
///   transition) — simplest, costs one envelope every N seconds.
/// - Explicit `SystemStateRequest` / `SystemStateResponse`
///   protocol pair — cleaner, but a protocol change.
/// - Bus-level last-message replay on subscribe — most general,
///   but a meaningful change to [`ordo_bus`].
pub async fn run(bus: Arc<dyn Bus>, config: SupervisorConfig, node_id: NodeId) {
    let mut sub = match bus.subscribe(topics::ALL).await {
        Ok(s) => s,
        Err(err) => {
            error!(error = %err, "supervisor: subscribe to ordo.* failed");
            return;
        }
    };
    info!("ordo-supervisor: subscribed to ordo.*");

    let mut model = SystemModel::new();
    // Delay the first tick by one full `eval_interval` rather
    // than firing immediately. The default `interval(period)`
    // ticks at t=0, which races against any subscriber that
    // joined moments later — broadcast channels don't replay, so
    // a late subscriber would miss the initial-state publish and
    // wait for the next genuine transition. One period of
    // headroom is enough for every in-process subscriber spawned
    // during the same boot to come online.
    let start = TokioInstant::now() + config.eval_interval;
    let mut tick = interval_at(start, config.eval_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // First derivation seeds subscribers; "transition from None
    // to Healthy" counts as a transition for publishing purposes.
    let mut last_published: Option<(HealthState, ActivityState)> = None;

    loop {
        tokio::select! {
            envelope = sub.next() => {
                let Some(env) = envelope else {
                    info!("ordo-supervisor: bus stream ended; exiting");
                    return;
                };
                ingest(&mut model, env, Instant::now());
            }
            _ = tick.tick() => {
                let now = Instant::now();
                model.age_out(now, &config);
                let (health, activity, reason) = model.derive(now, &config);

                if last_published != Some((health, activity)) {
                    publish_transition(
                        bus.as_ref(),
                        &node_id,
                        health,
                        activity,
                        reason.clone(),
                        last_published,
                    )
                    .await;
                    last_published = Some((health, activity));
                }
            }
        }
    }
}

/// Translate one envelope into model updates. Owns the entire
/// match over `OrdoMessage` so the state module stays free of
/// protocol-specific knowledge.
fn ingest(model: &mut SystemModel, envelope: BusEnvelope, now: Instant) {
    match &envelope.payload {
        OrdoMessage::Heartbeat(status) => {
            model.record_heartbeat(status.id.clone(), now);
        }
        OrdoMessage::SelfHealRequested { incident } => {
            model.record_incident(
                IncidentKind::SelfHeal {
                    urgency: incident.urgency,
                },
                now,
            );
        }
        OrdoMessage::MemoryLogHealthDegraded { .. } => {
            model.record_incident(IncidentKind::MemoryLogDegraded, now);
        }
        OrdoMessage::MemoryProjectionReplayDegraded { .. } => {
            model.record_incident(IncidentKind::MemoryProjectionDegraded, now);
        }
        OrdoMessage::SecretsSealTierDegraded { .. } => {
            model.record_incident(IncidentKind::SecretsSealDegraded, now);
        }
        OrdoMessage::McpClientAuthDegraded { .. } => {
            model.record_incident(IncidentKind::McpAuthDegraded, now);
        }
        OrdoMessage::RunRequested { run_id, .. } => {
            model.record_run_started(*run_id, now);
        }
        OrdoMessage::RunFinished { run_id, .. } => {
            model.record_run_finished(*run_id, now);
        }
        // Everything else is informational for the supervisor's
        // purposes. Keeping a wildcard arm rather than enumerating
        // is intentional — new variants land regularly and the
        // supervisor doesn't care about most of them.
        _ => {}
    }

    // Touch the protocol topic constants so `cargo check` flags
    // it if any are renamed or removed upstream. The supervisor
    // itself uses the `ordo.*` wildcard subscription and dispatches
    // by `OrdoMessage` variant, but linking to these constants
    // makes the dependency on those topics explicit at compile
    // time.
    let _ = (
        memory_topics::LOG_HEALTH_DEGRADED,
        memory_topics::PROJECTION_REPLAY_DEGRADED,
        secrets_topics::VAULT_SEAL_TIER_DEGRADED,
        mcp_topics::CLIENT_AUTH_DEGRADED,
    );
}

async fn publish_transition(
    bus: &dyn Bus,
    node_id: &NodeId,
    health: HealthState,
    activity: ActivityState,
    reason: Option<String>,
    previous: Option<(HealthState, ActivityState)>,
) {
    let envelope = Envelope::new(
        node_id.clone(),
        OrdoMessage::SystemStateChanged {
            health,
            activity,
            reason: reason.clone(),
        },
    );
    match bus.publish(topics::SYSTEM_STATE, envelope).await {
        Ok(()) => match previous {
            Some(prev) => info!(
                from_health = ?prev.0,
                from_activity = ?prev.1,
                to_health = ?health,
                to_activity = ?activity,
                reason = reason.as_deref().unwrap_or(""),
                "ordo-supervisor: state transition"
            ),
            None => info!(
                health = ?health,
                activity = ?activity,
                reason = reason.as_deref().unwrap_or(""),
                "ordo-supervisor: initial state"
            ),
        },
        Err(err) => {
            warn!(error = %err, "ordo-supervisor: publish failed");
            debug!(?health, ?activity, "publish target was {topic}", topic = topics::SYSTEM_STATE);
        }
    }
}
