//! ordo-mcp-client â€” invocation orchestration, DPoP, privilege
//! tiering, capability attenuation.
//!
//! Responsibility boundary: this crate owns the *outer pipeline*
//! that composes the MCP stack. For every tool invocation it:
//!
//!   1. Consults the registry for trust state + drift.
//!   2. Enforces ToolRiskLevel vs ServerTrustState minimums.
//!   3. Acquires a DPoP proof (single-use nonce enforced here).
//!   4. Dispatches to the sandbox.
//!   5. Routes the raw response through the Worker for extraction.
//!   6. Emits `McpClientInvokeResult` with the Worker-extracted
//!      data for the Planner to consume.
//!
//! Crate does NOT own sandbox execution, Worker extraction,
//! registry persistence â€” each is its own responsibility.
//!
//! Load-bearing commitments (blueprint Â§25, Â§27, Â§34, invariants
//! 25 + 27 + 34):
//!
//! - Raw tool responses never return to the Planner â€” Worker
//!   extraction is mandatory on every path.
//! - Every returned envelope carries an explicit privilege tier.
//! - DPoP nonces are single-use; a replay attempt returns
//!   `ClientError::DpopReplay`.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ordo_bus::Bus;
use ordo_mcp_provenance::ProvenanceService;
use ordo_mcp_registry::{AnomalySeverity, McpRegistryService, RegistryError};
use ordo_mcp_sandbox::{McpSandboxService, SandboxError};
use ordo_mcp_worker::WorkerPool;
use ordo_protocol::{
    mcp_topics, BusEnvelope, Envelope,
    McpExtractionError, NodeId, OrdoMessage, PrivilegeTier, ProvenanceCheckRequest,
    ServerTrustState, ToolRiskLevel, ToolSchema,
};
use serde::Serialize;
use serde_json::Value;

pub mod attenuation;
pub mod dpop;

