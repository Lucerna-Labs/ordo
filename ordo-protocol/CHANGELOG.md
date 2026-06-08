# ordo-protocol changelog

Tracks changes to the wire protocol. Additive entries do not bump
the crate version; breaking entries do. See Rule 11 in
`docs/architecture-contract.md` â€” protocol changes live here, not
inside the emitting crate.

## Unreleased

### Added

- Cloud credentials (Cycle 2 of 4 for the Cloud tab work):
  `CloudCredentialView` (view-side, secret omitted) and
  `CloudCredentialFull` (write-side, secret present, in-process
  only) structs in a new `ordo-protocol::cloud` module. Ten new
  `OrdoMessage` variants pairing list / upsert / remove / test /
  set-default as request+response or request+event:
  `CloudCredentialsListRequest`, `CloudCredentialsListResponse`,
  `CloudCredentialUpsertRequest`, `CloudCredentialUpserted`,
  `CloudCredentialRemoveRequest`, `CloudCredentialRemoved`,
  `CloudCredentialTestRequest`, `CloudCredentialTestResult`,
  `CloudCredentialSetDefaultRequest`,
  `CloudCredentialDefaultChanged`. New `cloud_topics` module
  with matching topic constants (`ordo.cloud.credentials.list.request`,
  etc.) following the existing
  `memory_topics` / `secrets_topics` / `mcp_topics` pattern.
  Additive: new variants, new structs, new topics — no breaking
  changes to existing types. The publisher
  (`ordo-cloud-bridge`, Cycle 3) and consumer
  (the studio Cloud/Provider tab) ships separately; nothing
  currently publishes or subscribes, so the topics are dormant
  until Cycle 3 lands. The list response bundles
  `default_service: Option<String>` so subscribers receive an
  atomic snapshot of "all providers + which is default" in one
  envelope. Exhaustive `match` arms over `OrdoMessage` need new
  branches; the same-cycle ripple updates land in `ordo-classify`
  (all ten variants → `TrafficClass::Background` +
  `ExecutionTarget::LocalOnly`, matching the existing memory and
  secrets CRUD patterns) and `ordo-router`'s `message_kind`
  helper.
- Supervisor groundwork (PR 1 of 3): `HealthState` enum
  (`healthy` / `rescue` / `critical`), `ActivityState` enum
  (`idle` / `processing`),
  `OrdoMessage::SystemStateChanged { health, activity, reason: Option<String> }`
  variant, and `topics::SYSTEM_STATE` (`ordo.system.state`).
  Two-axis because health and activity are orthogonal — a system
  can process while degraded; collapsing the axes loses
  information visible to operators. Wire labels are stable; new
  variants on either axis are additive. No thresholds or policy
  data on the wire — the supervisor (PR 2) owns derivation. The
  consumer (the studio) renders the two axes; nothing
  currently publishes or subscribes, so the topic is dormant
  until PR 2 lands. Backwards compatible: `match` arms over
  `OrdoMessage` need a new branch or wildcard, but envelopes from
  older builds remain valid.
- Phase 0.5: `CapabilityDescriptor.input_schema: Option<serde_json::Value>`.
  Optional JSON Schema describing a provider's argument shape for LLM
  tool advertisement and the MCP `tools/list` bridge. Descriptive only
  â€” runtime dispatch remains untyped `Value` per Rule 9. Backwards
  compatible: existing serialized descriptors without the field
  deserialize fine; descriptors without a schema serialize without the
  field (via `skip_serializing_if`).
- Phase 1.1: `App`, `AppStatus`, `AppEvent`, `AppEventKind` types plus
  the `ordo.apps.event` bus topic. Wire-shared because the new MCP
  bridge, the studio, and potential webhooks all need to reason about
  apps. `AppEventKind` is non-exhaustive by convention â€” new variants
  are additive; existing labels are frozen because they persist in the
  `app_events` table.
- Phase 1.1: `OrdoMessage::AppsEvent(AppEvent)` variant. Carries a
  persisted event end-to-end (no re-query needed). Additive â€” existing
  matches on `OrdoMessage` must add the variant or a wildcard arm, but
  serialized envelopes from older builds remain compatible.
- Phase 1.3: `UserAttachment` enum (`image_url`, `image_base64` for
  v1) for multimodal turn input. Variants are additive; translators
  skip unknown variants rather than failing the turn, so new
  attachment types can land without a protocol bump.
- Phase 1.4: `FileEntry` type, `OrdoMessage::FileUploaded` and
  `FileDeleted` variants, `ordo.files.event` topic. Bytes live on
  disk under `user_files/`; SQLite keeps the metadata the platform
  queries frequently (size, content_type, sha256). Additive.
- Phase 3.1: `WebhookSubscription` type. Describes an external HTTP
  subscriber registered against a set of bus topics with an HMAC-
  SHA256 secret. The secret field never appears in list/read
  responses even though it's part of the serde shape â€” callers that
  need the struct literally must redact before emitting.
- Phase 3.3: `Deployment` and `DeploymentState` types. Tie a point in
  an app's event stream to an externally-addressable release. The
  `state_at_version` fold (Phase 1.2) reconstructs exactly what was
  deployed. `DeploymentState` labels are frozen (they persist).
