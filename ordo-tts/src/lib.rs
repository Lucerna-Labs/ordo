//! `ordo-tts` — stub TTS producer.
//!
//! Step 2 of the avatar build order. Emits the three `ordo.tts.*`
//! envelopes defined by [`ordo_protocol::tts`]:
//!
//! 1. [`UtteranceStart`] when [`TtsService::speak`] is invoked.
//! 2. A schedule of [`PhonemeFrame`]s spaced out in real time
//!    according to each phoneme's `duration_ms`.
//! 3. [`UtteranceEnd`] when the schedule finishes.
//!
//! The phoneme schedule is **canned** — derived from the input
//! text by a deliberately stupid character→phoneme map. The real
//! engine (step 7) will compute real ARPAbet from text + voice,
//! but the wire format does not change. Consumers that work
//! against the stub also work against the real engine.
//!
//! Construction follows the codebase convention (Rule 7):
//! ```ignore
//! let tts = TtsService::new().with_bus(bus.clone());
//! tts.speak("hello ordo".to_string()).await;
//! ```
//!
//! `with_bus` is optional. Without a bus the schedule is computed
//! and the task runs, but nothing is published — useful for tests
//! and for `cargo check`-style smoke tests on machines that don't
//! wire a runtime.

use std::sync::Arc;
use std::time::Duration;

use ordo_bus::Bus;
use ordo_protocol::{
    tts_topics, Envelope, NodeId, OrdoMessage, Phoneme, PhonemeFrame, UtteranceEnd, UtteranceStart,
};
use uuid::Uuid;

/// Default per-phoneme duration in milliseconds. Picked to feel
/// like natural speech at ~12 phonemes/second; tunable per
/// utterance via [`SpeakOptions::phoneme_duration_ms`] when the
/// caller wants a different pace (e.g. fast diagnostic chatter).
pub const DEFAULT_PHONEME_DURATION_MS: u32 = 85;

/// Slightly longer pause for the `Rest` phoneme between words.
/// Two `Rest`s back-to-back across a word boundary would feel
/// stilted; giving `Rest` a longer single beat reads better.
pub const DEFAULT_REST_DURATION_MS: u32 = 120;

/// Stub TTS service. Publishes [`OrdoMessage::TtsUtteranceStarted`],
/// [`OrdoMessage::TtsPhonemeFrame`], and
/// [`OrdoMessage::TtsUtteranceEnded`] envelopes on the bus when
/// [`Self::speak`] is invoked.
#[derive(Clone)]
pub struct TtsService {
    node_id: NodeId,
    bus: Option<Arc<dyn Bus>>,
}

impl Default for TtsService {
    fn default() -> Self {
        Self::new()
    }
}

impl TtsService {
    pub fn new() -> Self {
        Self {
            node_id: NodeId::new(),
            bus: None,
        }
    }

    /// Wire a bus. Without one, [`Self::speak`] still runs the
    /// schedule but does not publish.
    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Speak `text`. Returns the `utterance_id` so callers can
    /// correlate logs / route concurrent utterances. The actual
    /// phoneme stream runs in a detached tokio task — the future
    /// returned by `speak` resolves as soon as `UtteranceStart` is
    /// published (so the caller can issue follow-up work without
    /// waiting for the entire utterance to play out).
    pub async fn speak(&self, text: String) -> Uuid {
        self.speak_with_options(text, SpeakOptions::default()).await
    }

    /// Speak with explicit options (voice id, per-phoneme pacing).
    pub async fn speak_with_options(&self, text: String, options: SpeakOptions) -> Uuid {
        let utterance_id = Uuid::new_v4();
        let schedule = schedule_phonemes(&text, &options);

        // Publish UtteranceStart synchronously so consumers see
        // the speaking-state transition before any phoneme.
        if let Some(bus) = &self.bus {
            let start = UtteranceStart {
                utterance_id,
                text: text.clone(),
                voice_id: options.voice_id.clone(),
            };
            let envelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::TtsUtteranceStarted(start),
            );
            if let Err(err) = bus.publish(tts_topics::UTTERANCE_STARTED, envelope).await {
                tracing::warn!(target: "ordo_tts", error = %err, "utterance.started publish failed");
            }
        }

        // The phoneme stream runs detached. We hold an `Arc<dyn Bus>`
        // clone so the task can outlive this call.
        let bus = self.bus.clone();
        let node_id = self.node_id.clone();
        tokio::spawn(async move {
            run_schedule(node_id, bus, utterance_id, schedule).await;
        });

