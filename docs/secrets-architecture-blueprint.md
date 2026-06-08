# Ordo â€” Secrets Architecture Blueprint (complete)

**Status: DESIGN HANDOFF.** This blueprint is the build spec for
the secrets crates (`ordo-secrets-vault`, `ordo-secrets-broker`,
`ordo-secrets-audit`, `ordo-secrets-threshold`). It is NOT yet
actionable â€” implementation is gated on the nine open questions at
the bottom of this document. Do not write code against this spec
until those answers are in the doc. See also the principle section
at the end: conventional "v2 roadmap" framing is the failure mode
this blueprint was rewritten to reject.

**Purpose:** Capability-mediated, bus-gated, locally-sealed,
cryptographically-audited secrets management. Built complete on
first commit â€” no "upgrade paths" for things that can be built now.

## Load-bearing commitments

1. The LLM never sees secrets. Ever. Handles only.
2. Secrets are scoped to providers by registration.
3. Every dereference is a logged, cryptographically chained event.
4. Master key is sealed to the best available hardware, with
   software fallback only on hardware-absent platforms.

## What changes from the previous draft

The earlier version deferred five things to "v2." Each is
re-evaluated:

**Threshold protection for high-value secrets â€” IN.**
FROST has production-grade Rust (`frost-ed25519`, reviewed). The
complexity is real but bounded. Deferring this is deferring the
single strongest protection for SSH cluster keys, which is exactly
the kind of oversight that causes catastrophic compromise later.
Threshold protection for `SshClusterAdmin` and `SigningKey`
classes lands in the core blueprint. Other classes remain
single-sealed â€” threshold for every API key is operationally
hostile and buys little.

**Automated rotation â€” IN (as an architectural primitive, not a
full-blown daemon).**
Rotation *policy* is built in: per-class rotation-due timestamps,
rotation-due events emitted on schedule, per-provider
`RotationAdvertisement` trait that providers implement to describe
how rotation works for their credential type. Providers that can
self-rotate (OAuth refresh tokens, cloud API keys via provider
APIs) do so. Providers that can't (SSH keys without a registered
CA) emit a rotation-required event and the user handles it. The
architectural slot for fully-automated rotation is present from
day one; what gets automated is a function of which providers
implement `AutoRotator`, not a separate phase.

**Full SCITT distributed transparency â€” deferred (honest reason,
not deferred).**
This genuinely requires a service you don't run and infrastructure
that doesn't make sense for a local-first single-user tool today.
But the *interface* is built: audit events are SCITT-shaped
(COSE-compatible signed statements), anchors are exportable in
SCITT receipt format, and the audit crate has a
`TransparencyService` trait with `LocalAnchor` as the default
implementation. If you ever want to submit to a real transparency
service (or run your own), you implement the trait; you don't
rewrite anything. The difference from the previous framing: this
is a *pluggable implementation*, not a future feature.

**Puncturable encryption â€” deferred (genuine library absence).**
No production-grade Rust implementation exists. The architectural
slot is present: `SecretLifecycleBackend` trait with `DekRotation`
as the default implementation. When puncturable encryption matures
in Rust, a new implementation of the trait drops in. This is the
only deferred item where the reason is truly "the tool doesn't
exist yet."

**CCA / TDX / CHERI â€” deferred (genuine hardware absence).**
Same pattern: the isolation boundary is defined at the
architectural level (capability handles, provider compartments,
isolator pass). When hardware CVM is commodity, the compartment
boundaries become hardware-enforced. The software implementation
is not placeholder â€” it's the correct implementation for the
platforms that exist today, and it happens to be forward-compatible
with stronger hardware.

**Cloud anchor export â€” IN.**
Previously deferred to v2 with a chicken-and-egg concern. The
chicken-and-egg is solvable: cloud export credentials are
themselves secrets in the vault, sealed on first configuration.
If the vault is compromised, cloud export fails closed (no anchors
leave), which is the right failure mode. Cloud export lands in the
core.

**Prompt-injection-specific defenses beyond the DRIFT isolator â€”
IN (the ones that are code).**
The isolator's detector pipeline was already pattern-matching,
structural, and capability-plaintext. Add:

- **Canary tokens** per active capability: the broker generates a
  unique canary string when a capability is issued and injects it
  into the tool's prompt context as a trap. If the tool's output
  contains the canary, the output is routed to rescue mode and
  the provider is flagged for review. Canary generation is free;
  detection is a string search.
- **Return-value structural limits**: tool outputs exceeding
  configured byte budgets or containing content types outside the
  provider's declared output schema are rejected as suspicious.
  Exfiltration attempts often manifest as unusually large or
  differently-typed output.
- **Chain-of-custody hashing for tool inputs**: the planner
  computes a hash of the tool's input (user intent + declared
  capabilities), and the validator verifies the tool's output
  doesn't claim to be responding to a different input than it
  received. Catches cases where a compromised tool rewrites its
  perceived task.

These are all code, all achievable now, all directly useful.
Leaving them for "later" is deferring real defense for hypothetical
priority.

