//! Pluggable history summarizer (Follow-up 5).
//!
//! Context compaction in Phase 4.2 shipped as MECHANICAL: the
//! prompt builder rendered a short preamble of elided turn
//! previews. That's adequate for single-digit-turn sessions but
//! degrades past ~20 turns where the preview becomes a wall of
//! shorthand no reader (LLM or human) can parse.
//!
//! This module introduces a `Summarizer` trait. The `AssistantService`
//! can be wired with an impl — mechanical, LLM-backed, or something
//! else — that transforms the oldest-N turns into a coherent
//! "here's what happened earlier" summary.
//!
//! Two impls ship:
//!   - `MechanicalSummarizer` — the default; wraps the existing
//!     preamble generator. No network calls.
//!   - `ScriptedSummarizer` — test double; returns pre-configured
//!     strings. Used by golden tests to prove the wire-up works.
//!
//! An `LlmBackedSummarizer` (calling the same Anthropic / OpenAI
//! clients as the turn loop) is the natural next impl — intentionally
//! not in this crate so this module stays network-free. When an
//! operator opts into it, the runtime wiring plugs one in.

use async_trait::async_trait;

use crate::types::Turn;

#[derive(Debug, thiserror::Error)]
pub enum SummarizerError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("summary exceeded max output length: {0} chars")]
    TooLong(usize),
}

pub type SummarizerResult = Result<String, SummarizerError>;

#[async_trait]
pub trait Summarizer: Send + Sync {
    /// Produce a summary of the supplied turns. Returned string is
    /// inserted in place of the raw turns in the prompt as a
    /// system-message preamble. Implementations should keep the
    /// summary under the hinted byte budget; callers are not
    /// strictly required to truncate if they exceed it, but a too-
    /// long summary defeats the purpose.
    async fn summarize(&self, turns: &[Turn], target_chars: usize) -> SummarizerResult;
}

/// Default mechanical impl. Produces the same "earlier turns
/// elided" preamble the Phase 4.2 compaction already renders, but
/// with a stable signature so callers can swap impls without
/// changing the call site.
pub struct MechanicalSummarizer;

#[async_trait]
impl Summarizer for MechanicalSummarizer {
    async fn summarize(&self, turns: &[Turn], target_chars: usize) -> SummarizerResult {
        if turns.is_empty() {
            return Ok(String::new());
        }
        let per_turn = (target_chars / turns.len().max(1)).max(40);
        let mut out = String::with_capacity(target_chars);
        out.push_str(&format!(
            "# Earlier in this conversation ({} turn(s) elided)\n\n",
            turns.len()
        ));
        out.push_str("You asked / the operator said, in order:\n");
        for turn in turns {
            let preview: String = turn.user_message.chars().take(per_turn).collect();
            let truncated = turn.user_message.chars().count() > per_turn;
            out.push_str("- ");
            out.push_str(&preview);
            if truncated {
                out.push('\u{2026}');
            }
            out.push('\n');
        }
        Ok(out)
    }
}

/// Test double with a pre-configured summary. Ignores the input
/// turns — returns the configured string.
pub struct ScriptedSummarizer {
    pub summary: String,
}

impl ScriptedSummarizer {
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

#[async_trait]
impl Summarizer for ScriptedSummarizer {
    async fn summarize(&self, _turns: &[Turn], _target_chars: usize) -> SummarizerResult {
        Ok(self.summary.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TurnContext;
    use chrono::Utc;

    fn turn(user: &str) -> Turn {
        Turn {
            id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            index: 0,
            created_at: Utc::now(),
            user_message: user.into(),
            assistant_response: "ok".into(),
            context: TurnContext {
                facts: vec![],
                rag_hits: vec![],
                tool_calls: vec![],
                history_window: 0,
            },
            model: None,
            credential_service: None,
        }
    }

    #[tokio::test]
    async fn mechanical_emits_elided_preamble_for_non_empty_input() {
        let s = MechanicalSummarizer;
        let out = s
            .summarize(&[turn("first thing"), turn("second thing")], 500)
            .await
            .expect("summary");
        assert!(out.contains("2 turn(s) elided"));
        assert!(out.contains("first thing"));
        assert!(out.contains("second thing"));
    }

    #[tokio::test]
    async fn mechanical_empty_input_returns_empty_string() {
        let s = MechanicalSummarizer;
        let out = s.summarize(&[], 100).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn scripted_summarizer_returns_configured_string() {
        let s = ScriptedSummarizer::new("the operator is building a brand system");
        let out = s.summarize(&[turn("whatever")], 1000).await.unwrap();
        assert_eq!(out, "the operator is building a brand system");
    }
}
