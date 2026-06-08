//! Bus surface — capability descriptors + invoke dispatch.
//!
//! The runtime adapter (`ordo-mcp-host::LogicCapabilityAdapter`,
//! defined in that crate to keep the dependency direction
//! ordo-logic ← ordo-mcp-host) wraps a [`LogicProvider`] and uses
//! these helpers to emit descriptors for `/api/capabilities` and to
//! route incoming `/api/tools/logic.*` calls back to the provider.
//!
//! The descriptors are operator-facing: name, one-sentence
//! description, JSON schema for the input. The Skills tab reads
//! them live and surfaces logic.* alongside every other capability.

use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier};
use serde_json::{json, Value};

use crate::provider::LogicProvider;
use crate::types::LogicError;

pub const PROVIDER_NAME: &str = "ordo-logic";

pub const LOGIC_IDENTIFY_CLAIMS: &str = "logic.identify_claims";
pub const LOGIC_FIND_FALLACIES: &str = "logic.find_fallacies";
pub const LOGIC_VALIDATE_CHAIN: &str = "logic.validate_chain";
pub const LOGIC_STEEL_MAN: &str = "logic.steel_man";
pub const LOGIC_CLASSIFY_CLAIM_DOMAIN: &str = "logic.classify_claim_domain";

fn describe(cap: &str, description: &str, input_schema: Value) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        cap,
        PROVIDER_NAME,
        description,
        CapabilityTier::Optional,
        CapabilityActivation::Lazy,
    )
    .with_input_schema(input_schema)
}

pub fn capability_descriptors() -> Vec<CapabilityDescriptor> {
    vec![
        describe(
            LOGIC_IDENTIFY_CLAIMS,
            "List the explicit claims a passage makes — one per assertion, with confidence and verbatim support spans.",
            json!({
                "type": "object",
                "required": ["text"],
                "properties": {
                    "text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Prose to analyze."
                    }
                }
            }),
        ),
        describe(
            LOGIC_FIND_FALLACIES,
            "Inspect an argument for logical fallacies (ad hominem, straw man, false dichotomy, etc.). Empty list is a normal outcome.",
            json!({
                "type": "object",
                "required": ["argument"],
                "properties": {
                    "argument": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The argument text to analyze."
                    }
                }
            }),
        ),
        describe(
            LOGIC_VALIDATE_CHAIN,
            "Check whether a conclusion follows from the listed premises under standard inference rules. Returns gaps if the chain is incomplete.",
            json!({
                "type": "object",
                "required": ["premises", "conclusion"],
                "properties": {
                    "premises": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string" }
                    },
                    "conclusion": { "type": "string", "minLength": 1 }
                }
            }),
        ),
        describe(
            LOGIC_STEEL_MAN,
            "Return the strongest, most charitable version of an argument without changing the conclusion.",
            json!({
                "type": "object",
                "required": ["argument"],
                "properties": {
                    "argument": { "type": "string", "minLength": 1 }
                }
            }),
        ),
        describe(
            LOGIC_CLASSIFY_CLAIM_DOMAIN,
            "Classify a factual claim by domain (legal/medical/financial/safety/etc.) and stakes (low/medium/high), and decide whether the operator should require an authoritative source before acting on it. Pairs with the Grounding Floor system prompt rule to defend against semantic injection.",
            json!({
                "type": "object",
                "required": ["claim"],
                "properties": {
                    "claim": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The factual assertion to classify."
                    }
                }
            }),
        ),
    ]
}

/// Dispatch a `/api/tools/logic.*` call into the provider. Returns
/// `Ok(Some(value))` when the capability matched and ran,
/// `Ok(None)` when the capability isn't ours (lets the bus host fall
/// through to other providers), `Err(message)` when the capability
/// is ours but the call failed (input shape, LLM error, etc.).
pub async fn invoke_capability(
    provider: &dyn LogicProvider,
    capability: &str,
    arguments: &Value,
) -> Result<Option<Value>, String> {
    let result: Result<Value, LogicError> = match capability {
        LOGIC_IDENTIFY_CLAIMS => {
            let text = arguments
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| LogicError::InvalidArgument("missing `text`".into()));
            match text {
                Ok(text) => provider
                    .identify_claims(text)
                    .await
                    .map(|claims| json!({ "claims": claims })),
                Err(e) => Err(e),
            }
        }
        LOGIC_FIND_FALLACIES => {
            let argument = arguments
                .get("argument")
                .and_then(|v| v.as_str())
                .ok_or_else(|| LogicError::InvalidArgument("missing `argument`".into()));
            match argument {
                Ok(argument) => provider
                    .find_fallacies(argument)
                    .await
                    .map(|fallacies| json!({ "fallacies": fallacies })),
                Err(e) => Err(e),
            }
        }
        LOGIC_VALIDATE_CHAIN => {
            let premises: Vec<String> = arguments
                .get("premises")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let conclusion = arguments
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            provider
                .validate_chain(&premises, conclusion)
                .await
                .map(|cv| serde_json::to_value(cv).unwrap_or(Value::Null))
        }
        LOGIC_STEEL_MAN => {
            let argument = arguments
                .get("argument")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            provider
                .steel_man(argument)
                .await
                .map(|text| json!({ "steel_man": text }))
        }
        LOGIC_CLASSIFY_CLAIM_DOMAIN => {
            let claim = arguments
                .get("claim")
                .and_then(|v| v.as_str())
                .ok_or_else(|| LogicError::InvalidArgument("missing `claim`".into()));
            match claim {
                Ok(claim) => provider
                    .classify_claim_domain(claim)
                    .await
                    .map(|c| serde_json::to_value(c).unwrap_or(Value::Null)),
                Err(e) => Err(e),
            }
        }
        _ => return Ok(None),
    };
    result.map(Some).map_err(|err| err.to_string())
}