## Revised crate structure

Same three crates, each now complete, plus a peer fourth crate for
FROST:

**`ordo-secrets-vault`** â€” unchanged from previous draft, with the
following complete from day one:

- All four sealing tiers implemented (TPM, SEP, OS keychain,
  Argon2id)
- `SecretLifecycleBackend` trait with `DekRotation` as default
  implementation, slot for future `Puncturable` implementation
- DEK rotation is the rotation mechanism; the slot for puncturable
  encryption exists

**`ordo-secrets-broker`** â€” extended with:

- Full DRIFT three-stage (planner, validator, isolator) â€” was
  already in the draft
- Threshold dereference logic for `Threshold`-protected secret
  classes (SshClusterAdmin, SigningKey): dereference requires
  quorum signatures, not just a capability handle
- Canary token generation per capability and canary-detection in
  the isolator
- Structural output limits enforced per provider
- Chain-of-custody input hashing and verification

**`ordo-secrets-audit`** â€” extended with:

- Hash chain (was in draft)
- Anchor signing and local export (was in draft)
- `TransparencyService` trait with `LocalAnchor` as default, slot
  for external transparency service
- Cloud anchor export with in-vault cloud credentials
- Anchor verification tooling â€” not just internal verify, but an
  exported CLI or bus capability that third parties can use to
  verify a provided anchor + chain slice against a published
  public key

**New: `ordo-secrets-threshold`** â€” the FROST layer.

- Implements distributed key generation for threshold-protected
  classes
- Implements two-round FROST signing for dereference operations
- Holds share storage with the same sealing discipline as the
  vault (shares on laptop sealed by vault; shares on YubiKey held
  by YubiKey; paper share is user's responsibility)
- Implements recovery flow: if a share-holding device is lost,
  the remaining quorum can redistribute
- Nonce-safety discipline: single-use nonces enforced at the
  crate level, not left to callers

This crate lives alongside the vault as a peer, because
threshold-protected secrets have a different dereference path than
single-sealed ones. The broker routes to whichever backend the
secret class declares.

## Extensions to `ordo-protocol`

All previous additions stand (see the prior blueprint for the full
list â€” it is part of the handoff material alongside this
document). Additions for completeness:

**Threshold support:**

```rust
pub enum ProtectionLevel {
    SingleSealed,                           // standard vault
    Threshold { t: u32, n: u32 },           // t-of-n FROST
}

pub struct ThresholdShareAnnouncement {
    share_id: Ulid,
    secret_id: Ulid,
    holder_device_fingerprint: [u8; 32],
    share_index: u32,
    total_shares: u32,
}

pub struct ThresholdSigningRequest {
    operation_id: Ulid,
    secret_id: Ulid,
    nonce_commitments: Vec<NonceCommitment>,
    message_hash: [u8; 32],
}
```

**Canary and chain-of-custody:**

```rust
pub struct CapabilityCanary {
    capability_id: Ulid,
    canary_token: String,  // high-entropy, unique
    injected_into_context: bool,
}

pub struct InputCustody {
    tool_invocation_id: Ulid,
    input_hash: [u8; 32],
    declared_capabilities_hash: [u8; 32],
}
```

**Rotation primitives:**

```rust
pub trait AutoRotator {
    fn can_auto_rotate(&self) -> bool;
    fn rotate(&self, old: &SecureBytes) -> Result<SecureBytes, RotationError>;
}

pub struct RotationDue {
    secret_id: Ulid,
    class: SecretClass,
    reason: RotationReason,
    auto_rotator_available: bool,
}

pub enum RotationReason {
    ScheduledPolicy,
    ComplianceRequirement,
    SuspectedCompromise,
    UserRequested,
}
```

**Transparency pluggability:**

```rust
pub trait TransparencyService: Send + Sync {
    fn submit_anchor(&self, anchor: AnchorStatement) -> Result<Receipt, TransparencyError>;
    fn verify_receipt(&self, receipt: &Receipt) -> Result<VerificationStatus, TransparencyError>;
    fn export_format(&self) -> TransparencyExportFormat;
}

pub struct LocalAnchor;  // default impl, no external service
```

**Additional protocol invariants (20 + 4):**

The prior blueprint defined 20 secrets-specific protocol
invariants. These four are additions:

21. **Threshold-protected secrets require quorum to dereference.**
    No single device, including the broker's own machine, can
    dereference alone.
22. **Canary tokens are per-capability, not per-secret.** A
    canary that appears in a tool output proves that *that
    specific capability's* context leaked, not the underlying
    secret.
23. **Rotation that destroys the old secret material is the only
    valid rotation.** A "rotation" that leaves the old secret
    usable is renaming, not rotating.
24. **Transparency anchors are signed by keys sealed at Tier 1 or
    Tier 2 only.** A Tier-3 signed anchor has no external meaning.

## What's actually in â€” all of it

- Four sealing tiers, TPM and Secure Enclave included from day
  one
- DEK rotation for forward secrecy (puncturable encryption slot
  reserved)
- Full DRIFT isolator with pattern, structural,
  capability-plaintext, canary, custody, and structural-limit
  detection
- Hash chain with anchor signing
- Local anchor export AND cloud anchor export (cloud creds in
  vault)
- Pluggable transparency service interface with LocalAnchor as
  default
- FROST threshold protection for SshClusterAdmin, SigningKey
  classes
- Automated rotation architecture with per-provider AutoRotator
  implementations
- Rotation-due scheduling, emission, and handling
- Per-class rotation policy defaults committed to the repo
- Mock sealers for CI that are clearly distinct from production
  sealers
- Canary token injection and detection
- Chain-of-custody hashing for tool invocations
- Structural output limits enforced by broker
- Recovery flow for lost threshold shares

## What's deferred â€” two items, both for honest reasons

1. **Puncturable encryption (true form)** â€” no production Rust
   implementation. DEK rotation is the actual, real, correct
   implementation for today's Rust. The slot for puncturable
   encryption exists in the `SecretLifecycleBackend` trait; when
   a mature crate lands, the trait gets a new implementation and
   existing secrets transparently migrate. Until then, DEK
   rotation *is* the implementation, not a placeholder.

2. **Hardware CVM / CHERI** â€” the hardware doesn't exist on user
   machines. The software architecture already enforces the
   isolation that CVM would enforce at silicon level. When
   hardware is available, compartments become hardware-backed;
   the code boundaries don't move.

Neither of these is a "v2 feature." They're "this is the current
correct implementation; a better implementation will exist when
its dependencies exist." Different category from roadmap deferral.

## Everything else ships in the initial commit

That includes:

- Full TPM 2.0 integration with secure sessions (non-negotiable)
- Apple Secure Enclave integration
- FROST threshold with DKG and signing
- Canary tokens, structural limits, custody hashing
- Cloud anchor export
- Rotation architecture
- Transparency service pluggability
- Recovery flows for threshold shares

The blueprint is one document, not a roadmap. Claude Code builds
the whole thing. It's a bigger first session (or more likely, a
series of sessions, because this is substantial) â€” but everything
that belongs in the architecture is *in* the architecture, and the
cost of forgetting later goes to zero.