        utterance_id
    }
}

/// Tunable per-utterance knobs. Defaults reproduce step-2 spec
/// behaviour (~12 phonemes/s, longer rest beats).
#[derive(Debug, Clone, Default)]
pub struct SpeakOptions {
    /// Override the per-phoneme duration (non-`Rest`). `None`
    /// uses [`DEFAULT_PHONEME_DURATION_MS`].
    pub phoneme_duration_ms: Option<u32>,
    /// Override the `Rest` phoneme duration. `None` uses
    /// [`DEFAULT_REST_DURATION_MS`].
    pub rest_duration_ms: Option<u32>,
    /// Voice profile id. Echoed verbatim on `UtteranceStart`;
    /// the stub ignores it but downstream consumers (and the
    /// real engine in step 7) consult it.
    pub voice_id: Option<String>,
}

/// One scheduled phoneme: which one, when (utterance-relative,
/// ms), and how long to hold it. Public so consumers / tests can
/// inspect the schedule without invoking the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledPhoneme {
    pub phoneme: Phoneme,
    pub timestamp_ms: u64,
    pub duration_ms: u32,
}

/// Pure text → phoneme schedule. No clock, no bus.
///
/// Stub heuristic: lowercase the text, map ASCII letters
/// through a fixed table, treat whitespace + punctuation as
/// [`Phoneme::Rest`]. Consecutive rests are collapsed so we
/// don't ship dead beats. The result is a flat schedule whose
/// timestamps run forward by `duration_ms` from `0`.
///
/// This is deliberately not linguistically accurate — the real
/// engine in step 7 replaces it. The job here is to produce a
/// plausible-feeling cadence so the avatar driver has something
/// to react to.
pub fn schedule_phonemes(text: &str, options: &SpeakOptions) -> Vec<ScheduledPhoneme> {
    let phoneme_dur = options
        .phoneme_duration_ms
        .unwrap_or(DEFAULT_PHONEME_DURATION_MS);
    let rest_dur = options.rest_duration_ms.unwrap_or(DEFAULT_REST_DURATION_MS);

    let mut schedule: Vec<ScheduledPhoneme> = Vec::new();
    let mut cursor_ms: u64 = 0;

    for ch in text.chars() {
        let next = char_to_phoneme(ch);

        // Collapse consecutive Rests — multiple spaces, trailing
        // punctuation runs, etc., shouldn't extend silence past
        // a single rest beat.
        if next == Phoneme::Rest
            && schedule
                .last()
                .map(|last| last.phoneme == Phoneme::Rest)
                .unwrap_or(false)
        {
            continue;
        }

        let duration_ms = if next == Phoneme::Rest {
            rest_dur
        } else {
            phoneme_dur
        };

        schedule.push(ScheduledPhoneme {
            phoneme: next,
            timestamp_ms: cursor_ms,
            duration_ms,
        });
        cursor_ms = cursor_ms.saturating_add(duration_ms as u64);
    }

    // Trailing rest is unhelpful — the avatar driver already
    // returns to a closed mouth on UtteranceEnd. Trim it.
    if schedule
        .last()
        .map(|last| last.phoneme == Phoneme::Rest)
        .unwrap_or(false)
    {
        schedule.pop();
    }

    schedule
}

/// Drive the scheduled phoneme stream against a real clock,
/// publishing each [`PhonemeFrame`] on the bus when its
/// `timestamp_ms` arrives, then publishing [`UtteranceEnd`].
async fn run_schedule(
    node_id: NodeId,
    bus: Option<Arc<dyn Bus>>,
    utterance_id: Uuid,
    schedule: Vec<ScheduledPhoneme>,
) {
    let start = tokio::time::Instant::now();

    for entry in &schedule {
        let target = start + Duration::from_millis(entry.timestamp_ms);
        tokio::time::sleep_until(target).await;

        if let Some(bus) = &bus {
            let frame = PhonemeFrame {
                utterance_id,
                phoneme: entry.phoneme,
                timestamp_ms: entry.timestamp_ms,
                duration_ms: entry.duration_ms,
            };
            let envelope = Envelope::new(node_id.clone(), OrdoMessage::TtsPhonemeFrame(frame));
            if let Err(err) = bus.publish(tts_topics::PHONEME_FRAME, envelope).await {
                tracing::warn!(target: "ordo_tts", error = %err, "phoneme.frame publish failed");
            }
        }
    }

    // Wait out the final phoneme's duration before sending
    // UtteranceEnd, so consumers see the last viseme held for
    // its full beat rather than truncated.
    if let Some(last) = schedule.last() {
        let tail = start
            + Duration::from_millis(last.timestamp_ms)
            + Duration::from_millis(last.duration_ms as u64);
        tokio::time::sleep_until(tail).await;
    }

    if let Some(bus) = &bus {
        let end = UtteranceEnd { utterance_id };
        let envelope = Envelope::new(node_id.clone(), OrdoMessage::TtsUtteranceEnded(end));
        if let Err(err) = bus.publish(tts_topics::UTTERANCE_ENDED, envelope).await {
            tracing::warn!(target: "ordo_tts", error = %err, "utterance.ended publish failed");
        }
    }
}

