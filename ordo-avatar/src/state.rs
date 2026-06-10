//! Avatar state model. Pure — no bus, no tokio, no logging.
//! Every input is an explicit method call; every output is an
//! [`AvatarFrame`] returned from [`AvatarModel::derive_frame`].
//!
//! The model is the only place that knows how to compose:
//!
//! - **mouth**: which [`Phoneme`] is "current" given the
//!   utterance-relative phoneme schedule and a wall-clock
//!   anchor.
//! - **expression**: priority order — degraded health > active
//!   utterance > processing > idle.
//! - **glitch**: v0.1 minimal — `Heavy` while system health is
//!   `Critical`, `Light` for a brief window after a health
//!   transition, otherwise `None`. The personality / mode-
//!   transition / brand-beat layer lands in a later PR.

use std::time::{Duration, Instant};

use ordo_protocol::{
    ActivityState, AvatarFrame, Expression, GlitchLevel, HealthState, Phoneme, PhonemeFrame,
    UtteranceEnd, UtteranceStart,
};
use uuid::Uuid;

/// Tunable knobs for the driver. Frame interval is the only one
/// today; future personality timings (blink rate, glitch
/// frequency) live here so the protocol stays untouched.
#[derive(Debug, Clone)]
pub struct AvatarConfig {
    /// Interval between emitted [`AvatarFrame`]s. Default 33ms
    /// ≈ 30Hz. Lower for higher-fidelity mouth animation; higher
    /// for low-power modes.
    pub frame_interval: Duration,
    /// How long after a health transition (e.g. Healthy →
    /// Rescue) to hold a `Light` glitch overlay. Default 600ms
    /// — a single brand beat, not a sustained distortion.
    pub transition_glitch_window: Duration,
}

impl Default for AvatarConfig {
    fn default() -> Self {
        Self {
            frame_interval: Duration::from_millis(33),
            transition_glitch_window: Duration::from_millis(600),
        }
    }
}

/// In-flight utterance state. Holds the phoneme schedule the
/// TTS producer has streamed so far + a wall-clock anchor so
/// the driver can answer "which phoneme is current right now?"
#[derive(Debug, Clone)]
struct Utterance {
    id: Uuid,
    /// Wall-clock anchor matched to the utterance's t=0. Set on
    /// `UtteranceStart`; phoneme `timestamp_ms` is added to this
    /// to get a real `Instant`.
    started_at: Instant,
    /// All [`PhonemeFrame`]s received for this utterance, in
    /// arrival order. v0.1 trusts arrival order to match
    /// `timestamp_ms` order (the stub producer guarantees this;
    /// the real engine in step 7 should too).
    frames: Vec<PhonemeFrame>,
}

/// Pure state model. Construct with [`Self::new`], drive with
/// the `on_*` mutators, read back with
/// [`Self::derive_frame`].
#[derive(Debug, Clone)]
pub struct AvatarModel {
    utterance: Option<Utterance>,
    health: HealthState,
    activity: ActivityState,
    /// Set on each `SystemStateChanged` ingest. Used to drive
    /// the brief `Light` glitch after a state transition.
    last_health_transition: Option<Instant>,
}

impl Default for AvatarModel {
    fn default() -> Self {
        Self::new()
    }
}

impl AvatarModel {
    pub fn new() -> Self {
        Self {
            utterance: None,
            health: HealthState::Healthy,
            activity: ActivityState::Idle,
            last_health_transition: None,
        }
    }

    /// Begin tracking a new utterance. If one was already in
    /// flight the new one supersedes it — concurrent utterances
    /// are not supported by the v0.1 driver (the avatar has one
    /// mouth).
    pub fn on_utterance_start(&mut self, start: UtteranceStart, now: Instant) {
        self.utterance = Some(Utterance {
            id: start.utterance_id,
            started_at: now,
            frames: Vec::new(),
        });
    }

    /// Append a phoneme to the active utterance's schedule. A
    /// frame for an unknown utterance id is dropped silently —
    /// stale frames from a superseded utterance shouldn't move
    /// the mouth on the current one.
    pub fn on_phoneme_frame(&mut self, frame: PhonemeFrame) {
        if let Some(active) = &mut self.utterance {
            if active.id == frame.utterance_id {
                active.frames.push(frame);
            }
        }
    }

    /// Close the active utterance, if it matches. Mismatched ids
    /// are ignored.
    pub fn on_utterance_end(&mut self, end: UtteranceEnd) {
        if let Some(active) = &self.utterance {
            if active.id == end.utterance_id {
                self.utterance = None;
            }
        }
    }

