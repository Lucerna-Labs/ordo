//! Adversarial quality gate (Stage 4).
//!
//! Judges a subagent's output against its subtask and emits a
//! [`TaskVerdict`] (`Pass` / `Revise` / `Fail`, defined in `ordo-protocol`
//! at Stage 0). Two tiers:
//!
//! 1. **Deterministic** (cheap, always-on): structural checks that reject
//!    empty output, placeholders, and refusals WITHOUT a model call.
//! 2. **LLM critic** (optional): an adversarial reviewer spawned as a
//!    scoped subagent (the Critic profile), prompted to *refute*. The CALL
//!    is the production [`Critic`] impl (Stage 5 glue); the prompt builder
//!    and response parser here are pure + unit-tested.
//!
//! The driver loop (Stage 5) acts on the verdict: `Pass` accepts, `Revise`
//! re-dispatches the subtask with the feedback (bounded), `Fail` halts it.

use ordo_protocol::TaskVerdict;
use serde::Deserialize;

use crate::dispatch::Subtask;
use crate::plan::extract_json;

/// Structural checks that reject an obviously bad output without a model
/// call. Returns `Some(verdict)` to reject (always `Revise` — these are
/// recoverable: ask the subagent again), or `None` if the output clears
/// the deterministic floor and is eligible for the (optional) critic.
pub fn deterministic_check(output: &str) -> Option<TaskVerdict> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Some(TaskVerdict::Revise {
            feedback: "produced no output; attempt the task and return a concrete answer".into(),
        });
    }
    let lowered = trimmed.to_lowercase();

    // The WHOLE output is a placeholder non-answer (exact match, so valid
    // content that merely mentions "todo" isn't rejected).
    const PLACEHOLDERS: &[&str] = &["...", "tbd", "n/a", "na", "todo", "lorem ipsum"];
    if PLACEHOLDERS.contains(&lowered.as_str()) {
        return Some(TaskVerdict::Revise {
            feedback: "output is a placeholder, not an answer; complete the task".into(),
        });
    }

    // Refusal / non-attempt.
    const REFUSALS: &[&str] = &[
        "i cannot",
        "i can't",
        "i am unable",
        "i'm unable",
        "as an ai",
        "as a language model",
    ];
    if REFUSALS.iter().any(|marker| lowered.contains(marker)) {
        return Some(TaskVerdict::Revise {
            feedback: "the subagent refused or did not attempt the task; retry with a direct attempt"
                .into(),
        });
    }

    None
}

/// Adversarially judges whether a subagent's `output` satisfies its
/// `subtask`. Implemented by a glue layer that spawns the Critic profile
/// as a scoped subagent (Stage 5); the orchestrator depends only on this
/// trait so the gate is testable with a stub.
#[async_trait::async_trait]
pub trait Critic: Send + Sync {
    async fn critique(&self, subtask: &Subtask, output: &str) -> TaskVerdict;
}

/// Run the gate: deterministic floor first (cheap reject, no model call);
/// if it clears and a `critic` is configured, defer to the critic;
/// otherwise `Pass`.
pub async fn verify(
    subtask: &Subtask,
    output: &str,
    critic: Option<&dyn Critic>,
) -> TaskVerdict {
    if let Some(verdict) = deterministic_check(output) {
        return verdict;
    }
    match critic {
        Some(critic) => critic.critique(subtask, output).await,
        None => TaskVerdict::Pass {
            evidence: "passed deterministic checks (no critic configured)".into(),
        },
    }
}

/// Build the adversarial critic prompt. Asks for STRICT JSON and tells the
/// model to default to rejecting when not confident.
pub fn critic_prompt(task_goal: &str, output: &str) -> String {
    format!(
        "You are an ADVERSARIAL reviewer. Decide whether the OUTPUT genuinely and correctly \
         satisfies the TASK. Be skeptical: look for unmet requirements, errors, fabrication, or \
         non-answers. Default to rejecting if you are not confident it is correct AND complete.\n\n\
         Return STRICT JSON only — no prose, no code fences:\n\
         {{\"verdict\":\"pass|revise|fail\",\"detail\":\"<evidence, or what to fix>\"}}\n\
         - pass:   the output correctly and completely satisfies the task.\n\
         - revise: close, with a fixable gap (detail = exactly what to fix).\n\
         - fail:   wrong, fabricated, or off-task (detail = why).\n\n\
         TASK:\n{task_goal}\n\nOUTPUT:\n{output}"
    )
}