/// ASCII character → stub phoneme. Lowercases, treats anything
/// non-alphabetic as [`Phoneme::Rest`]. The map intentionally
/// uses a small subset of ARPAbet — it's not trying to be
/// accurate, just non-degenerate so the visemes change frame to
/// frame.
fn char_to_phoneme(ch: char) -> Phoneme {
    let lower = ch.to_ascii_lowercase();
    match lower {
        // Vowels
        'a' => Phoneme::Ae,
        'e' => Phoneme::Eh,
        'i' => Phoneme::Ih,
        'o' => Phoneme::Ow,
        'u' => Phoneme::Uh,
        'y' => Phoneme::Iy,
        // Consonants — single-letter mappings to the closest
        // common ARPAbet phoneme. Digraphs (sh, ch, th, ng) are
        // not detected by the stub; the real engine handles them.
        'b' => Phoneme::B,
        'c' => Phoneme::K,
        'd' => Phoneme::D,
        'f' => Phoneme::F,
        'g' => Phoneme::G,
        'h' => Phoneme::Hh,
        'j' => Phoneme::Jh,
        'k' => Phoneme::K,
        'l' => Phoneme::L,
        'm' => Phoneme::M,
        'n' => Phoneme::N,
        'p' => Phoneme::P,
        'q' => Phoneme::K,
        'r' => Phoneme::R,
        's' => Phoneme::S,
        't' => Phoneme::T,
        'v' => Phoneme::V,
        'w' => Phoneme::W,
        'x' => Phoneme::K,
        'z' => Phoneme::Z,
        _ => Phoneme::Rest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_empty_text_is_empty() {
        let schedule = schedule_phonemes("", &SpeakOptions::default());
        assert!(schedule.is_empty());
    }

    #[test]
    fn schedule_lays_out_timestamps_in_order() {
        let schedule = schedule_phonemes("hi", &SpeakOptions::default());
        assert_eq!(schedule.len(), 2);
        assert_eq!(schedule[0].phoneme, Phoneme::Hh);
        assert_eq!(schedule[0].timestamp_ms, 0);
        assert_eq!(schedule[1].phoneme, Phoneme::Ih);
        assert_eq!(schedule[1].timestamp_ms, DEFAULT_PHONEME_DURATION_MS as u64);
    }

    #[test]
    fn schedule_inserts_rest_between_words_but_collapses_runs() {
        // Two spaces should collapse to a single rest beat.
        let schedule = schedule_phonemes("a  b", &SpeakOptions::default());
        let rests = schedule
            .iter()
            .filter(|entry| entry.phoneme == Phoneme::Rest)
            .count();
        assert_eq!(rests, 1, "consecutive spaces should collapse");
    }

    #[test]
    fn schedule_trims_trailing_rest() {
        let schedule = schedule_phonemes("hi.", &SpeakOptions::default());
        // The '.' would be Rest; trailing rest is trimmed.
        assert_eq!(schedule.len(), 2);
        assert!(schedule.iter().all(|entry| entry.phoneme != Phoneme::Rest));
    }

    #[test]
    fn schedule_honors_custom_durations() {
        let options = SpeakOptions {
            phoneme_duration_ms: Some(50),
            rest_duration_ms: Some(200),
            voice_id: None,
        };
        let schedule = schedule_phonemes("a b", &options);
        assert_eq!(schedule[0].duration_ms, 50);
        assert_eq!(schedule[1].phoneme, Phoneme::Rest);
        assert_eq!(schedule[1].duration_ms, 200);
        assert_eq!(schedule[2].duration_ms, 50);
    }
}