    /// Update the system state inputs. Records the transition
    /// instant when health changes so [`Self::derive_frame`] can
    /// fire a brief `Light` glitch.
    pub fn on_system_state(
        &mut self,
        health: HealthState,
        activity: ActivityState,
        now: Instant,
    ) {
        if self.health != health {
            self.last_health_transition = Some(now);
        }
        self.health = health;
        self.activity = activity;
    }

    /// Compose one [`AvatarFrame`] from the current model state.
    /// Always returns a frame — there's no "no change" case
    /// (see [`AvatarFrame`] doc comment).
    pub fn derive_frame(&self, now: Instant, config: &AvatarConfig) -> AvatarFrame {
        AvatarFrame {
            mouth: self.derive_mouth(now),
            expression: self.derive_expression(),
            glitch: self.derive_glitch(now, config),
        }
    }

    fn derive_mouth(&self, now: Instant) -> Phoneme {
        let Some(active) = &self.utterance else {
            return Phoneme::Rest;
        };

        // Compute elapsed since the utterance started. Saturate
        // at zero in the (impossible-in-practice) case where
        // `now < started_at`.
        let elapsed = now.saturating_duration_since(active.started_at);
        let elapsed_ms = elapsed.as_millis() as u64;

        // Find the phoneme whose [timestamp_ms, timestamp_ms +
        // duration_ms) window contains `elapsed_ms`. Frames
        // arrive in order; walk from the back for cheap lookup
        // on the common case (we're near the end of what's
        // arrived). The schedule is small — ~12 frames/s × a
        // typical few-second utterance — so the linear scan is
        // fine.
        for frame in active.frames.iter().rev() {
            if elapsed_ms >= frame.timestamp_ms {
                let end_ms = frame.timestamp_ms.saturating_add(frame.duration_ms as u64);
                if elapsed_ms < end_ms {
                    return frame.phoneme;
                }
                // Past the duration of the latest frame we've
                // seen — hold Rest until the next frame arrives.
                // (Holding the last viseme would feel "stuck";
                // Rest reads as a natural pause.)
                return Phoneme::Rest;
            }
        }

        // Before the first phoneme has arrived: closed mouth.
        Phoneme::Rest
    }

    fn derive_expression(&self) -> Expression {
        // Priority order:
        // 1. Degraded health overrides everything — operators
        //    need to see "something is wrong" even mid-speech.
        // 2. Otherwise, an in-flight utterance shows Speaking.
        // 3. Otherwise, Processing activity shows Thinking.
        // 4. Otherwise, Neutral.
        match self.health {
            HealthState::Rescue | HealthState::Critical => return Expression::Alarmed,
            HealthState::Healthy => {}
        }

        if self.utterance.is_some() {
            return Expression::Speaking;
        }

        match self.activity {
            ActivityState::Processing => Expression::Thinking,
            ActivityState::Idle => Expression::Neutral,
        }
    }