- Memory v2 blueprint: new `memory` module with `MemoryEvent`,
  `MemoryEventType`, `RetentionTier` (named distinctly from the
  legacy `MemoryTier{Working,Pinned}` enum used by the ordo-memory
  working-set store), `MemoryLogQueryByRange`,
  `MemoryLogQueryResult`, `MemoryLogFilter`, `FeedbackSignal` +
  `FeedbackPolarity` + `FeedbackSource`, `RetrievalSemantics`,
  `CostHint`, `RouteMode`, `ProviderRegistration`, `TreeNode`,
  `ClassifierOutput` + `ClassifierNodeChoice`, `RouteDecided`,
  `ProjectionRequest`, `ProjectionBuilt`, `ReplayDegradedReason`,
  `ProtocolViolation` + `ProtocolViolationType` + `Severity`.
  Plus the `memory_topics` module (bus topic name constants).
  All additive, all frozen at the label level because they persist
  in the event log.
- Memory v2 concern 1 (log health): `MemoryLogHealth` +
  `MemoryLogIntegrityReport` types; `OrdoMessage::MemoryLogHealthRequest`,
  `MemoryLogHealthResponse`, `MemoryLogHealthOk`,
  `MemoryLogHealthDegraded`, `MemoryLogIntegrityResult` variants;
  `system.health_probe` event type (auto-pinned); topics
  `ordo.memory.log.health.*` and `ordo.memory.log.integrity.result`.
  Rescue Mode subscribes to the `degraded` topic so a silent write
  path failure never persists unnoticed.
- Memory v2 concern 2 (turn grouping): optional `turn_id: Option<Ulid>`
  field on `MemoryEvent`; `OrdoMessage::MemoryLogQueryByTurnRequest`
  and `MemoryLogQueryByTurnResponse` variants; topics
  `ordo.memory.log.query.by_turn.{request,response}`. Optional field
  tolerates pre-migration events with null turn_id; turn loop stamps
  every event it emits from this point forward.
- Secrets blueprint v2 (COMPLETE â€” no phasing): new `secrets` module
  with `SecretClass`, `SealingTier`, `ProtectionLevel`, `SecretRecord`,
  `CapabilityHandle`, `CapabilityCanary`, `InputCustody`,
  `StructuralOutputCheck`, `ThresholdShareAnnouncement`,
  `NonceCommitment`, `ThresholdSigningRequest`, `RotationPolicy`,
  `RotationDue`, `RotationReason`, `AuditEntry`, `SecretAuditEventType`,
  `AnchorStatement`, `TransparencyReceipt` types plus the
  `secrets_topics` module. 14 new `OrdoMessage` variants covering the
  full surface (capability issue/revoke, canary/custody/structural
  detection, tier degradation, rotation lifecycle, threshold share +
  signing lifecycle, audit append + anchor). Invariants 21â€“24 added
  (threshold quorum required; canary-per-capability-not-secret;
  rotation destroys old material; Tier-3 anchors have no external
  meaning). Additive at the type level; requires wildcard or explicit
  arms in existing exhaustive matches on `OrdoMessage`.
- MCP security architecture (COMPLETE â€” no phasing): new `mcp`
  module with `ServerTrustState`, `ServerIdentity`, `CapabilityDeclaration`,
  `ResourceLimits`, `ToolSchema`, `ToolRiskLevel`, `Taint`,
  `PrivilegeTier`, `AttenuationConstraints`, `ArgumentConstraint`,
  `DpopProof`, `Attestation`, `TrustClaim`, `McpServerLockfile`,
  `McpExtractionResult`, `McpExtractionError`, `ResourceUsage`,
  `HostCallRecord`, `HostCallOutcome`, `ProvenanceCheckRequest`,
  `ProvenanceCheckResult` types plus the `mcp_topics` module.
  22 new `OrdoMessage` variants covering the full surface
  (Worker extract, sandbox install/invoke/host-call/violation,
  client invoke/auth-degraded, registry trust/drift/quarantine/
  re-authorize, provenance check/sanitize/user-confirm/blocked).
  17 new `ProtocolViolationType` variants (sandbox escape,
  lockfile tamper, drift, egress denied, instruction injection,
  schema violation, resource limit, worker contamination,
  privilege tier, taint, sensitive-action blocked, capability
  widening, DPoP replay, auth degraded, non-WASM, signature,
  attestation). Invariants 25â€“34 added. Additive at the type
  level; exhaustive matches on `OrdoMessage` or
  `ProtocolViolationType` need the new arms.
- Memory v2: new `OrdoMessage` variants for every memory operation
  (15 variants: `MemoryLogAppendRequest/Response/Appended`,
  `MemoryLogQueryRequest/Response`, `MemoryLogColdQuery`,
  `MemoryRetentionTransition`, `MemoryRouteRequest/Response/Decided`,
  `MemoryRouteLowConfidence`, `MemoryProviderRegister/Deregister/Heartbeat`,
  `MemoryTreeChange`, `MemoryProjectionBuildRequest/Response/Built`,
  `MemoryProjectionIdentityOverBudget/ReplayDegraded`,
  `MemoryFeedbackSignal`, `MemoryProtocolViolation`) + the
  `TreeChangeType` enum.  Additive; requires wildcard arms in
  existing exhaustive matches on `OrdoMessage`.
