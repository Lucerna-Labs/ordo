//! `HybridLogicProvider` — composes LLM + propositional prover.
//!
//! The architectural promise of `ordo-logic`: when the question is
//! formalizable, return a deterministic answer with `Certainty::Formal`;
//! when it isn't, return the LLM's rhetorical read with
//! `Certainty::Rhetorical`. Operators (and the planner) read the
//! certainty tier to decide whether to cite proof or argue.
//!
//! Implementation strategy for `validate_chain`:
//!
//!   1. Ask the LLM to formalize the premises + conclusion into
//!      propositional expressions. The LLM is explicitly allowed —
//!      and encouraged — to refuse if the argument is normative,
//!      modal, or otherwise not propositional.
//!   2. If formalization succeeded AND every formula parses cleanly,
//!      run the truth-table prover. The prover gives a deterministic
//!      `holds: bool` plus an optional counterexample (variable
//!      assignment that breaks the chain).
//!      - holds=true   → certainty=Formal, gaps=[], notes describe vocab
//!      - holds=false  → certainty=Formal, gaps include counterexample
//!   3. If formalization failed (or the prover bailed with
//!      `TooLarge`), fall through to the inner provider's
//!      `validate_chain` and tag the result `Rhetorical`.
//!
//! The non-validate_chain methods (identify_claims, find_fallacies,
//! steel_man) just pass through — they're inherently rhetorical and
//! get no benefit from the prover.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use crate::fol::{self, FolError, FolExpr};
use crate::propositional::{self, Expr, ProverError};
use crate::provider::LogicProvider;
use crate::types::{
    Certainty, ChainValidation, Claim, ClaimClassification, Fallacy, LogicError, LogicResult,
};

/// Wraps any `LogicProvider` (typically `LlmLogicProvider`) and adds
/// a formal-verification path on top. The inner provider handles
/// formalization (it has the LLM); this layer parses the formal
/// output, runs the propositional prover, and tags certainty.
pub struct HybridLogicProvider {
    inner: Arc<dyn LogicProvider>,
    /// Hook the formalize call into the inner LLM. Stored as a
    /// closure so we can inject in tests without spinning up a real
    /// cloud client. Returns the raw LLM JSON (the FormalizationOutput
    /// shape below).
    formalize: FormalizeFn,
}

/// Callback shape for the formalize step. Takes premises + conclusion
/// (plain English), returns the LLM's structured response or an
/// error. In production this delegates to `LlmLogicProvider`'s
/// chat-completion path; in tests it's a fixed stub.
pub type FormalizeFn = Arc<
    dyn Fn(Vec<String>, String) -> futures_box::BoxFuture<'static, LogicResult<FormalizationOutput>>
        + Send
        + Sync,
>;

// We don't take a futures dep just for BoxFuture; build our own
// minimal alias inline.
mod futures_box {
    use std::future::Future;
    use std::pin::Pin;
    pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
}