## Open questions (nine â€” all must have answers before
implementation)

The prior blueprint had eight questions. This revision adds a
ninth. All nine must be answered in this doc before any code is
written. The first eight are not repeated here; they live in the
prior blueprint material (the handoff should include that
document alongside this one).

1. **[from prior blueprint â€” PASTE ANSWER]**
2. **[from prior blueprint â€” PASTE ANSWER]**
3. **[from prior blueprint â€” PASTE ANSWER]**
4. **[from prior blueprint â€” PASTE ANSWER]**
5. **[from prior blueprint â€” PASTE ANSWER]**
6. **[from prior blueprint â€” PASTE ANSWER]**
7. **[from prior blueprint â€” PASTE ANSWER]**
8. **[from prior blueprint â€” PASTE ANSWER]**
9. **Threshold share distribution flow for initial setup.** When
   a user enables threshold protection for a secret class, how
   are the shares distributed to their devices? Suggest: vault
   generates the shares locally, user scans QR codes to move
   shares to phone/YubiKey, one share prints to paper with
   explicit instructions. This is a UX question that affects the
   crate's public API. Needs concrete answer before Claude Code
   writes the share-distribution code.

## The broader discipline

This correction applies to every future artifact. "v2," "later,"
"upgrade path," "phase 2" â€” these phrases are signals of
conventional project-management thinking overriding the actual
architectural principle, which is that parallel progress and
building-for-completeness-not-phases is how nothing gets lost to
time and growth.

The only legitimate reasons to *not* build something now:

- The dependencies don't exist yet (the library isn't written,
  the hardware doesn't ship)
- Building it would require changing something sacred (like
  `ordo-protocol`) in a way you haven't thought through
- It's genuinely not part of the architecture â€” it's scope creep
  from a different product

"It would take more work" is not a reason. "We can add it later"
is not a reason â€” it's the exact form of optimism that causes the
forgetting.

For the secrets work specifically: this is the moment. The
runtime is small. The contract is clean. Adding threshold
protection, canaries, custody hashing, rotation architecture,
cloud anchors â€” all of it is tractable now. The version of this
that exists a year from now without these pieces will be much
harder to retrofit, because production data, active providers,
and user expectations will all constrain the change.

## What needs to happen next

1. Fill in the eight question answers above (copy from the prior
   blueprint document).
2. Answer question 9.
3. Commit the completed doc.
4. Open the implementation session.

Until step 3 is done, this crate set stays unwritten. The
architecture contract
(`docs/architecture-contract.md`) Rule 11 applies: protocol
additions this significant require a deliberate spec, not
drive-by struct definitions. Treat this doc as the spec, treat
the open questions as constitutional amendments that require
debate, and only then does the implementation become a mechanical
translation of the doc into code.
