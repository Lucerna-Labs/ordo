//! Avatar performance driver — step 3 of the avatar build order.
//!
//! Subscribes to the bus, tracks just enough state to compose
//! one [`AvatarFrame`] per tick, and republishes that frame on
//! [`avatar_topics::FRAME_EMITTED`] at a fixed ~30Hz cadence.
//!
//! The driver is deliberately the only place performance policy
//! lives. The protocol carries shapes; this crate decides:
//!
//! - Which phoneme is "current" given utterance-relative
//!   timestamps + wall-clock elapsed.
//! - Which [`Expression`] to pick from the active state mix
//!   (speaking trumps thinking trumps neutral; degraded health
//!   trumps everything).
//! - When (and how hard) to glitch.
//!
//! Renderer adapters (`ordo-studio`'s React canvas in step 6) do
//! not reproduce any of this — they just paint whatever frame
//! arrives.
//!
//! ## Boot integration (step 5 / runtime PR)
//!
//! Spawned by `ordo-runtime` exactly once, gated behind an
//! `enable_avatar` flag in [`AvatarConfig`]:
//!
//! ```ignore
//! components.push(spawn_component("avatar", async move {
//!     ordo_avatar::run(bus, AvatarConfig::default(), node_id).await;
//! }));
//! ```
//!
//! ## What this crate is *not*
//!
//! - Not a renderer — it picks intent (`mouth = Phoneme::Ae`,
//!   `glitch = GlitchLevel::Light`), not pixels.
//! - Not a phoneme deriver — that's `ordo-tts` (stub in step 2,
//!   real engine in step 7).
//! - Not a personality engine — v0.1 glitches on Critical health
//!   only. Mode-transition flashes and "brand beats" land later.

pub mod atlas;
pub mod state;

pub use atlas::{
    expression_to_cell, glitch_to_cell, phoneme_to_viseme, AtlasDescriptor, AtlasLayer,
    EXPRESSION_CELL_COUNT, GLITCH_CELL_COUNT, MOUTH_VISEME_COUNT,
};

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{
    avatar_topics, topics, AvatarFrame, BusEnvelope, Envelope, NodeId, OrdoMessage,
};
use tokio::time::{interval_at, Instant as TokioInstant, MissedTickBehavior};
use tracing::{error, info, warn};

pub use state::{AvatarConfig, AvatarModel};

/// Run the avatar driver. Returns when the bus subscription
/// ends (e.g. runtime shutdown) or fails to establish.
///
/// `node_id` is stamped on outgoing `AvatarFrameEmitted`
/// envelopes — typically the runtime's own `NodeId`.
pub async fn run(bus: Arc<dyn Bus>, config: AvatarConfig, node_id: NodeId) {
    let mut sub = match bus.subscribe(topics::ALL).await {
        Ok(s) => s,
        Err(err) => {
            error!(error = %err, "avatar: subscribe to ordo.* failed");
            return;
        }
    };
    info!(
        target: "ordo_avatar",
        frame_interval_ms = config.frame_interval.as_millis() as u64,
        "ordo-avatar: subscribed to ordo.*"
    );

    let mut model = AvatarModel::new();

    // Match the supervisor's headroom convention: first tick at
    // t + frame_interval rather than t=0, so any in-process
    // subscriber spawned during the same boot has a window to
    // come online before we start broadcasting frames.
    let start = TokioInstant::now() + config.frame_interval;
    let mut tick = interval_at(start, config.frame_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            envelope = sub.next() => {
                let Some(env) = envelope else {
                    info!("ordo-avatar: bus stream ended; exiting");
                    return;
                };
                ingest(&mut model, env, Instant::now());
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let frame = model.derive_frame(now, &config);
                publish_frame(bus.as_ref(), &node_id, frame).await;
            }
        }
    }
}

/// Translate one envelope into model updates. Keeps the
/// `OrdoMessage` match in this module so the state module stays
/// free of protocol-specific knowledge.
fn ingest(model: &mut AvatarModel, envelope: BusEnvelope, now: Instant) {
    match envelope.payload {
        OrdoMessage::TtsUtteranceStarted(start) => {
            model.on_utterance_start(start, now);
        }
        OrdoMessage::TtsPhonemeFrame(frame) => {
            model.on_phoneme_frame(frame);
        }
        OrdoMessage::TtsUtteranceEnded(end) => {
            model.on_utterance_end(end);
        }
        OrdoMessage::SystemStateChanged {
            health, activity, ..
        } => {
            model.on_system_state(health, activity, now);
        }
        // Everything else is irrelevant to the avatar driver.
        // We subscribe to `*` because future driver inputs
        // (typing-indicator topics, tool-call notifications)
        // will land here without needing a subscription change.
        _ => {}
    }
}

/// Publish one [`AvatarFrame`] on the bus. We always publish
/// every tick — see [`AvatarFrame`] doc comment for why. A
/// subscriber that wants only changes can dedupe by comparing
/// consecutive frames; the driver can't know which subscribers
/// care.
async fn publish_frame(bus: &dyn Bus, node_id: &NodeId, frame: AvatarFrame) {
    let envelope = Envelope::new(node_id.clone(), OrdoMessage::AvatarFrameEmitted(frame));
    if let Err(err) = bus.publish(avatar_topics::FRAME_EMITTED, envelope).await {
        warn!(target: "ordo_avatar", error = %err, "avatar.frame.emitted publish failed");
    }
}

/// Re-export so callers don't need to depend on `std::time`
/// directly for the common case of constructing an
/// [`AvatarConfig`] with a non-default frame rate.
pub fn frame_interval_from_hz(hz: u32) -> Duration {
    if hz == 0 {
        // Match `AvatarConfig::default()` behavior on a degenerate
        // input rather than dividing by zero. Callers that want
        // an explicit 30Hz should use the default.
        Duration::from_millis(33)
    } else {
        Duration::from_micros(1_000_000 / hz as u64)
    }
}
