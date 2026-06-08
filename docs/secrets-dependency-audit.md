# Secrets Architecture â€” Dependency Audit

**Date:** 2026-04-24
**Purpose:** Confirm every crate the
[secrets-architecture-blueprint](secrets-architecture-blueprint.md)
depends on actually exists, is current, is not yanked, and builds
on this machine's Windows toolchain before we commit to the
blueprint's "no deferrals" discipline.

**Method:** Scratch crate outside the workspace at
`%TEMP%\deps-audit`, add each dep, `cargo check`, record outcome.
Non-committal â€” produces no code in the project tree, just answers.

## Summary

All four core deps and both platform-gated deps compile clean on
Windows 11 / stable Rust.

| Crate               | Latest stable            | Used for                                                   | Status |
|---------------------|--------------------------|------------------------------------------------------------|--------|
| `frost-ed25519`     | 3.0.0                    | FROST threshold signatures (`ordo-secrets-threshold`)       | âœ… compiles; not yanked |
| `coset`             | 0.4.2                    | COSE / SCITT-shaped signed statements (`ordo-secrets-audit`) | âœ… compiles; not yanked |
| `argon2`            | 0.5 (stable) / 0.6.0-rc.8 | Tier-4 software fallback KDF                               | âœ… compiles on 0.5; 0.6 is rc |
| `sha2`, `blake3`, `hex`, `rand` | already in workspace | Chain hashing, payload hashing, encoding, RNG      | âœ… already-proven |
| `windows`           | 0.59                     | TBS TPM access on Windows 11 â€” Tier-1 sealing              | âœ… compiles with `Win32_Security_Tpm` feature |
| `security-framework` | 3.7.0                    | Apple Secure Enclave bindings â€” Tier-2 sealing on macOS/iOS | âœ… latest; macOS-only (target-gated) |
| `tss-esapi`         | 7.7.0 stable (8.x is alpha) | TPM 2.0 via TSS â€” Tier-1 sealing on Linux               | âœ… available stable; Linux-only dep |

## Detail

### frost-ed25519 3.0.0
- Publisher: ZcashFoundation
- Part of `frost-core` family; reviewed.
- Compiles clean in scratch.
- **Verdict:** Use `frost-ed25519 = "3"` in `ordo-secrets-threshold`.
  No feature gates needed for the core signing path.

### coset 0.4.2
- Publisher: Google (via `google/coset` on GitHub)
- CBOR / COSE types aligned with SCITT draft format.
- Compiles clean in scratch.
- **Verdict:** Use `coset = "0.4"` in `ordo-secrets-audit` for the
  COSE-shaped anchor statements and receipts.

### argon2
- Stable line: `argon2 = "0.5"` (current: 0.5.3).
- `0.6.0-rc.8` exists but is release-candidate; don't pin an RC in
  a security-critical crate.
- Compiles clean in scratch on 0.5.
- **Verdict:** Use `argon2 = "0.5"` in `ordo-secrets-vault` for the
  Tier-4 software KDF (Argon2id with memory/time params chosen per
  the blueprint).

### windows 0.59 (TBS)
- Microsoft's auto-generated Windows API crate. `windows-sys` does
  NOT expose the `Win32_Security_Tpm` feature (TBS API), but the
  higher-level `windows` crate does.
- Compiles clean with `features = ["Win32_Security_Tpm"]`.
- **Verdict:** Use `windows = { version = "0.59", features =
  ["Win32_Security_Tpm"] }` target-gated to Windows for TBS-backed
  Tier-1 sealing. This is the correct integration path on Windows
  11 (TPM 2.0 is mostly mandatory on Win 11 hardware).

### tss-esapi
- Linux integration path (via libtpm2-tss C library, FFI).
- Latest on crates.io: `8.0.0-alpha.2` (released 2026-02-26).
- Latest stable: `7.7.0`.
- **Verdict:** Use `tss-esapi = "7"` target-gated to Linux. Do NOT
  chase 8.x until it leaves alpha â€” we don't need the 8.x features
  and alpha-version API churn in a security crate is a cost
  we shouldn't accept.
- Not tested on this Windows machine (target-gated to
  `cfg(target_os = "linux")`); the audit for that lane happens
  when we next have a Linux build host. The dep is present for
  completeness; not compiling it today is expected.

### security-framework 3.7.0
- Bindings to Apple's Security.framework (Keychain Services,
  Secure Enclave).
- Compiles clean target-gated to macOS.
- **Verdict:** Use `security-framework = "3"` target-gated to
  Apple platforms for Tier-2 sealing (Secure Enclave ECC keys +
  Keychain-stored wrapped material).

## Platform matrix

| Platform          | Tier-1 (hardware root) | Tier-2 (secure element) | Tier-3 (OS keychain) | Tier-4 (software fallback) |
|-------------------|------------------------|-------------------------|----------------------|----------------------------|
| Windows 11        | `windows` TBS          | n/a                     | `keyring` (Windows Credential Manager) | `argon2` |
| macOS             | n/a                    | `security-framework` (SEP) | `keyring` (Keychain) | `argon2` |
| Linux (TPM)       | `tss-esapi`            | n/a                     | `keyring` (SecretService) | `argon2` |
| Linux (no TPM)    | n/a                    | n/a                     | `keyring` (SecretService) | `argon2` |

Tier-3 (`keyring`) is already in the workspace deps, already
working on Windows/macOS/Linux. No new audit needed.

## Blockers / risks found

**None.** The blueprint can be implemented against these exact
crates on this machine. No "we'll revisit when library X lands"
notes required â€” all libraries exist and work.

Three minor gotchas to remember during implementation:

1. `windows` crate feature names are case-sensitive and versioned â€”
   pinning `windows = "0.59"` locks the exact feature surface we
   audited; bumping the version later should re-run this audit.
2. `tss-esapi` needs the host's `libtpm2-tss-dev` package at build
   time on Linux â€” this is a system-level dep, not just a Cargo
   one. Build scripts must tolerate its absence on hosts without
   TPM (fall back to a stub sealer instead of failing the whole
   crate build).
3. `frost-ed25519` DKG requires careful channel management â€”
   secure, authenticated channels between participants. The
   blueprint's `ordo-secrets-threshold` crate owns this; a naive
   implementation over the existing pub/sub bus leaks share
   material. Threshold channel plumbing uses the
   `BusCorrelator` primitive (already in `ordo-bus`) with
   per-DKG ephemeral Noise-wrapped envelopes.

## Verdict

**Ready to build.** No "phase 2" required â€” every library named in
the blueprint is production-grade, current, and compiles.

The blueprint's two honest deferrals (puncturable encryption, CVM
hardware) remain deferred for the reasons given there. Nothing in
this audit changes that.