#[derive(Debug, Clone, Deserialize)]
pub struct FormalizationOutput {
    pub formalizable: bool,
    /// Which formal layer the LLM picked. "fol" → use the FOL parser +
    /// grounder; "propositional" → use the propositional parser
    /// directly. Default to "fol" when missing — FOL parses
    /// propositional inputs cleanly (a propositional formula is
    /// just FOL with zero quantifiers and zero-arity predicates),
    /// so this is a safe fallback.
    #[serde(default = "default_layer")]
    pub layer: FormalLayer,
    #[serde(default)]
    pub premises: Vec<String>,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub vocabulary: serde_json::Map<String, Value>,
    #[serde(default)]
    pub reason_if_not: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FormalLayer {
    Fol,
    Propositional,
}

fn default_layer() -> FormalLayer {
    FormalLayer::Fol
}

impl HybridLogicProvider {
    /// Build a hybrid provider whose formal path delegates to the
    /// supplied closure. The "default" wiring (used by the runtime)
    /// is in `wire_default` below — it builds the closure to call
    /// the inner LLM via the same `one_shot` path other capabilities
    /// use.
    pub fn new(inner: Arc<dyn LogicProvider>, formalize: FormalizeFn) -> Self {
        Self { inner, formalize }
    }
}

/// What `parse_formal` produced — either a propositional pair
/// ready for the truth-table prover, or an FOL pair we'll ground
/// inside `validate_chain`.
enum ParsedFormal {
    Propositional(Vec<Expr>, Expr),
    Fol(Vec<FolExpr>, FolExpr),
}

/// Parse a `FormalizationOutput` into typed expressions on the
/// requested layer. Returns an explanatory string on parse failure
/// so the caller can stitch it into `notes`.
fn parse_formal(output: &FormalizationOutput) -> Result<ParsedFormal, String> {
    match output.layer {
        FormalLayer::Propositional => {
            let mut premises = Vec::with_capacity(output.premises.len());
            for (i, raw) in output.premises.iter().enumerate() {
                let expr = propositional::parse(raw).map_err(|e| {
                    format!(
                        "premise {} did not parse as propositional ({raw:?}): {e}",
                        i + 1
                    )
                })?;
                premises.push(expr);
            }
            let conclusion = propositional::parse(&output.conclusion).map_err(|e| {
                format!(
                    "conclusion did not parse as propositional ({:?}): {e}",
                    output.conclusion
                )
            })?;
            Ok(ParsedFormal::Propositional(premises, conclusion))
        }
        FormalLayer::Fol => {
            let mut premises = Vec::with_capacity(output.premises.len());
            for (i, raw) in output.premises.iter().enumerate() {
                let expr = fol::parse(raw).map_err(|e| {
                    format!("premise {} did not parse as FOL ({raw:?}): {e}", i + 1)
                })?;
                premises.push(expr);
            }
            let conclusion = fol::parse(&output.conclusion).map_err(|e| {
                format!(
                    "conclusion did not parse as FOL ({:?}): {e}",
                    output.conclusion
                )
            })?;
            Ok(ParsedFormal::Fol(premises, conclusion))
        }
    }
}

fn vocabulary_notes(output: &FormalizationOutput) -> Vec<String> {
    output
        .vocabulary
        .iter()
        .map(|(k, v)| {
            let meaning = v.as_str().unwrap_or("(no meaning given)");
            format!("{k} = {meaning}")
        })
        .collect()
}

fn render_counterexample(counter: &std::collections::BTreeMap<String, bool>) -> String {
    if counter.is_empty() {
        return "vacuously falsified".into();
    }
    let parts: Vec<String> = counter.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("counterexample: {}", parts.join(", "))
}

#[async_trait]
impl LogicProvider for HybridLogicProvider {
    async fn identify_claims(&self, text: &str) -> LogicResult<Vec<Claim>> {
        // Pure-LLM — no formal path adds signal here.
        self.inner.identify_claims(text).await
    }

    async fn find_fallacies(&self, argument: &str) -> LogicResult<Vec<Fallacy>> {
        // Pure-LLM — fallacies are rhetoric problems by definition.
        self.inner.find_fallacies(argument).await
    }

