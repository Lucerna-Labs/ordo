use std::collections::HashMap;

use ordo_handshake::{build_hello, negotiate_handshake, NegotiationError};
use ordo_protocol::{
    CryptoSuite, Envelope, ExecutionTarget, HandshakeSelection, OrdoMessage, PeerPresence,
    RouteDirective, SessionId, TransportKind,
};
use ordo_transport::{
    plan_mesh_route, DefaultTransportAdapter, MeshRoutePlan, TransportAdapter, TransportError,
    TransportReceipt,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    LocalReady,
    Handshaking,
    Established,
    BroadcastReady,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTranscriptEvent {
    RoutePlanned {
        transport: TransportKind,
        relay_required: bool,
    },
    PeerSelected {
        peer_label: String,
    },
    HandshakeStarted,
    HandshakeNegotiated {
        transport: TransportKind,
        crypto_suite: CryptoSuite,
        pairing_required: bool,
    },
    MessageSent {
        kind: String,
        transport: TransportKind,
    },
    DeliveryConfirmed {
        transport: TransportKind,
        delivered_to: Vec<String>,
        description: String,
    },
    MessageReceived {
        kind: String,
        sender_label: String,
    },
    SessionEstablished {
        state: SessionState,
    },
}

#[derive(Debug, Clone)]
pub struct MeshSession {
    pub id: SessionId,
    pub target: ExecutionTarget,
    pub peer: Option<PeerPresence>,
    pub route_plan: MeshRoutePlan,
    pub handshake: Option<HandshakeSelection>,
    pub state: SessionState,
    pub transcript: Vec<SessionTranscriptEvent>,
    pub outbox: Vec<Envelope<OrdoMessage>>,
    pub inbox: Vec<Envelope<OrdoMessage>>,
}

#[derive(Debug, Error)]
pub enum RouterError {
    #[error("no peer matched the requested execution target")]
    NoPeerMatched,
    #[error("session not found")]
    SessionNotFound,
    #[error("transport delivery failed")]
    Transport(#[from] TransportError),
    #[error("handshake negotiation failed")]
    Handshake(#[from] NegotiationError),
}

#[derive(Debug)]
pub struct Router {
    local_peer: PeerPresence,
    transport: Box<dyn TransportAdapter>,
    sessions: HashMap<SessionId, MeshSession>,
}

impl Router {
    pub fn new(local_peer: PeerPresence) -> Self {
        Self::with_transport(local_peer, Box::new(DefaultTransportAdapter::default()))
    }

    pub fn with_transport(local_peer: PeerPresence, transport: Box<dyn TransportAdapter>) -> Self {
        Self {
            local_peer,
            transport,
            sessions: HashMap::new(),
        }
    }

    pub fn establish_session(
        &mut self,
        directive: &RouteDirective,
        peers: &[PeerPresence],
    ) -> Result<SessionId, RouterError> {
        let route_plan = plan_mesh_route(directive, peers);
        let mut transcript = vec![SessionTranscriptEvent::RoutePlanned {
            transport: route_plan.transport.clone(),
            relay_required: route_plan.relay_required,
        }];

        let session_id = SessionId::new();
        let target_peer = resolve_peer(&route_plan.target, directive, peers);

        let (peer, handshake, state) = match route_plan.target.clone() {
            ExecutionTarget::LocalOnly => {
                transcript.push(SessionTranscriptEvent::PeerSelected {
                    peer_label: self.local_peer.label.clone(),
                });
                transcript.push(SessionTranscriptEvent::HandshakeStarted);
                let local_hello = build_hello(self.local_peer.clone());
                let selection =
                    negotiate_handshake(&local_hello, &local_hello, route_plan.transport.clone())?;
                transcript.push(SessionTranscriptEvent::HandshakeNegotiated {
                    transport: selection.transport.clone(),
                    crypto_suite: selection.crypto_suite.clone(),
                    pairing_required: selection.pairing_required,
                });
                (None, Some(selection), SessionState::LocalReady)
            }
            ExecutionTarget::BestPeer | ExecutionTarget::SpecificPeer(_) => {
                let peer = target_peer.ok_or(RouterError::NoPeerMatched)?;
                transcript.push(SessionTranscriptEvent::PeerSelected {
                    peer_label: peer.label.clone(),
                });
                transcript.push(SessionTranscriptEvent::HandshakeStarted);
                let local_hello = build_hello(self.local_peer.clone());
                let remote_hello = build_hello(peer.clone());
                let selection =
                    negotiate_handshake(&local_hello, &remote_hello, route_plan.transport.clone())?;
                transcript.push(SessionTranscriptEvent::HandshakeNegotiated {
                    transport: selection.transport.clone(),
                    crypto_suite: selection.crypto_suite.clone(),
                    pairing_required: selection.pairing_required,
                });
                (Some(peer), Some(selection), SessionState::Established)
            }
            ExecutionTarget::Broadcast => (None, None, SessionState::BroadcastReady),
        };

        transcript.push(SessionTranscriptEvent::SessionEstablished {
            state: state.clone(),
        });

        let session = MeshSession {
            id: session_id.clone(),
            target: route_plan.target.clone(),
            peer,
            route_plan,
            handshake,
            state,
            transcript,
            outbox: Vec::new(),
            inbox: Vec::new(),
        };

        self.sessions.insert(session_id.clone(), session);
        Ok(session_id)
    }

    pub fn get_session(&self, id: &SessionId) -> Option<&MeshSession> {
        self.sessions.get(id)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn local_peer(&self) -> &PeerPresence {
        &self.local_peer
    }

    pub fn send_message(
        &mut self,
        session_id: &SessionId,
        payload: OrdoMessage,
    ) -> Result<TransportReceipt, RouterError> {
        let local_id = self.local_peer.id.clone();
        let local_label = self.local_peer.label.clone();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RouterError::SessionNotFound)?;

        let envelope = Envelope::new(local_id, payload.clone());
        let kind = message_kind(&payload);
        session.outbox.push(envelope.clone());
        session
            .transcript
            .push(SessionTranscriptEvent::MessageSent {
                kind: kind.clone(),
                transport: session.route_plan.transport.clone(),
            });

        let receipt = self.transport.send(
            &session.route_plan,
            &self.local_peer,
            session.peer.as_ref(),
            &envelope,
        )?;
        session
            .transcript
            .push(SessionTranscriptEvent::DeliveryConfirmed {
                transport: receipt.transport.clone(),
                delivered_to: receipt.delivered_to.clone(),
                description: receipt.description.clone(),
            });

        if receipt.loopback {
            match session.target {
                ExecutionTarget::LocalOnly => {
                    session.inbox.push(envelope);
                    session
                        .transcript
                        .push(SessionTranscriptEvent::MessageReceived {
                            kind,
                            sender_label: local_label,
                        });
                }
                ExecutionTarget::BestPeer
                | ExecutionTarget::SpecificPeer(_)
                | ExecutionTarget::Broadcast => {}
            }
        }

        Ok(receipt)
    }

    pub fn inject_incoming(
        &mut self,
        session_id: &SessionId,
        sender: PeerPresence,
        payload: OrdoMessage,
    ) -> Result<(), RouterError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RouterError::SessionNotFound)?;
        let kind = message_kind(&payload);
        session
            .inbox
            .push(Envelope::new(sender.id.clone(), payload));
        session
            .transcript
            .push(SessionTranscriptEvent::MessageReceived {
                kind,
                sender_label: sender.label,
            });
        Ok(())
    }

    pub fn drain_inbox(
        &mut self,
        session_id: &SessionId,
    ) -> Result<Vec<Envelope<OrdoMessage>>, RouterError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RouterError::SessionNotFound)?;
        Ok(std::mem::take(&mut session.inbox))
    }

    pub fn transport_name(&self) -> &'static str {
        "default"
    }
}

