// NOTE: the text-to-mode auto-classifier (`mode_classifier`) was retired.
// Modes are chosen explicitly at session creation (and, going forward, are
// operator-created — see docs/mode-lifecycle.md), so a TF-IDF router that
// guessed a mode from the prompt against a fixed template list no longer fits
// the architecture. It had no callers. This crate now only does message
// traffic/route classification (below).

use ordo_protocol::{ExecutionTarget, OrdoMessage, PeerPresence, RouteDirective, TrafficClass};

#[derive(Debug, Clone)]
pub struct ClassificationInput {
    pub message: OrdoMessage,
    pub peers: Vec<PeerPresence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationDecision {
    pub directive: RouteDirective,
    pub rationale: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedClassifier;

impl RuleBasedClassifier {
    pub fn classify(&self, input: &ClassificationInput) -> ClassificationDecision {
        let traffic_class = classify_message(&input.message);
        let required_capabilities = infer_required_capabilities(&input.message);
        let peer_available = has_peer_for_capabilities(&input.peers, &required_capabilities);

        let execution_target = match &input.message {
            OrdoMessage::Heartbeat(_)
            | OrdoMessage::HealthProbe
            | OrdoMessage::HealthSnapshot(_) => ExecutionTarget::Broadcast,
            OrdoMessage::CapabilityInventoryRequested
            | OrdoMessage::CapabilityInventorySnapshot { .. }
            | OrdoMessage::MemoryStored { .. }
            | OrdoMessage::MemoryStoreRequested { .. }
            | OrdoMessage::MemoryStoreCompleted { .. }
            | OrdoMessage::MemoryRemoveRequested { .. }
            | OrdoMessage::MemoryRemoveCompleted { .. }
            | OrdoMessage::MemoryListRequested { .. }
            | OrdoMessage::MemoryListed { .. }
            | OrdoMessage::MemoryQuery { .. }
            | OrdoMessage::MemoryQueried { .. }
            | OrdoMessage::SelfHealRequested { .. }
            | OrdoMessage::SelfHealPlanned { .. }
            | OrdoMessage::RagDocumentIndexed { .. }
            | OrdoMessage::RagCollectionsRequested
            | OrdoMessage::RagCollectionsListed { .. }
            | OrdoMessage::RagQueryCompleted { .. } => ExecutionTarget::LocalOnly,
            _ if !required_capabilities.is_empty() && peer_available => ExecutionTarget::BestPeer,
            _ => ExecutionTarget::LocalOnly,
        };

        let directive = RouteDirective {
            traffic_class,
            execution_target: execution_target.clone(),
            required_capabilities,
            prefer_pq: !matches!(execution_target, ExecutionTarget::LocalOnly),
            allow_relay_fallback: matches!(
                execution_target,
                ExecutionTarget::BestPeer | ExecutionTarget::SpecificPeer(_)
            ),
        };

        let rationale = match directive.execution_target {
            ExecutionTarget::LocalOnly => {
                "kept local because no remote capability was required".to_string()
            }
            ExecutionTarget::Broadcast => {
                "broadcast control traffic across discovered peers".to_string()
            }
            ExecutionTarget::BestPeer => {
                "remote capability match found; prefer best peer".to_string()
            }
            ExecutionTarget::SpecificPeer(_) => {
                "explicit peer target supplied by higher-level policy".to_string()
            }
        };

        ClassificationDecision {
            directive,
            rationale,
        }
    }
}

fn classify_message(message: &OrdoMessage) -> TrafficClass {
    match message {
        OrdoMessage::Heartbeat(_) | OrdoMessage::HealthProbe | OrdoMessage::HealthSnapshot(_) => {
            TrafficClass::Control
        }
        OrdoMessage::CapabilityInventoryRequested
        | OrdoMessage::CapabilityInventorySnapshot { .. } => TrafficClass::Control,
        OrdoMessage::MemoryStored { .. }
        | OrdoMessage::MemoryStoreRequested { .. }
        | OrdoMessage::MemoryStoreCompleted { .. }
        | OrdoMessage::MemoryRemoveRequested { .. }
        | OrdoMessage::MemoryRemoveCompleted { .. }
        | OrdoMessage::MemoryListRequested { .. }
        | OrdoMessage::MemoryListed { .. }
        | OrdoMessage::MemoryQuery { .. }
        | OrdoMessage::MemoryQueried { .. }
        | OrdoMessage::SelfHealRequested { .. }
        | OrdoMessage::SelfHealPlanned { .. }
        | OrdoMessage::RagIngestRequested { .. }
        | OrdoMessage::RagDocumentIndexed { .. }
        | OrdoMessage::RagCollectionsRequested
        | OrdoMessage::RagCollectionsListed { .. }
        | OrdoMessage::RagQueryRequested { .. }
        | OrdoMessage::RagQueryCompleted { .. }
        | OrdoMessage::ToolCallRequested { .. }
        | OrdoMessage::ToolCallCompleted { .. }
        | OrdoMessage::ToolCallFailed { .. } => TrafficClass::Background,
        OrdoMessage::RunRequested { .. }
        | OrdoMessage::RunAccepted { .. }
        | OrdoMessage::StepStarted { .. }
        | OrdoMessage::StepCompleted { .. }
        | OrdoMessage::StepFailed { .. }
        | OrdoMessage::RunFinished { .. }
        | OrdoMessage::ModelInvocationRequested { .. }
        | OrdoMessage::ModelInvocationCompleted { .. }
        | OrdoMessage::AppsEvent(_)
        | OrdoMessage::FileUploaded(_)
        | OrdoMessage::FileDeleted { .. }
        | OrdoMessage::MemoryLogAppendRequest { .. }
        | OrdoMessage::MemoryLogAppendResponse { .. }
        | OrdoMessage::MemoryLogAppended { .. }
        | OrdoMessage::MemoryLogQueryRequest(_)
        | OrdoMessage::MemoryLogQueryResponse(_)
        | OrdoMessage::MemoryLogColdQuery { .. }
        | OrdoMessage::MemoryRetentionTransition { .. }
        | OrdoMessage::MemoryRouteRequest { .. }
        | OrdoMessage::MemoryRouteResponse(_)
        | OrdoMessage::MemoryRouteDecided(_)
        | OrdoMessage::MemoryRouteLowConfidence { .. }
        | OrdoMessage::MemoryProviderRegister(_)
        | OrdoMessage::MemoryProviderDeregister { .. }
        | OrdoMessage::MemoryProviderHeartbeat { .. }
        | OrdoMessage::MemoryTreeChange { .. }
        | OrdoMessage::MemoryProjectionBuildRequest(_)
        | OrdoMessage::MemoryProjectionBuildResponse(_)
        | OrdoMessage::MemoryProjectionBuilt(_)
        | OrdoMessage::MemoryProjectionIdentityOverBudget { .. }
        | OrdoMessage::MemoryProjectionReplayDegraded { .. }
        | OrdoMessage::MemoryFeedbackSignal(_)
        | OrdoMessage::MemoryProtocolViolation(_)
        | OrdoMessage::MemoryLogHealthRequest
        | OrdoMessage::MemoryLogHealthResponse(_)
        | OrdoMessage::MemoryLogHealthOk(_)
        | OrdoMessage::MemoryLogHealthDegraded { .. }
        | OrdoMessage::MemoryLogIntegrityResult(_)
        | OrdoMessage::MemoryLogQueryByTurnRequest { .. }
        | OrdoMessage::MemoryLogQueryByTurnResponse(_)
        | OrdoMessage::SecretsCapabilityIssued { .. }
        | OrdoMessage::SecretsCapabilityRevoked { .. }
        | OrdoMessage::SecretsCanaryDetected { .. }
        | OrdoMessage::SecretsCustodyMismatch(_)
        | OrdoMessage::SecretsStructuralRejection(_)
        | OrdoMessage::SecretsSealTierDegraded { .. }
        | OrdoMessage::SecretsRotationDue(_)
        | OrdoMessage::SecretsRotationCompleted { .. }
        | OrdoMessage::SecretsThresholdShareAnnouncement(_)
        | OrdoMessage::SecretsThresholdSigningRequest(_)
        | OrdoMessage::SecretsThresholdSigningCompleted { .. }
        | OrdoMessage::SecretsAuditEntryAppended { .. }
        | OrdoMessage::SecretsAuditAnchorSigned(_)
        | OrdoMessage::McpWorkerExtract { .. }
        | OrdoMessage::McpWorkerExtractResult { .. }
        | OrdoMessage::McpWorkerStatus { .. }
        | OrdoMessage::McpSandboxInstalled { .. }
        | OrdoMessage::McpSandboxUninstalled { .. }
        | OrdoMessage::McpSandboxInvoke { .. }
        | OrdoMessage::McpSandboxInvokeResult { .. }
        | OrdoMessage::McpSandboxHostCall(_)
        | OrdoMessage::McpSandboxViolation { .. }
        | OrdoMessage::McpClientInvokeAccepted { .. }
        | OrdoMessage::McpClientInvokeResult { .. }
        | OrdoMessage::McpClientAuthDegraded { .. }
        | OrdoMessage::McpRegistryTrustChanged { .. }
        | OrdoMessage::McpRegistryDriftDetected { .. }
        | OrdoMessage::McpRegistryQuarantined { .. }
        | OrdoMessage::McpRegistryReAuthorized { .. }
        | OrdoMessage::McpProvenanceCheckRequest(_)
        | OrdoMessage::McpProvenanceCheckResult(_)
        | OrdoMessage::McpProvenanceSanitized { .. }
        | OrdoMessage::McpProvenanceUserConfirmed { .. }
        | OrdoMessage::McpProvenanceSensitiveBlocked { .. } => TrafficClass::Background,
        OrdoMessage::RequirementMessage { .. } | OrdoMessage::CapabilityMessage { .. } => {
            TrafficClass::Interactive
        }
        // TTS phoneme stream + avatar performance frames drive the live
        // avatar window; they must be delivered promptly, so classify them
        // as Interactive rather than letting them fall to Background.
        OrdoMessage::TtsUtteranceStarted(_)
        | OrdoMessage::TtsPhonemeFrame(_)
        | OrdoMessage::TtsUtteranceEnded(_)
        | OrdoMessage::AvatarFrameEmitted(_) => TrafficClass::Interactive,
        // New variants added since this code was last touched default to Background.
        _ => TrafficClass::Background,
    }
}

fn infer_required_capabilities(message: &OrdoMessage) -> Vec<String> {
    match message {
        OrdoMessage::RequirementMessage { requirement } => {
            let requirement = requirement.to_ascii_lowercase();
            let mut caps = Vec::new();

            if requirement.contains("read file") {
                caps.push("filesystem.read_file".to_string());
            }
            if requirement.contains("write file") {
                caps.push("filesystem.write_file".to_string());
            }
            if requirement.contains("summarize")
                || requirement.contains("summary")
                || requirement.contains("explain")
                || requirement.contains("architecture")
                || requirement.contains("design")
            {
                caps.push("knowledge.summarize".to_string());
            }

            caps
        }
        OrdoMessage::RagIngestRequested { .. } => vec!["rag.ingest_document".to_string()],
        OrdoMessage::RagQueryRequested { .. } => vec!["rag.query".to_string()],
        OrdoMessage::RunRequested {
            plan: Some(plan), ..
        } => plan
            .steps
            .iter()
            .map(|step| step.capability.clone())
            .collect(),
        OrdoMessage::ToolCallRequested { capability, .. } => vec![capability.clone()],
        _ => Vec::new(),
    }
}

fn has_peer_for_capabilities(peers: &[PeerPresence], required_capabilities: &[String]) -> bool {
    if required_capabilities.is_empty() {
        return false;
    }

    peers.iter().any(|peer| {
        required_capabilities
            .iter()
            .all(|required| peer.capabilities.iter().any(|cap| cap == required))
    })
}

#[cfg(test)]
mod tests {
    use ordo_protocol::{CryptoSuite, NatKind, NodeId, PairingMode, TransportKind, TrustTier};

    use super::{ClassificationInput, ExecutionTarget, RuleBasedClassifier};
    use ordo_protocol::{OrdoMessage, PeerPresence, TrafficClass};

    fn filesystem_peer() -> PeerPresence {
        PeerPresence {
            id: NodeId::new(),
            label: "peer-a".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::Quic],
            crypto_suites: vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
            endpoints: vec!["quic://peer-a".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        }
    }

    #[test]
    fn requirement_routes_to_best_peer_when_capability_exists() {
        let classifier = RuleBasedClassifier;
        let decision = classifier.classify(&ClassificationInput {
            message: OrdoMessage::RequirementMessage {
                requirement: "please read file config.json".to_string(),
            },
            peers: vec![filesystem_peer()],
        });

        assert_eq!(decision.directive.traffic_class, TrafficClass::Interactive);
        assert_eq!(
            decision.directive.execution_target,
            ExecutionTarget::BestPeer
        );
        assert_eq!(
            decision.directive.required_capabilities,
            vec!["filesystem.read_file".to_string()]
        );
        assert!(decision.directive.prefer_pq);
    }

    #[test]
    fn memory_query_stays_local() {
        let classifier = RuleBasedClassifier;
        let decision = classifier.classify(&ClassificationInput {
            message: OrdoMessage::MemoryQuery {
                query: "config".to_string(),
            },
            peers: vec![filesystem_peer()],
        });

        assert_eq!(decision.directive.traffic_class, TrafficClass::Background);
        assert_eq!(
            decision.directive.execution_target,
            ExecutionTarget::LocalOnly
        );
        assert!(decision.directive.required_capabilities.is_empty());
    }
}
