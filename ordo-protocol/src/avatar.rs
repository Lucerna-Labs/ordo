//! Avatar message types.
//!
//! [`AvatarFrame`] is the per-tick state emitted by the avatar
//! performance driver (`claw-avatar`) and consumed by renderer
//! adapters. v0.1 has exactly one consumer
//! (`claw-avatar-sprite`, the sprite-atlas adapter routed
//! through the WebView2 renderer). Future adapters — a Vello
//! native path, a 3D head, a debug visualizer, a streaming
//! exporter — read the same struct unchanged.
//!
//! Deliberately small for v0.1. Three fields. Gaze, blink,
//! head tilt, idle-motion seed — all deferred. Add them when a
//! consumer needs them, not before.
//!
//! The driver emits at a fixed rate (~30Hz in v0.1) reflecting
//! the avatar's current state. The state is derived from the
//! TTS message stream ([`crate::tts`]), the runtime's
//! `SystemStateChanged` events, and the driver's own
//! personality profile (timing of glitches, blinks, etc.).
//!
//! Topic strings live in [`avatar_topics`]. Naming follows the
//! existing `ordo.<area>.<thing>.<verb>` convention.

use serde::{Deserialize, Serialize};

use crate::tts::Phoneme;

/// Expression layer — the avatar's emotional/cognitive state.
/// One of a small fixed set, deliberately. v0.1 uses these to
/// pick one of 6 expression-layer sprite cells from the atlas.
///
/// State semantics:
/// - [`Expression::Neutral`] — idle, not speaking, no warnings.
/// - [`Expression::Speaking`] — actively producing speech (at
///   least one phoneme in flight). The driver sets this between
///   `UtteranceStart` and `UtteranceEnd` for the most recent
///   utterance.
/// - [`Expression::Thinking`] — the runtime is computing but
///   not yet emitting speech. Set when `SystemStateChanged`
///   shows the activity is `Processing` and no utterance is
///   currently in flight.
/// - [`Expression::Alarmed`] — runtime health degraded
///   (`Rescue` or `Critical`).
/// - [`Expression::Amused`] — reserved for v0.2 (personality
///   reactions to specific message content). Kept in v0.1 so
///   the enum doesn't need a wire-format break later.
/// - [`Expression::Glitched`] — paired with a non-`None`
///   [`GlitchLevel`] when the avatar is intentionally
///   distorting itself (mode transitions, error states, brand
///   beats). Renderers composite the matching glitch overlay on
///   top of the expression sprite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Expression {
    Neutral,
    Speaking,
    Thinking,
    Alarmed,
    Amused,
    Glitched,
}

/// Glitch overlay intensity. Three steps so the renderer can
/// pick the right overlay cell without coordination from the
/// driver — the driver decides the *level*, the adapter decides
/// the *pixels*.
///
/// In v0.1 the sprite atlas carries:
/// - `Light` → faint chromatic shift + occasional scanline.
/// - `Heavy` → full-frame fragmentation + RGB split.
///
/// Authors can re-tune the atlas without breaking the
/// protocol — the levels are intent, not specific pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GlitchLevel {
    None,
    Light,
    Heavy,
}

/// One frame of avatar state. Emitted by the driver at a fixed
/// rate (~30Hz in v0.1) regardless of whether anything changed
/// — keeps the consumer simple (always-on render loop) and
/// makes the seam ready for a streaming-output adapter without
/// special-casing "no change" gaps.
///
/// The three fields compose: the renderer picks one mouth cell,
/// one expression cell, and (if `glitch != None`) overlays one
/// glitch cell. Layers compose at runtime; the atlas does NOT
/// pre-render the combinatorial product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarFrame {
    /// Current mouth shape. [`Phoneme::Rest`] when silent.
    /// Renderers map this to one of N viseme cells (8 in v0.1).
    pub mouth: Phoneme,
    /// Current expression layer.
    pub expression: Expression,
    /// Glitch overlay intensity. [`GlitchLevel::None`] means
    /// no overlay is composited.
    pub glitch: GlitchLevel,
}

/// Bus topic for avatar-driver output. Producer: `claw-avatar`.
/// Consumers: renderer adapters (`claw-avatar-sprite` in v0.1).
pub mod avatar_topics {
    /// One [`super::AvatarFrame`] per tick from the driver.
    pub const FRAME_EMITTED: &str = "ordo.avatar.frame.emitted";
}