fn resolve_peer(
    target: &ExecutionTarget,
    directive: &RouteDirective,
    peers: &[PeerPresence],
) -> Option<PeerPresence> {
    match target {
        ExecutionTarget::SpecificPeer(id) => peers.iter().find(|peer| &peer.id == id).cloned(),
        ExecutionTarget::BestPeer => peers
            .iter()
            .find(|peer| {
                directive
                    .required_capabilities
                    .iter()
                    .all(|required| peer.capabilities.iter().any(|cap| cap == required))
            })
            .cloned(),
        ExecutionTarget::LocalOnly | ExecutionTarget::Broadcast => None,
    }
}

fn message_kind(message: &OrdoMessage) -> String {
    match message {
        OrdoMessage::Heartbeat(_) => "heartbeat",
        OrdoMessage::HealthProbe => "health_probe",
        OrdoMessage::HealthSnapshot(_) => "health_snapshot",
        OrdoMessage::SystemStateChanged { .. } => "system_state_changed",
        OrdoMessage::CapabilityInventoryRequested => "capability_inventory_requested",
        OrdoMessage::CapabilityInventorySnapshot { .. } => "capability_inventory_snapshot",
        OrdoMessage::RequirementMessage { .. } => "requirement",
        OrdoMessage::CapabilityMessage { .. } => "capability",
        OrdoMessage::RunRequested { .. } => "run_requested",
        OrdoMessage::RunAccepted { .. } => "run_accepted",
        OrdoMessage::StepStarted { .. } => "step_started",
        OrdoMessage::StepCompleted { .. } => "step_completed",
        OrdoMessage::StepFailed { .. } => "step_failed",
        OrdoMessage::RunFinished { .. } => "run_finished",
        OrdoMessage::BuildStepCompleted(_) => "build_step_completed",
        OrdoMessage::BuildGateResult(_) => "build_gate_result",
        OrdoMessage::BuildPlannerEvent(_) => "build_planner_event",
        OrdoMessage::RagIngestRequested { .. } => "rag_ingest_requested",
        OrdoMessage::RagDocumentIndexed { .. } => "rag_document_indexed",
        OrdoMessage::RagCollectionsRequested => "rag_collections_requested",
        OrdoMessage::RagCollectionsListed { .. } => "rag_collections_listed",
        OrdoMessage::RagQueryRequested { .. } => "rag_query_requested",
        OrdoMessage::RagQueryCompleted { .. } => "rag_query_completed",
        OrdoMessage::ToolCallRequested { .. } => "tool_call_requested",
        OrdoMessage::ToolCallCompleted { .. } => "tool_call_completed",
        OrdoMessage::ToolCallFailed { .. } => "tool_call_failed",
        OrdoMessage::MemoryStored { .. } => "memory_stored",
        OrdoMessage::MemoryStoreRequested { .. } => "memory_store_requested",
        OrdoMessage::MemoryStoreCompleted { .. } => "memory_store_completed",
        OrdoMessage::MemoryRemoveRequested { .. } => "memory_remove_requested",
        OrdoMessage::MemoryRemoveCompleted { .. } => "memory_remove_completed",
        OrdoMessage::MemoryListRequested { .. } => "memory_list_requested",
        OrdoMessage::MemoryListed { .. } => "memory_listed",
        OrdoMessage::MemoryQuery { .. } => "memory_query",
        OrdoMessage::MemoryQueried { .. } => "memory_queried",
        OrdoMessage::SelfHealRequested { .. } => "self_heal_requested",
        OrdoMessage::SelfHealPlanned { .. } => "self_heal_planned",
        OrdoMessage::ModelInvocationRequested { .. } => "model_invocation_requested",
        OrdoMessage::ModelInvocationCompleted { .. } => "model_invocation_completed",
        OrdoMessage::AppsEvent(_) => "apps_event",
        OrdoMessage::FileUploaded(_) => "file_uploaded",
        OrdoMessage::FileDeleted { .. } => "file_deleted",
        // Memory v2 â€” all classified as "memory.*" for routing.
        OrdoMessage::MemoryLogAppendRequest { .. } => "memory_log_append_request",
        OrdoMessage::MemoryLogAppendResponse { .. } => "memory_log_append_response",
        OrdoMessage::MemoryLogAppended { .. } => "memory_log_appended",
        OrdoMessage::MemoryLogQueryRequest(_) => "memory_log_query_request",
        OrdoMessage::MemoryLogQueryResponse(_) => "memory_log_query_response",
        OrdoMessage::MemoryLogColdQuery { .. } => "memory_log_cold_query",
        OrdoMessage::MemoryRetentionTransition { .. } => "memory_retention_transition",
        OrdoMessage::MemoryRouteRequest { .. } => "memory_route_request",
        OrdoMessage::MemoryRouteResponse(_) => "memory_route_response",
        OrdoMessage::MemoryRouteDecided(_) => "memory_route_decided",
        OrdoMessage::MemoryRouteLowConfidence { .. } => "memory_route_low_confidence",
        OrdoMessage::MemoryProviderRegister(_) => "memory_provider_register",
        OrdoMessage::MemoryProviderDeregister { .. } => "memory_provider_deregister",
        OrdoMessage::MemoryProviderHeartbeat { .. } => "memory_provider_heartbeat",
        OrdoMessage::MemoryTreeChange { .. } => "memory_tree_change",
        OrdoMessage::MemoryProjectionBuildRequest(_) => "memory_projection_build_request",
        OrdoMessage::MemoryProjectionBuildResponse(_) => "memory_projection_build_response",
        OrdoMessage::MemoryProjectionBuilt(_) => "memory_projection_built",
        OrdoMessage::MemoryProjectionIdentityOverBudget { .. } => {
            "memory_projection_identity_over_budget"
        }
        OrdoMessage::MemoryProjectionReplayDegraded { .. } => "memory_projection_replay_degraded",
        OrdoMessage::MemoryFeedbackSignal(_) => "memory_feedback_signal",
        OrdoMessage::MemoryProtocolViolation(_) => "memory_protocol_violation",
        OrdoMessage::MemoryLogHealthRequest => "memory_log_health_request",
        OrdoMessage::MemoryLogHealthResponse(_) => "memory_log_health_response",
        OrdoMessage::MemoryLogHealthOk(_) => "memory_log_health_ok",
        OrdoMessage::MemoryLogHealthDegraded { .. } => "memory_log_health_degraded",
        OrdoMessage::MemoryLogIntegrityResult(_) => "memory_log_integrity_result",
        OrdoMessage::MemoryLogQueryByTurnRequest { .. } => "memory_log_query_by_turn_request",
        OrdoMessage::MemoryLogQueryByTurnResponse(_) => "memory_log_query_by_turn_response",
        OrdoMessage::SecretsCapabilityIssued { .. } => "secrets_capability_issued",
        OrdoMessage::SecretsCapabilityRevoked { .. } => "secrets_capability_revoked",
        OrdoMessage::SecretsCanaryDetected { .. } => "secrets_canary_detected",
        OrdoMessage::SecretsCustodyMismatch(_) => "secrets_custody_mismatch",
        OrdoMessage::SecretsStructuralRejection(_) => "secrets_structural_rejection",
        OrdoMessage::SecretsSealTierDegraded { .. } => "secrets_seal_tier_degraded",
        OrdoMessage::SecretsRotationDue(_) => "secrets_rotation_due",
        OrdoMessage::SecretsRotationCompleted { .. } => "secrets_rotation_completed",
        OrdoMessage::SecretsThresholdShareAnnouncement(_) => "secrets_threshold_share_announcement",
        OrdoMessage::SecretsThresholdSigningRequest(_) => "secrets_threshold_signing_request",
        OrdoMessage::SecretsThresholdSigningCompleted { .. } => {
            "secrets_threshold_signing_completed"
        }
        OrdoMessage::SecretsAuditEntryAppended { .. } => "secrets_audit_entry_appended",
        OrdoMessage::SecretsAuditAnchorSigned(_) => "secrets_audit_anchor_signed",
        OrdoMessage::McpWorkerExtract { .. } => "mcp_worker_extract",
        OrdoMessage::McpWorkerExtractResult { .. } => "mcp_worker_extract_result",
        OrdoMessage::McpWorkerStatus { .. } => "mcp_worker_status",
        OrdoMessage::McpSandboxInstalled { .. } => "mcp_sandbox_installed",
        OrdoMessage::McpSandboxUninstalled { .. } => "mcp_sandbox_uninstalled",
        OrdoMessage::McpSandboxInvoke { .. } => "mcp_sandbox_invoke",
        OrdoMessage::McpSandboxInvokeResult { .. } => "mcp_sandbox_invoke_result",
        OrdoMessage::McpSandboxHostCall(_) => "mcp_sandbox_host_call",
        OrdoMessage::McpSandboxViolation { .. } => "mcp_sandbox_violation",
        OrdoMessage::McpClientInvokeAccepted { .. } => "mcp_client_invoke_accepted",
        OrdoMessage::McpClientInvokeResult { .. } => "mcp_client_invoke_result",
        OrdoMessage::McpClientAuthDegraded { .. } => "mcp_client_auth_degraded",
        OrdoMessage::McpRegistryTrustChanged { .. } => "mcp_registry_trust_changed",
        OrdoMessage::McpRegistryDriftDetected { .. } => "mcp_registry_drift_detected",
        OrdoMessage::McpRegistryQuarantined { .. } => "mcp_registry_quarantined",
        OrdoMessage::McpRegistryReAuthorized { .. } => "mcp_registry_re_authorized",
        OrdoMessage::McpProvenanceCheckRequest(_) => "mcp_provenance_check_request",
        OrdoMessage::McpProvenanceCheckResult(_) => "mcp_provenance_check_result",
        OrdoMessage::McpProvenanceSanitized { .. } => "mcp_provenance_sanitized",
        OrdoMessage::McpProvenanceUserConfirmed { .. } => "mcp_provenance_user_confirmed",
        OrdoMessage::McpProvenanceSensitiveBlocked { .. } => "mcp_provenance_sensitive_blocked",
        OrdoMessage::CloudCredentialsListRequest => "cloud_credentials_list_request",
        OrdoMessage::CloudCredentialsListResponse { .. } => "cloud_credentials_list_response",
        OrdoMessage::CloudCredentialUpsertRequest { .. } => "cloud_credential_upsert_request",
        OrdoMessage::CloudCredentialUpserted(_) => "cloud_credential_upserted",
        OrdoMessage::CloudCredentialRemoveRequest { .. } => "cloud_credential_remove_request",
        OrdoMessage::CloudCredentialRemoved { .. } => "cloud_credential_removed",
        OrdoMessage::CloudCredentialTestRequest { .. } => "cloud_credential_test_request",
        OrdoMessage::CloudCredentialTestResult { .. } => "cloud_credential_test_result",
        OrdoMessage::CloudCredentialSetDefaultRequest { .. } => {
            "cloud_credential_set_default_request"
        }
        OrdoMessage::CloudCredentialDefaultChanged { .. } => "cloud_credential_default_changed",
        OrdoMessage::EmailCommandReceived { .. } => "email_command_received",
        OrdoMessage::EmailReplyRequested { .. } => "email_reply_requested",
        OrdoMessage::GoalSubmitted { .. } => "goal_submitted",
        OrdoMessage::PlanCreated { .. } => "plan_created",
        OrdoMessage::TaskQueued { .. } => "task_queued",
        OrdoMessage::TaskStarted { .. } => "task_started",
        OrdoMessage::TaskCompleted { .. } => "task_completed",
        OrdoMessage::TaskFailed { .. } => "task_failed",
        OrdoMessage::PolicyCheckRequired { .. } => "policy_check_required",
        OrdoMessage::UserApprovalRequired { .. } => "user_approval_required",
        OrdoMessage::GoalCompleted { .. } => "goal_completed",
        OrdoMessage::TaskVerified { .. } => "task_verified",
        OrdoMessage::JobScheduled { .. } => "job_scheduled",
        OrdoMessage::JobTriggered { .. } => "job_triggered",
        OrdoMessage::JobCompleted { .. } => "job_completed",
        OrdoMessage::JobFailed { .. } => "job_failed",
        OrdoMessage::TtsUtteranceStarted(_) => "tts_utterance_started",
        OrdoMessage::TtsPhonemeFrame(_) => "tts_phoneme_frame",
        OrdoMessage::TtsUtteranceEnded(_) => "tts_utterance_ended",
        OrdoMessage::AvatarFrameEmitted(_) => "avatar_frame_emitted",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use ordo_protocol::{
        CryptoSuite, ExecutionTarget, NatKind, OrdoMessage, PairingMode, PeerPresence,
        RouteDirective, TrafficClass, TransportKind, TrustTier,
    };

    use super::{Router, RouterError, SessionState, SessionTranscriptEvent};
    use ordo_protocol::NodeId;

    fn local_peer() -> PeerPresence {
        PeerPresence {
            id: NodeId::new(),
            label: "local".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::LocalProcess,
            pairing_mode: PairingMode::LocalOnly,
            nat_kind: NatKind::OpenInternet,
            transports: vec![
                TransportKind::InProcess,
                TransportKind::Quic,
                TransportKind::RelayQuic,
                TransportKind::TcpNoise,
            ],
            crypto_suites: vec![
                CryptoSuite::InProcess,
                CryptoSuite::HybridPqNoiseX25519,
                CryptoSuite::NoiseX25519,
            ],
            endpoints: vec!["inproc://local".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        }
    }

    fn remote_peer() -> PeerPresence {
        PeerPresence {
            id: NodeId::new(),
            label: "peer-a".to_string(),
            protocol_version: "ordo/0.1".to_string(),
            trust_tier: TrustTier::PairedPeer,
            pairing_mode: PairingMode::PairingRequired,
            nat_kind: NatKind::Cone,
            transports: vec![TransportKind::Quic, TransportKind::RelayQuic],
            crypto_suites: vec![CryptoSuite::HybridPqNoiseX25519, CryptoSuite::NoiseX25519],
            endpoints: vec!["quic://peer-a".to_string()],
            capabilities: vec!["filesystem.read_file".to_string()],
        }
    }

    #[test]
    fn establishes_remote_session_with_transcript() {
        let mut router = Router::new(local_peer());
        let session_id = router
            .establish_session(
                &RouteDirective {
                    traffic_class: TrafficClass::Interactive,
                    execution_target: ExecutionTarget::BestPeer,
                    required_capabilities: vec!["filesystem.read_file".to_string()],
                    prefer_pq: true,
                    allow_relay_fallback: true,
                },
                &[remote_peer()],
            )
            .expect("session");

        let session = router.get_session(&session_id).expect("stored session");
        assert_eq!(session.state, SessionState::Established);
        assert_eq!(session.route_plan.transport, TransportKind::Quic);
        assert_eq!(
            session.handshake.as_ref().expect("handshake").crypto_suite,
            CryptoSuite::HybridPqNoiseX25519
        );
        assert!(matches!(
            session.transcript.last(),
            Some(SessionTranscriptEvent::SessionEstablished {
                state: SessionState::Established
            })
        ));
    }

    #[test]
    fn establishes_local_loopback_session() {
        let mut router = Router::new(local_peer());
        let session_id = router
            .establish_session(
                &RouteDirective {
                    traffic_class: TrafficClass::Background,
                    execution_target: ExecutionTarget::LocalOnly,
                    required_capabilities: Vec::new(),
                    prefer_pq: false,
                    allow_relay_fallback: false,
                },
                &[],
            )
            .expect("local session");

        let session = router.get_session(&session_id).expect("stored session");
        assert_eq!(session.state, SessionState::LocalReady);
        assert_eq!(
            session
                .handshake
                .as_ref()
                .expect("loopback handshake")
                .transport,
            TransportKind::InProcess
        );
        assert_eq!(router.session_count(), 1);
    }

    #[test]
    fn missing_peer_is_reported() {
        let mut router = Router::new(local_peer());
        let error = router
            .establish_session(
                &RouteDirective {
                    traffic_class: TrafficClass::Interactive,
                    execution_target: ExecutionTarget::BestPeer,
                    required_capabilities: vec!["filesystem.read_file".to_string()],
                    prefer_pq: true,
                    allow_relay_fallback: true,
                },
                &[],
            )
            .expect_err("missing peer");

        assert!(matches!(error, RouterError::NoPeerMatched));
    }

    #[test]
    fn remote_session_can_send_and_receive_messages() {
        let remote = remote_peer();
        let mut router = Router::new(local_peer());
        let session_id = router
            .establish_session(
                &RouteDirective {
                    traffic_class: TrafficClass::Interactive,
                    execution_target: ExecutionTarget::BestPeer,
                    required_capabilities: vec!["filesystem.read_file".to_string()],
                    prefer_pq: true,
                    allow_relay_fallback: true,
                },
                std::slice::from_ref(&remote),
            )
            .expect("session");

        let receipt = router
            .send_message(
                &session_id,
                OrdoMessage::RequirementMessage {
                    requirement: "read file config.json".to_string(),
                },
            )
            .expect("send");
        assert_eq!(receipt.transport, TransportKind::Quic);
        assert_eq!(receipt.delivered_to, vec!["peer-a".to_string()]);
        assert_eq!(receipt.description, "simulated peer delivery to peer-a");

        router
            .inject_incoming(
                &session_id,
                remote,
                OrdoMessage::CapabilityMessage {
                    capability: "filesystem.read_file".to_string(),
                    description: "Reads files from the local disk.".to_string(),
                },
            )
            .expect("inject");

        let inbox = router.drain_inbox(&session_id).expect("drain inbox");
        assert_eq!(inbox.len(), 1);
        assert!(matches!(
            inbox[0].payload,
            OrdoMessage::CapabilityMessage { .. }
        ));
    }

    #[test]
    fn local_session_loops_messages_back_into_inbox() {
        let mut router = Router::new(local_peer());
        let session_id = router
            .establish_session(
                &RouteDirective {
                    traffic_class: TrafficClass::Background,
                    execution_target: ExecutionTarget::LocalOnly,
                    required_capabilities: Vec::new(),
                    prefer_pq: false,
                    allow_relay_fallback: false,
                },
                &[],
            )
            .expect("local session");

        router
            .send_message(
                &session_id,
                OrdoMessage::MemoryQuery {
                    query: "config".to_string(),
                },
            )
            .expect("send loopback");

        let inbox = router.drain_inbox(&session_id).expect("loopback inbox");
        assert_eq!(inbox.len(), 1);
        assert!(matches!(inbox[0].payload, OrdoMessage::MemoryQuery { .. }));

        let session = router.get_session(&session_id).expect("stored session");
        assert!(session.transcript.iter().any(|event| {
            matches!(
                event,
                SessionTranscriptEvent::DeliveryConfirmed {
                    transport: TransportKind::InProcess,
                    ..
                }
            )
        }));
    }
}