    fn derive_glitch(&self, now: Instant, config: &AvatarConfig) -> GlitchLevel {
        // Critical health → sustained Heavy distortion. Rescue
        // doesn't trigger sustained glitch (the Alarmed
        // expression carries the signal); only Critical earns
        // the wire fragmentation.
        if matches!(self.health, HealthState::Critical) {
            return GlitchLevel::Heavy;
        }

        // Brief Light flash after a health-state transition.
        if let Some(transition_at) = self.last_health_transition {
            if now.saturating_duration_since(transition_at) < config.transition_glitch_window {
                return GlitchLevel::Light;
            }
        }

        GlitchLevel::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(utterance_id: Uuid, phoneme: Phoneme, timestamp_ms: u64, duration_ms: u32) -> PhonemeFrame {
        PhonemeFrame {
            utterance_id,
            phoneme,
            timestamp_ms,
            duration_ms,
        }
    }

    #[test]
    fn idle_model_emits_neutral_rest_no_glitch() {
        let model = AvatarModel::new();
        let frame = model.derive_frame(Instant::now(), &AvatarConfig::default());
        assert_eq!(frame.mouth, Phoneme::Rest);
        assert_eq!(frame.expression, Expression::Neutral);
        assert_eq!(frame.glitch, GlitchLevel::None);
    }

    #[test]
    fn speaking_picks_phoneme_inside_window() {
        let id = Uuid::new_v4();
        let start_at = Instant::now();
        let mut model = AvatarModel::new();
        model.on_utterance_start(
            UtteranceStart {
                utterance_id: id,
                text: "hi".into(),
                voice_id: None,
            },
            start_at,
        );
        model.on_phoneme_frame(frame(id, Phoneme::Hh, 0, 100));
        model.on_phoneme_frame(frame(id, Phoneme::Ih, 100, 100));

        // 50ms in — middle of Hh.
        let f = model.derive_frame(start_at + Duration::from_millis(50), &AvatarConfig::default());
        assert_eq!(f.mouth, Phoneme::Hh);
        assert_eq!(f.expression, Expression::Speaking);

        // 150ms in — middle of Ih.
        let f = model.derive_frame(start_at + Duration::from_millis(150), &AvatarConfig::default());
        assert_eq!(f.mouth, Phoneme::Ih);

        // 250ms in — past the end of the schedule, mouth rests
        // but expression stays Speaking until UtteranceEnd.
        let f = model.derive_frame(start_at + Duration::from_millis(250), &AvatarConfig::default());
        assert_eq!(f.mouth, Phoneme::Rest);
        assert_eq!(f.expression, Expression::Speaking);
    }

    #[test]
    fn stale_phoneme_for_superseded_utterance_is_ignored() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let mut model = AvatarModel::new();
        model.on_utterance_start(
            UtteranceStart {
                utterance_id: id_a,
                text: "a".into(),
                voice_id: None,
            },
            Instant::now(),
        );
        // Start a second utterance, replacing the first.
        let start_b = Instant::now();
        model.on_utterance_start(
            UtteranceStart {
                utterance_id: id_b,
                text: "b".into(),
                voice_id: None,
            },
            start_b,
        );
        // Stale frame for id_a should not influence id_b.
        model.on_phoneme_frame(frame(id_a, Phoneme::Ae, 0, 500));
        let f = model.derive_frame(start_b + Duration::from_millis(10), &AvatarConfig::default());
        assert_eq!(f.mouth, Phoneme::Rest, "stale frame must not bleed into new utterance");
    }

    #[test]
    fn utterance_end_returns_mouth_to_rest_and_expression_to_neutral() {
        let id = Uuid::new_v4();
        let mut model = AvatarModel::new();
        model.on_utterance_start(
            UtteranceStart {
                utterance_id: id,
                text: "a".into(),
                voice_id: None,
            },
            Instant::now(),
        );
        model.on_utterance_end(UtteranceEnd { utterance_id: id });
        let f = model.derive_frame(Instant::now(), &AvatarConfig::default());
        assert_eq!(f.mouth, Phoneme::Rest);
        assert_eq!(f.expression, Expression::Neutral);
    }

    #[test]
    fn processing_activity_with_no_utterance_shows_thinking() {
        let mut model = AvatarModel::new();
        model.on_system_state(HealthState::Healthy, ActivityState::Processing, Instant::now());
        // The transition from default (Healthy) to Healthy isn't
        // a change, so no glitch should fire.
        let f = model.derive_frame(Instant::now(), &AvatarConfig::default());
        assert_eq!(f.expression, Expression::Thinking);
        assert_eq!(f.glitch, GlitchLevel::None);
    }

    #[test]
    fn degraded_health_overrides_speaking_and_thinking() {
        let mut model = AvatarModel::new();
        let now = Instant::now();
        model.on_system_state(HealthState::Rescue, ActivityState::Processing, now);
        // Start an utterance — Alarmed should still win.
        model.on_utterance_start(
            UtteranceStart {
                utterance_id: Uuid::new_v4(),
                text: "a".into(),
                voice_id: None,
            },
            now,
        );
        let f = model.derive_frame(now, &AvatarConfig::default());
        assert_eq!(f.expression, Expression::Alarmed);
    }

    #[test]
    fn critical_health_emits_heavy_glitch() {
        let mut model = AvatarModel::new();
        let now = Instant::now();
        model.on_system_state(HealthState::Critical, ActivityState::Idle, now);
        // Skip past the transition window.
        let later = now + Duration::from_secs(5);
        let f = model.derive_frame(later, &AvatarConfig::default());
        assert_eq!(f.glitch, GlitchLevel::Heavy);
    }

    #[test]
    fn brief_light_glitch_after_health_transition() {
        let mut model = AvatarModel::new();
        let now = Instant::now();
        model.on_system_state(HealthState::Rescue, ActivityState::Idle, now);
        // Immediately after transition, Light flash.
        let f = model.derive_frame(now + Duration::from_millis(100), &AvatarConfig::default());
        assert_eq!(f.glitch, GlitchLevel::Light);
        // After the window closes, no glitch (Rescue alone
        // doesn't sustain glitch).
        let f = model.derive_frame(now + Duration::from_secs(5), &AvatarConfig::default());
        assert_eq!(f.glitch, GlitchLevel::None);
    }
}