    async fn validate_chain(
        &self,
        premises: &[String],
        conclusion: &str,
    ) -> LogicResult<ChainValidation> {
        // Step 1: ask the LLM to formalize.
        let formal = (self.formalize)(premises.to_vec(), conclusion.to_string()).await;

        // If the formalize call itself failed (LLM error, parse error
        // on the formalize JSON), don't fail the whole capability —
        // just fall back to the inner LLM's rhetorical chain check.
        let formal = match formal {
            Ok(f) => f,
            Err(err) => {
                debug!(
                    target: "ordo_logic",
                    error = %err,
                    "formalization step failed, falling back to rhetorical validation"
                );
                let mut rhetorical = self.inner.validate_chain(premises, conclusion).await?;
                rhetorical.certainty = Certainty::Rhetorical;
                rhetorical
                    .notes
                    .insert(0, format!("formalization unavailable: {err}"));
                return Ok(rhetorical);
            }
        };

        if !formal.formalizable {
            // The LLM explicitly punted on formalization — argument
            // is normative, modal, vague, etc. Get a rhetorical read
            // and tag it accordingly.
            let mut rhetorical = self.inner.validate_chain(premises, conclusion).await?;
            rhetorical.certainty = Certainty::Rhetorical;
            if !formal.reason_if_not.trim().is_empty() {
                rhetorical
                    .notes
                    .insert(0, format!("not formalizable: {}", formal.reason_if_not));
            }
            return Ok(rhetorical);
        }

        // Step 2: parse the formal output and run the prover.
        let parsed = match parse_formal(&formal) {
            Ok(p) => p,
            Err(reason) => {
                // The LLM said it was formalizable but emitted
                // un-parseable formulas. Treat as rhetorical with a
                // note so the operator can see what went wrong.
                let mut rhetorical = self.inner.validate_chain(premises, conclusion).await?;
                rhetorical.certainty = Certainty::Rhetorical;
                rhetorical
                    .notes
                    .insert(0, format!("formalization parse failed — {reason}"));
                return Ok(rhetorical);
            }
        };

        let mut notes = vocabulary_notes(&formal);

        // Branch on the layer the LLM picked. FOL grounds first into
        // a propositional formula; propositional dispatches directly.
        // Both layers funnel into the same truth-table prover, so the
        // counterexample shape stays consistent.
        let (holds, counterexample, summary) = match parsed {
            ParsedFormal::Propositional(prems, concl) => {
                match propositional::entails(&prems, &concl) {
                    Ok(r) => (
                        r.holds,
                        r.counterexample,
                        format!(
                            "verified by truth-table over {} propositional assignment{}",
                            r.assignments_checked,
                            if r.assignments_checked == 1 { "" } else { "s" }
                        ),
                    ),
                    Err(ProverError::TooLarge { vars }) => {
                        return self
                            .fallback_too_large(premises, conclusion, vars, "propositional")
                            .await;
                    }
                    Err(other) => {
                        return self
                            .fallback_prover_error(premises, conclusion, &other)
                            .await;
                    }
                }
            }
            ParsedFormal::Fol(prems, concl) => {
                match fol::entails(&prems, &concl) {
                    Ok(r) => (
                        r.holds,
                        r.counterexample,
                        format!(
                            "verified by truth-table over FOL grounding: \
                             domain {{{}}}, {} atomic ground predicate{}",
                            r.domain.join(", "),
                            r.atom_count,
                            if r.atom_count == 1 { "" } else { "s" }
                        ),
                    ),
                    Err(FolError::DomainTooLarge { size }) => {
                        return self
                            .fallback_too_large(premises, conclusion, size, "FOL domain")
                            .await;
                    }
                    Err(FolError::TooManyAtoms { atoms }) => {
                        return self
                            .fallback_too_large(premises, conclusion, atoms, "FOL atoms")
                            .await;
                    }
                    Err(FolError::Propositional(propositional::ProverError::TooLarge { vars })) => {
                        return self
                            .fallback_too_large(premises, conclusion, vars, "FOL→propositional")
                            .await;
                    }
                    Err(other) => {
                        // Generic FOL error (parse, unbound var) — render
                        // as a note and fall back to rhetorical so the
                        // operator sees what went wrong without a hard
                        // failure.
                        let mut rhetorical =
                            self.inner.validate_chain(premises, conclusion).await?;
                        rhetorical.certainty = Certainty::Rhetorical;
                        rhetorical
                            .notes
                            .insert(0, format!("FOL grounding error: {other}"));
                        return Ok(rhetorical);
                    }
                }
            }
        };

        let mut gaps = Vec::new();
        if !holds {
            if let Some(cx) = &counterexample {
                gaps.push(render_counterexample(cx));
            }
        }
        notes.insert(0, summary);

        Ok(ChainValidation {
            holds,
            gaps,
            notes,
            certainty: Certainty::Formal,
        })
    }

    async fn steel_man(&self, argument: &str) -> LogicResult<String> {
        // Pure-LLM — generative.
        self.inner.steel_man(argument).await
    }

    async fn classify_claim_domain(&self, claim: &str) -> LogicResult<ClaimClassification> {
        // Pure-LLM — classification, not formal verification.
        self.inner.classify_claim_domain(claim).await
    }
}

impl HybridLogicProvider {
    /// Fallback path when a formal layer reports the problem is too
    /// big for the in-runtime prover. Produces a rhetorical answer
    /// and surfaces the bound in the note so the operator knows
    /// `logic-mcp` is the install lever.
    async fn fallback_too_large(
        &self,
        premises: &[String],
        conclusion: &str,
        size: usize,
        kind: &str,
    ) -> LogicResult<ChainValidation> {
        let mut rhetorical = self.inner.validate_chain(premises, conclusion).await?;
        rhetorical.certainty = Certainty::Rhetorical;
        rhetorical.notes.insert(
            0,
            format!(
                "{kind} layer hit the in-runtime cap ({size} units); rhetorical fallback. \
                 Install logic-mcp for unbounded SAT/SMT."
            ),
        );
        Ok(rhetorical)
    }

