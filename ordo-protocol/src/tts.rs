//! Text-to-speech message types.
//!
//! Three structs framing a single spoken utterance:
//!
//! - [`UtteranceStart`] — marks the beginning of an utterance,
//!   carries the source text (for captions / logs) and an
//!   optional voice profile id.
//! - [`PhonemeFrame`] — individual phoneme events streamed
//!   between an [`UtteranceStart`] and its matching
//!   [`UtteranceEnd`]. Timestamps are **utterance-relative**
//!   (offset in ms from `UtteranceStart`), NOT wall-clock. This
//!   is the critical property that makes record-and-replay,
//!   cross-process avatars, and offline rendering possible
//!   later. Wall-clock synchronization is the consumer's job.
//! - [`UtteranceEnd`] — closes the utterance. Lets consumers
//!   exit the speaking state independently of seeing the last
//!   phoneme (in case of network loss, dropped frames, etc.).
//!
//! Plus [`Phoneme`] — the ARPAbet-derived enum used by
//! [`PhonemeFrame::phoneme`] and (later) by
//! [`crate::avatar::AvatarFrame::mouth`].
//!
//! Producers: `claw-tts` (a stub in v0.1, a real engine later).
//! Consumers: `claw-avatar` (drives mouth animation), the audio
//! playback subsystem (when it lands; consumes the same stream
//! for synchronized audio scheduling).
//!
//! Topic strings live in [`tts_topics`] and follow the existing
//! `ordo.<area>.<thing>.<verb>` convention.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// ARPAbet-derived phoneme set. 39 standard ARPAbet phonemes
/// (15 vowels + 24 consonants) plus [`Phoneme::Rest`] for
/// silence.
///
/// Serialized in ARPAbet form (`"AA"`, `"CH"`, `"REST"`, etc.)
/// via `#[serde(rename_all = "UPPERCASE")]`. The mapping is
/// stable and matches CMU-dict + the bulk of public TTS
/// pipelines, so a stub here can be swapped for a real engine
/// later without a wire-format break.
///
/// The avatar driver maps `Phoneme` → 1-of-N visemes
/// (8 mouth-shape sprite cells in v0.1). That mapping is the
/// driver's concern, not the protocol's — the protocol carries
/// the raw phoneme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Phoneme {
    // Vowels (15)
    Aa,
    Ae,
    Ah,
    Ao,
    Aw,
    Ay,
    Eh,
    Er,
    Ey,
    Ih,
    Iy,
    Ow,
    Oy,
    Uh,
    Uw,
    // Consonants (24)
    B,
    Ch,
    D,
    Dh,
    F,
    G,
    Hh,
    Jh,
    K,
    L,
    M,
    N,
    Ng,
    P,
    R,
    S,
    Sh,
    T,
    Th,
    V,
    W,
    Y,
    Z,
    Zh,
    /// Silence between phonemes or between utterances. Renderers
    /// treat this as "closed mouth / neutral viseme."
    Rest,
}

/// Marks the start of a spoken utterance. Carries the source
/// text (so consumers can render captions or log what was said)
/// and an optional voice profile id (the TTS engine picks the
/// default voice when `None`).
///
/// Followed on the same `utterance_id` by zero-or-more
/// [`PhonemeFrame`]s and exactly one [`UtteranceEnd`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UtteranceStart {
    /// Unique id for this utterance. Echoed on every
    /// [`PhonemeFrame`] + the closing [`UtteranceEnd`] so
    /// consumers can route concurrent utterances correctly.
    pub utterance_id: Uuid,
    /// The text being spoken. Used for captions and logging;
    /// not for re-derivation of phonemes (those come over the
    /// wire pre-computed).
    pub text: String,
    /// Voice profile id; `None` = engine default. Shape kept
    /// loose for v0.1 — the real TTS engine added at step 7 of
    /// the build order may define a concrete voice catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
}

/// One phoneme event within an in-flight utterance. Timestamps
/// are utterance-relative; the consumer is responsible for
/// wall-clock alignment (typically by scheduling against the
/// audio subsystem's clock or its own `Instant::now()`).
///
/// `duration_ms` is the intended duration of this phoneme — the
/// driver uses it both to hold the corresponding viseme on
/// screen and to anticipate when to glide to the next mouth
/// shape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhonemeFrame {
    /// Identifies which utterance this phoneme belongs to —
    /// matches the `utterance_id` from the corresponding
    /// [`UtteranceStart`].
    pub utterance_id: Uuid,
    /// The phoneme itself. Renderers map this to a viseme cell.
    pub phoneme: Phoneme,
    /// Offset from the start of the utterance, in
    /// milliseconds. **NOT wall-clock.**
    pub timestamp_ms: u64,
    /// How long this phoneme is expected to last, in
    /// milliseconds.
    pub duration_ms: u32,
}

/// Closes an utterance. Consumers transition out of the
/// speaking state on this event even if some intermediate
/// [`PhonemeFrame`]s were lost.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UtteranceEnd {
    pub utterance_id: Uuid,
}

/// Bus topics for the TTS message stream. Producer:
/// `claw-tts`. Consumers: `claw-avatar`, audio playback (when
/// it lands), any future logging / caption subsystem.
pub mod tts_topics {
    pub const UTTERANCE_STARTED: &str = "ordo.tts.utterance.started";
    pub const PHONEME_FRAME: &str = "ordo.tts.phoneme.frame";
    pub const UTTERANCE_ENDED: &str = "ordo.tts.utterance.ended";
}
