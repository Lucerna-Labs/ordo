//! Pluggable LLM classifier abstraction.
//!
//! The router's Classify mode calls an LLM to rank tree nodes for a
//! query. The classifier is injected so:
//!   - tests use a scripted mock (`ScriptedClassifier`)
//!   - production wires in an LLM-backed impl (lives in the
//!     runtime wiring layer, not in this crate â€” keeps the router
//!     free of cloud deps)
//!
//! Replay fidelity (DPM): the classifier's output is CACHED onto
//! the routing decision event. Replay reads the cache, never calls
//! the classifier again. See the blueprint's "Replay fidelity for
//! classify mode" section.

use async_trait::async_trait;
use ordo_protocol::{ClassifierNodeChoice, TreeNode};

/// Output from a classifier call. Includes the `model` identifier
/// so replay can detect model drift.
#[derive(Debug, Clone)]
pub struct ClassifyOutput {
    pub model: String,
    pub nodes: Vec<ClassifierNodeChoice>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    #[error("classifier transport: {0}")]
    Transport(String),
    #[error("classifier returned malformed output: {0}")]
    Malformed(String),
}

#[async_trait]
pub trait Classifier: Send + Sync {
    async fn classify(
        &self,
        query: &str,
        tree: &[TreeNode],
    ) -> Result<ClassifyOutput, ClassifierError>;
}

/// Test helper: returns a pre-configured set of choices regardless
/// of input. Used for deterministic classifier tests.
pub struct ScriptedClassifier {
    pub model: String,
    pub choices: Vec<ClassifierNodeChoice>,
}

impl ScriptedClassifier {
    pub fn new(model: impl Into<String>, choices: Vec<(impl Into<String>, f32)>) -> Self {
        Self {
            model: model.into(),
            choices: choices
                .into_iter()
                .map(|(path, conf)| ClassifierNodeChoice {
                    path: path.into(),
                    confidence: conf,
                })
                .collect(),
        }
    }
}

#[async_trait]
impl Classifier for ScriptedClassifier {
    async fn classify(
        &self,
        _query: &str,
        _tree: &[TreeNode],
    ) -> Result<ClassifyOutput, ClassifierError> {
        Ok(ClassifyOutput {
            model: self.model.clone(),
            nodes: self.choices.clone(),
        })
    }
}