/// Parse a critic model response into a [`TaskVerdict`]. Permissive (strips
/// fences/prose). An UNPARSEABLE critic response does not block output that
/// already cleared the deterministic floor — it is treated as an
/// inconclusive `Pass` (the critic is additive, not the floor).
pub fn parse_critic_verdict(raw: &str) -> TaskVerdict {
    try_parse_verdict(raw).unwrap_or(TaskVerdict::Pass {
        evidence: "critic response was inconclusive".into(),
    })
}

#[derive(Deserialize)]
struct CriticResponse {
    verdict: String,
    #[serde(default)]
    detail: String,
}

fn try_parse_verdict(raw: &str) -> Option<TaskVerdict> {
    let parsed: CriticResponse = serde_json::from_str(extract_json(raw)).ok()?;
    let detail = parsed.detail.trim().to_string();
    let with_default = |fallback: &str| {
        if detail.is_empty() {
            fallback.to_string()
        } else {
            detail.clone()
        }
    };
    match parsed.verdict.trim().to_lowercase().as_str() {
        "pass" => Some(TaskVerdict::Pass {
            evidence: with_default("critic approved"),
        }),
        "revise" => Some(TaskVerdict::Revise {
            feedback: with_default("critic asked for a revision (no detail given)"),
        }),
        "fail" => Some(TaskVerdict::Fail {
            reason: with_default("critic rejected the output (no detail given)"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn subtask() -> Subtask {
        Subtask::new("do the thing", None)
    }

    #[test]
    fn deterministic_rejects_empty_placeholder_and_refusal() {
        assert!(matches!(
            deterministic_check(""),
            Some(TaskVerdict::Revise { .. })
        ));
        assert!(matches!(
            deterministic_check("  ...  "),
            Some(TaskVerdict::Revise { .. })
        ));
        assert!(matches!(
            deterministic_check("As an AI language model, I cannot do that."),
            Some(TaskVerdict::Revise { .. })
        ));
    }

    #[test]
    fn deterministic_passes_a_real_answer() {
        // Mentions "todo" inside real content — must NOT be rejected.
        assert!(deterministic_check("Add a TODO comment above the handler and wire the route.").is_none());
    }

    /// Records whether `critique` was called, so we can assert the
    /// deterministic floor short-circuits before the (expensive) critic.
    struct SpyCritic {
        called: AtomicBool,
        verdict: TaskVerdict,
    }

    #[async_trait::async_trait]
    impl Critic for SpyCritic {
        async fn critique(&self, _subtask: &Subtask, _output: &str) -> TaskVerdict {
            self.called.store(true, Ordering::SeqCst);
            self.verdict.clone()
        }
    }

    #[tokio::test]
    async fn verify_short_circuits_before_critic_on_deterministic_reject() {
        let spy = SpyCritic {
            called: AtomicBool::new(false),
            verdict: TaskVerdict::Pass { evidence: "x".into() },
        };
        let verdict = verify(&subtask(), "", Some(&spy)).await;
        assert!(matches!(verdict, TaskVerdict::Revise { .. }));
        assert!(!spy.called.load(Ordering::SeqCst), "critic must not run after a deterministic reject");
    }

    #[tokio::test]
    async fn verify_defers_to_critic_when_floor_passes() {
        let spy = SpyCritic {
            called: AtomicBool::new(false),
            verdict: TaskVerdict::Fail { reason: "fabricated".into() },
        };
        let verdict = verify(&subtask(), "a plausible but wrong answer", Some(&spy)).await;
        assert!(matches!(verdict, TaskVerdict::Fail { .. }));
        assert!(spy.called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn verify_passes_when_floor_clears_and_no_critic() {
        let verdict = verify(&subtask(), "a complete, correct answer", None).await;
        assert!(matches!(verdict, TaskVerdict::Pass { .. }));
    }

    #[test]
    fn parses_each_critic_verdict() {
        assert!(matches!(
            parse_critic_verdict(r#"{"verdict":"pass","detail":"ok"}"#),
            TaskVerdict::Pass { .. }
        ));
        assert!(matches!(
            parse_critic_verdict("```json\n{\"verdict\":\"fail\",\"detail\":\"off-task\"}\n```"),
            TaskVerdict::Fail { .. }
        ));
        assert!(matches!(
            parse_critic_verdict(r#"{"verdict":"revise","detail":"fix the edge case"}"#),
            TaskVerdict::Revise { .. }
        ));
    }

    #[test]
    fn unparseable_critic_response_is_inconclusive_pass() {
        // Additive critic: a flaky/garbled response must not fail output
        // that already cleared the deterministic floor.
        assert!(matches!(
            parse_critic_verdict("the model rambled with no json"),
            TaskVerdict::Pass { .. }
        ));
    }
}