    async fn fallback_prover_error(
        &self,
        premises: &[String],
        conclusion: &str,
        err: &ProverError,
    ) -> LogicResult<ChainValidation> {
        let mut rhetorical = self.inner.validate_chain(premises, conclusion).await?;
        rhetorical.certainty = Certainty::Rhetorical;
        rhetorical.notes.insert(0, format!("prover error: {err}"));
        Ok(rhetorical)
    }
}

// ─── Default wiring ──────────────────────────────────────────────
//
// `wire_default` builds a `HybridLogicProvider` whose formalize step
// reuses the `LlmLogicProvider`'s LLM client. This is the function
// the runtime calls — it produces a single Arc<dyn LogicProvider>
// that fronts the entire stack.

use ordo_cloud::{CloudCredentialTask, CloudHttp};
use serde_json::json;

/// Build the canonical hybrid provider for the runtime. Uses the
/// supplied http client + credential task for both the inner
/// LLM-backed methods AND the formalize step. One credential walker,
/// one timeout policy, one set of audit events.
pub fn wire_default(
    http: Arc<CloudHttp>,
    credentials: CloudCredentialTask,
) -> Arc<dyn LogicProvider> {
    let llm = Arc::new(crate::llm::LlmLogicProvider::new(
        http.clone(),
        credentials.clone(),
    ));
    // Formalize closure: runs a one-shot chat call against the same
    // LLM the inner provider uses, with the formalize prompt. We
    // duplicate a tiny bit of the LLM call logic here rather than
    // exposing it on the trait — the formalize call is implementation
    // detail, not part of the public LogicProvider surface.
    let formalize_http = http;
    let formalize_creds = credentials;
    let formalize: FormalizeFn = Arc::new(move |premises, conclusion| {
        let http = formalize_http.clone();
        let creds = formalize_creds.clone();
        Box::pin(async move {
            // Same credential walk as LlmLogicProvider::pick_credential
            let cred = crate::llm::pick_credential_public(&creds).await?;
            let prompt = crate::prompts::formalize_chain(&premises, &conclusion);
            let mut chat_args = json!({
                "messages": [{ "role": "user", "content": prompt }],
                "temperature": 0.1,
                "max_tokens": 2048,
            });
            if let Some(model) = cred.extras.get("model") {
                chat_args["model"] = json!(model);
            }
            let is_anthropic = cred.auth_style == "anthropic";
            let response = if is_anthropic {
                ordo_cloud::anthropic::messages(&http, &cred, &chat_args)
                    .await
                    .map_err(|err| LogicError::LlmFailed(err.to_string()))?
            } else {
                ordo_cloud::openai::chat(&http, &cred, &chat_args)
                    .await
                    .map_err(|err| LogicError::LlmFailed(err.to_string()))?
            };
            let raw = response
                .get("content_raw")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| response.get("assistant_message").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let json_slice = crate::llm::extract_json_public(&raw);
            serde_json::from_str::<FormalizationOutput>(json_slice).map_err(|err| {
                let snippet: String = json_slice.chars().take(240).collect();
                LogicError::LlmFailed(format!(
                    "could not parse formalize JSON ({err}); snippet: {snippet}"
                ))
            })
        })
    });
    Arc::new(HybridLogicProvider::new(llm, formalize))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Stub inner provider for tests — every method returns an empty
    // rhetorical answer so we can verify the hybrid layer's behavior
    // without standing up a real LLM.
    struct StubInner;
    #[async_trait]
    impl LogicProvider for StubInner {
        async fn identify_claims(&self, _text: &str) -> LogicResult<Vec<Claim>> {
            Ok(Vec::new())
        }
        async fn find_fallacies(&self, _argument: &str) -> LogicResult<Vec<Fallacy>> {
            Ok(Vec::new())
        }
        async fn validate_chain(
            &self,
            _premises: &[String],
            _conclusion: &str,
        ) -> LogicResult<ChainValidation> {
            Ok(ChainValidation {
                holds: false,
                gaps: vec!["stub-rhetorical-gap".into()],
                notes: vec!["stub-rhetorical-note".into()],
                certainty: Certainty::Rhetorical,
            })
        }
        async fn steel_man(&self, _argument: &str) -> LogicResult<String> {
            Ok(String::new())
        }
        async fn classify_claim_domain(&self, _claim: &str) -> LogicResult<ClaimClassification> {
            Ok(ClaimClassification {
                domains: vec!["general".into()],
                stakes: crate::types::ClaimStakes::Low,
                requires_authoritative_source: false,
                rationale: "stub".into(),
            })
        }
    }

    fn fixed_formalize(
        formalizable: bool,
        layer: FormalLayer,
        formal_premises: Vec<&str>,
        formal_conclusion: &str,
        reason: &str,
    ) -> FormalizeFn {
        let p = formal_premises
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let c = formal_conclusion.to_string();
        let r = reason.to_string();
        Arc::new(move |_pre, _con| {
            let p = p.clone();
            let c = c.clone();
            let r = r.clone();
            Box::pin(async move {
                Ok(FormalizationOutput {
                    formalizable,
                    layer,
                    premises: p,
                    conclusion: c,
                    vocabulary: serde_json::Map::new(),
                    reason_if_not: r,
                })
            })
        })
    }

    #[tokio::test]
    async fn formal_path_proves_modus_ponens() {
        let inner: Arc<dyn LogicProvider> = Arc::new(StubInner);
        let hybrid = HybridLogicProvider::new(
            inner,
            fixed_formalize(
                true,
                FormalLayer::Propositional,
                vec!["p", "p -> q"],
                "q",
                "",
            ),
        );
        let r = hybrid
            .validate_chain(
                &[
                    "it's raining".into(),
                    "if it's raining then ground is wet".into(),
                ],
                "ground is wet",
            )
            .await
            .expect("validate");
        assert!(r.holds);
        assert_eq!(r.certainty, Certainty::Formal);
        assert!(r.gaps.is_empty());
        assert!(r.notes.iter().any(|n| n.contains("truth-table")));
    }

    #[tokio::test]
    async fn formal_path_rejects_invalid_chain_with_counterexample() {
        // Affirming the consequent — should be rejected.
        let inner: Arc<dyn LogicProvider> = Arc::new(StubInner);
        let hybrid = HybridLogicProvider::new(
            inner,
            fixed_formalize(
                true,
                FormalLayer::Propositional,
                vec!["p -> q", "q"],
                "p",
                "",
            ),
        );
        let r = hybrid
            .validate_chain(&["x".into(), "y".into()], "z")
            .await
            .expect("validate");
        assert!(!r.holds);
        assert_eq!(r.certainty, Certainty::Formal);
        assert!(r.gaps.iter().any(|g| g.starts_with("counterexample")));
    }

    #[tokio::test]
    async fn rhetorical_fallback_when_not_formalizable() {
        // LLM punts; hybrid passes through to inner with Rhetorical tag.
        let inner: Arc<dyn LogicProvider> = Arc::new(StubInner);
        let hybrid = HybridLogicProvider::new(
            inner,
            fixed_formalize(
                false,
                FormalLayer::Propositional,
                vec![],
                "",
                "argument is normative",
            ),
        );
        let r = hybrid
            .validate_chain(&["a".into()], "b")
            .await
            .expect("validate");
        // Inner stub returned holds=false; hybrid preserves that and
        // tags certainty as Rhetorical.
        assert!(!r.holds);
        assert_eq!(r.certainty, Certainty::Rhetorical);
        assert!(r.notes.iter().any(|n| n.contains("not formalizable")));
        // Stub's gap should still be there too.
        assert!(r.gaps.iter().any(|g| g == "stub-rhetorical-gap"));
    }

    #[tokio::test]
    async fn fol_path_proves_penguin_syllogism_through_hybrid() {
        // The motivating example: "All birds have feathers, penguins
        // are birds, therefore penguins have feathers." Propositional
        // alone can't formalize this (needs ∀); the FOL path through
        // the hybrid provider should now prove it Formal.
        let inner: Arc<dyn LogicProvider> = Arc::new(StubInner);
        let hybrid = HybridLogicProvider::new(
            inner,
            fixed_formalize(
                true,
                FormalLayer::Fol,
                vec!["forall x. Bird(x) -> Feathered(x)", "Bird(penguin)"],
                "Feathered(penguin)",
                "",
            ),
        );
        let r = hybrid
            .validate_chain(
                &[
                    "All birds have feathers".into(),
                    "Penguins are birds".into(),
                ],
                "Penguins have feathers",
            )
            .await
            .expect("validate");
        assert!(r.holds, "penguin syllogism should prove formally");
        assert_eq!(r.certainty, Certainty::Formal);
        assert!(r.gaps.is_empty());
        // The FOL grounding summary should mention the domain.
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("FOL grounding") && n.contains("penguin")),
            "expected FOL summary mentioning penguin domain in notes: {:?}",
            r.notes
        );
    }

    #[tokio::test]
    async fn rhetorical_fallback_on_formalize_parse_error() {
        // LLM said formalizable=true but emitted garbage — fall back.
        let inner: Arc<dyn LogicProvider> = Arc::new(StubInner);
        let hybrid = HybridLogicProvider::new(
            inner,
            fixed_formalize(
                true,
                FormalLayer::Propositional,
                vec!["this is not propositional!!!"],
                "neither",
                "",
            ),
        );
        let r = hybrid
            .validate_chain(&["a".into()], "b")
            .await
            .expect("validate");
        assert_eq!(r.certainty, Certainty::Rhetorical);
        assert!(r
            .notes
            .iter()
            .any(|n| n.contains("formalization parse failed")));
    }
}