pub use attenuation::attenuate_capability;
pub use dpop::{DpopIssuer, DpopLedger, DpopLedgerError};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("registry: {0}")]
    Registry(#[from] RegistryError),
    #[error("sandbox: {0}")]
    Sandbox(#[from] SandboxError),
    #[error("extraction: {0:?}")]
    Extraction(McpExtractionError),
    #[error("server not installed: {0}")]
    ServerNotInstalled(String),
    #[error("tool not in catalog: {0}")]
    UnknownTool(String),
    #[error("tool risk level {risk:?} requires server trust state {required:?} or higher; current: {current:?}")]
    TrustGateFailed {
        risk: ToolRiskLevel,
        required: ServerTrustState,
        current: ServerTrustState,
    },
    #[error("dpop replay: {0}")]
    DpopReplay(String),
    #[error("dpop ledger: {0}")]
    DpopLedger(#[from] DpopLedgerError),
    #[error("capability widening attempted")]
    CapabilityWidening,
    #[error("provenance blocked: {0}")]
    ProvenanceBlocked(String),
    #[error("bad input: {0}")]
    BadInput(String),
}

pub type ClientResult<T> = Result<T, ClientError>;

/// A successful invocation result. Carries the Worker-extracted
/// data + the privilege tier the Planner should treat it as +
/// the sanitization node id so downstream provenance queries
/// resolve correctly.
#[derive(Debug, Clone)]
pub struct InvocationResult {
    pub extracted_data: Value,
    pub privilege_tier: PrivilegeTier,
    pub sanitization_node_id: String,
}

pub struct McpClientService {
    registry: Arc<McpRegistryService>,
    sandbox: Arc<McpSandboxService>,
    worker_pool: Arc<WorkerPool>,
    provenance: Option<Arc<ProvenanceService>>,
    dpop_issuer: DpopIssuer,
    dpop_ledger: Arc<DpopLedger>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

impl McpClientService {
    pub fn new(
        registry: Arc<McpRegistryService>,
        sandbox: Arc<McpSandboxService>,
        worker_pool: Arc<WorkerPool>,
        signing_key: SigningKey,
    ) -> Self {
        let dpop_issuer = DpopIssuer::new(signing_key);
        Self {
            registry,
            sandbox,
            worker_pool,
            provenance: None,
            dpop_issuer,
            dpop_ledger: Arc::new(DpopLedger::default()),
            bus: None,
            node_id: NodeId::new(),
        }
    }

    pub fn with_provenance(mut self, provenance: Arc<ProvenanceService>) -> Self {
        self.provenance = Some(provenance);
        self
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }

    pub fn dpop_issuer(&self) -> &DpopIssuer {
        &self.dpop_issuer
    }

    /// The central pipeline. Planner calls this; the Planner never
    /// sees the raw response, only the Worker-extracted data.
    pub async fn invoke(
        &self,
        server_id: &str,
        tool: &ToolSchema,
        arguments: Value,
        privilege_tier: PrivilegeTier,
        causal_chain: Option<Vec<String>>,
    ) -> ClientResult<InvocationResult> {
        // 1. Trust gate.
        let installed = self
            .registry
            .get(server_id)
            .ok_or_else(|| ClientError::ServerNotInstalled(server_id.to_string()))?;
        let min_trust = tool.risk_level.min_trust();
        if installed.trust_state.rank() < min_trust.rank()
            || matches!(installed.trust_state, ServerTrustState::Quarantined)
        {
            return Err(ClientError::TrustGateFailed {
                risk: tool.risk_level,
                required: min_trust,
                current: installed.trust_state,
            });
        }

        // 2. Lockfile verification (defence in depth: drift
        // detection runs on a separate call path, but we re-verify
        // the signature on every invocation so a tampered on-disk
        // lockfile is caught even if drift detection was skipped).
        self.registry.verify_lockfile(&installed.lockfile)?;

        // 3. Optional provenance gate. If the caller supplies a
        // causal chain and the action is sensitive (HighRisk or
        // Sensitive), ask provenance whether any tainted ancestor
        // gates it.
        if let (Some(provenance), Some(chain)) = (&self.provenance, causal_chain) {
            let sensitive = matches!(
                tool.risk_level,
                ToolRiskLevel::Sensitive | ToolRiskLevel::HighRisk
            );
            if sensitive {
                let request = ProvenanceCheckRequest {
                    action: format!("mcp.invoke {}/{}", server_id, tool.name),
                    proposed_causal_chain: chain,
                    horizon_turns: Some(2),
                };
                let verdict = provenance
                    .check(request)
                    .await
                    .map_err(|err| ClientError::ProvenanceBlocked(err.to_string()))?;
                if !verdict.allowed {
                    return Err(ClientError::ProvenanceBlocked(verdict.summary));
                }
            }
        }

        // 4. Issue a DPoP proof and record the nonce so a replay
        // is detectable.
        let invocation_id = ulid::Ulid::new().to_string();
        let proof =
            self.dpop_issuer
                .issue_proof(server_id, &tool.name, &arguments, &invocation_id)?;
        self.dpop_ledger.consume(proof.nonce)?;

        // Emit InvokeAccepted so audit / telemetry can observe.
        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpClientInvokeAccepted {
                    invocation_id: invocation_id.clone(),
                    server_id: server_id.to_string(),
                    tool_name: tool.name.clone(),
                    privilege_tier,
                },
            );
            let _ = bus.publish(mcp_topics::CLIENT_INVOKE, env).await;
        }

        // 5. Dispatch to sandbox.
        let invocation_result = self
            .sandbox
            .invoke(server_id, &invocation_id, &tool.name, arguments)
            .await;

        let raw_response = match invocation_result {
            Ok((raw, _usage)) => raw,
            Err(err) => {
                // Record anomaly in registry â€” either a resource
                // limit or a trap suggests misbehavior.
                let severity = match &err {
                    SandboxError::RateLimited { .. } => AnomalySeverity::Minor,
                    SandboxError::Trap(_) | SandboxError::Policy(_) => AnomalySeverity::Minor,
                    _ => AnomalySeverity::Minor,
                };
                let _ = self
                    .registry
                    .record_anomaly(server_id, severity, err.to_string())
                    .await;
                return Err(ClientError::Sandbox(err));
            }
        };

        // 6. Worker extraction. Raw response never flows to the
        // Planner â€” only the extracted, schema-validated data
        // does.
        let extraction = self
            .worker_pool
            .extract(
                &invocation_id,
                &tool.name,
                server_id,
                &raw_response,
                &tool.output_schema,
            )
            .await;

        let extracted = match extraction {
            Ok(ex) => ex,
            Err(err) => {
                // Distinguish security-class failures (which should
                // demote trust) from quality-class failures (a
                // schema mismatch is an integration bug, not an
                // attack). Security classes:
                //   - InstructionDensityExceeded: prompt-injection
                //     pattern in the response
                //   - SchemaChangeAttempt: server returned fields
                //     it didn't declare
                // Quality classes pass through as errors but don't
                // demote.
                let is_security_class = matches!(
                    &err,
                    McpExtractionError::InstructionDensityExceeded { .. }
                        | McpExtractionError::SchemaChangeAttempt { .. }
                );
                if is_security_class {
                    let _ = self
                        .registry
                        .record_anomaly(server_id, AnomalySeverity::Minor, format!("{err:?}"))
                        .await;
                }
                if let Some(bus) = &self.bus {
                    let env: BusEnvelope = Envelope::new(
                        self.node_id.clone(),
                        OrdoMessage::McpClientInvokeResult {
                            invocation_id: invocation_id.clone(),
                            extracted_data: Err(format!("{err:?}")),
                        },
                    );
                    let _ = bus.publish(mcp_topics::CLIENT_INVOKE_RESULT, env).await;
                }
                return Err(ClientError::Extraction(err));
            }
        };

        // 7. Success â€” record trust success and return.
        let _ = self.registry.record_success(server_id).await;

        if let Some(bus) = &self.bus {
            let env: BusEnvelope = Envelope::new(
                self.node_id.clone(),
                OrdoMessage::McpClientInvokeResult {
                    invocation_id: invocation_id.clone(),
                    extracted_data: Ok(extracted.extracted_data.clone()),
                },
            );
            let _ = bus.publish(mcp_topics::CLIENT_INVOKE_RESULT, env).await;
        }

        Ok(InvocationResult {
            extracted_data: extracted.extracted_data,
            privilege_tier,
            sanitization_node_id: extracted.sanitization_node_id,
        })
    }

    pub fn dpop_ledger(&self) -> Arc<DpopLedger> {
        self.dpop_ledger.clone()
    }
}

/// Serialize helper â€” a projection of the invocation into a
/// `[[Privilege N]]`-tagged prompt fragment. Projection uses this
/// to assemble multi-source prompts without touching serializer
/// internals.
#[derive(Debug, Clone, Serialize)]
pub struct TaggedPromptFragment {
    pub tier: PrivilegeTier,
    pub body: String,
}

impl TaggedPromptFragment {
    pub fn new(tier: PrivilegeTier, body: impl Into<String>) -> Self {
        Self {
            tier,
            body: body.into(),
        }
    }

    pub fn render(&self) -> String {
        format!(
            "{}\n{}\n{}",
            self.tier.open_tag(),
            self.body,
            self.tier.close_tag()
        )
    }
}

/// Simple guard â€” prevent the Planner from issuing an MCP
/// invocation that requests a higher privilege tier than
/// `PrivilegeTier::UntrustedMcp`. Any MCP call MUST enter the
/// Planner's context as tier 5.
pub fn validate_privilege_tier_for_mcp(tier: PrivilegeTier) -> Result<(), ClientError> {
    match tier {
        PrivilegeTier::UntrustedMcp => Ok(()),
        other => Err(ClientError::BadInput(format!(
            "MCP tool output must enter at privilege tier UntrustedMcp (5); got {}",
            other.label()
        ))),
    }
}

/// Used internally by tests. Not strictly public API but kept
/// accessible for downstream debugging.
pub fn new_handle_id() -> String {
    ulid::Ulid::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use ordo_mcp_sandbox::NullHost;
    use ordo_mcp_worker::DeterministicExtractor;
    use ordo_protocol::{CapabilityDeclaration, ResourceLimits, ServerIdentity};
    use rand::rngs::OsRng;

    fn minimal_wat() -> Vec<u8> {
        let wat = r#"
            (module
              (memory (export "memory") 1)
              (global $bump (mut i32) (i32.const 1024))
              (func (export "alloc") (param $n i32) (result i32)
                (local $p i32)
                (local.set $p (global.get $bump))
                (global.set $bump (i32.add (global.get $bump) (local.get $n)))
                (local.get $p))
              (func (export "echo") (param $inp i32) (param $len i32) (result i64)
                (i64.or
                  (i64.shl (i64.extend_i32_u (local.get $inp)) (i64.const 32))
                  (i64.extend_i32_u (local.get $len)))))
        "#;
        wat::parse_str(wat).expect("valid wat")
    }

    fn test_identity() -> ServerIdentity {
        ServerIdentity {
            name: "test".into(),
            version: "0.1".into(),
            publisher: "lab".into(),
            sigstore_cert: vec![1, 2, 3],
            identity_hash: [0u8; 32],
        }
    }

    fn tool_read_only(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.into(),
            description: "t".into(),
            input_schema: serde_json::json!({ "type": "object" }),
            output_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "result": { "type": "string" }
                },
                "required": ["result"]
            }),
            risk_level: ToolRiskLevel::ReadOnly,
        }
    }

    fn tool_high_risk(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.into(),
            description: "t".into(),
            input_schema: serde_json::json!({ "type": "object" }),
            output_schema: serde_json::json!({
                "type": "object",
                "properties": { "result": { "type": "string" } },
                "required": ["result"]
            }),
            risk_level: ToolRiskLevel::HighRisk,
        }
    }

    async fn build_client_with_server(tool: &ToolSchema) -> (Arc<McpClientService>, String) {
        let registry = Arc::new(McpRegistryService::new(SigningKey::generate(&mut OsRng)));
        let sandbox = Arc::new(McpSandboxService::new(Arc::new(NullHost)).unwrap());
        sandbox
            .install(
                "srv-a",
                minimal_wat(),
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .unwrap();
        registry
            .install(
                "srv-a".into(),
                test_identity(),
                std::slice::from_ref(tool),
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .await
            .unwrap();
        let worker_pool = Arc::new(WorkerPool::new(Arc::new(DeterministicExtractor::default())));
        let client = Arc::new(McpClientService::new(
            registry.clone(),
            sandbox.clone(),
            worker_pool,
            SigningKey::generate(&mut OsRng),
        ));
        (client, "srv-a".to_string())
    }

    #[tokio::test]
    async fn invocation_routes_through_worker_and_returns_extracted_data() {
        // The minimal wasm module echoes whatever bytes it receives
        // unchanged; we pass JSON that matches the schema and
        // verify Worker extraction succeeds.
        let tool = tool_read_only("echo");
        let (client, server_id) = build_client_with_server(&tool).await;
        let arguments = serde_json::json!({ "result": "hello", "extra": "dropped" });
        let result = client
            .invoke(
                &server_id,
                &tool,
                arguments,
                PrivilegeTier::UntrustedMcp,
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.extracted_data.get("result").unwrap(), "hello");
        assert!(
            result.extracted_data.get("extra").is_none(),
            "Worker must drop undeclared fields"
        );
        assert!(matches!(result.privilege_tier, PrivilegeTier::UntrustedMcp));
        assert!(!result.sanitization_node_id.is_empty());
    }

    #[tokio::test]
    async fn high_risk_tool_on_untrusted_server_is_blocked() {
        let tool = tool_high_risk("danger");
        let (client, server_id) = build_client_with_server(&tool).await;
        let err = client
            .invoke(
                &server_id,
                &tool,
                serde_json::json!({ "result": "x" }),
                PrivilegeTier::UntrustedMcp,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::TrustGateFailed { .. }));
    }

    #[tokio::test]
    async fn invocation_to_uninstalled_server_fails() {
        let tool = tool_read_only("echo");
        let (client, _) = build_client_with_server(&tool).await;
        let err = client
            .invoke(
                "no-such-server",
                &tool,
                serde_json::json!({ "result": "x" }),
                PrivilegeTier::UntrustedMcp,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::ServerNotInstalled(_)));
    }

    #[tokio::test]
    async fn dpop_nonce_single_use_enforced() {
        let ledger = DpopLedger::default();
        let n = [7u8; 32];
        ledger.consume(n).unwrap();
        let err = ledger.consume(n).unwrap_err();
        assert!(matches!(err, DpopLedgerError::Replay));
    }

    #[tokio::test]
    async fn tagged_prompt_fragment_renders_correctly() {
        let frag = TaggedPromptFragment::new(PrivilegeTier::UntrustedMcp, "tool output here");
        let rendered = frag.render();
        assert!(rendered.starts_with("[[Privilege 5: UntrustedMcp]]"));
        assert!(rendered.ends_with("[[/Privilege 5]]"));
        assert!(rendered.contains("tool output here"));
    }

    #[tokio::test]
    async fn privilege_tier_for_mcp_must_be_untrusted() {
        assert!(validate_privilege_tier_for_mcp(PrivilegeTier::UntrustedMcp).is_ok());
        let err = validate_privilege_tier_for_mcp(PrivilegeTier::System).unwrap_err();
        assert!(matches!(err, ClientError::BadInput(_)));
    }
}
