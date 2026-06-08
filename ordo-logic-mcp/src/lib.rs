//! ordo-logic-mcp — full SAT/SMT logic as an opt-in MCP server.
//!
//! Why this is a separate binary, not built into the runtime:
//!
//! - The Ordo runtime ships with a tiny truth-table propositional
//!   prover (~5 KB; capped at 16 variables; in `ordo-logic`). That's
//!   enough for syllogisms, simple chain validation, and contradiction
//!   detection in modest fact sets.
//! - Anything bigger needs a real SAT solver. `varisat` adds ~500 KB,
//!   which is fine — but most operators don't need formal proofs over
//!   1000-variable formulas. Shipping it built-in would tax everyone
//!   for a feature few use.
//! - The MCP install pattern Ordo already supports (Skills tab → MCP →
//!   Install) is the right vehicle: opt-in, signed, sandboxed,
//!   uninstallable.
//!
//! Capabilities this server exposes (over and above what the runtime
//! has built-in):
//!
//!   logic.satisfiability  — full SAT, arbitrary problem size
//!   logic.entailment      — full SAT, arbitrary problem size
//!   logic.equivalence     — pair of SAT calls
//!   logic.normalize       — CNF / DNF conversion
//!
//! When installed, the runtime's HybridLogicProvider would prefer
//! these over the truth-table baseline for any problem larger than
//! the in-runtime cap. (That preference wiring is a follow-up — the
//! seam is in place via `Arc<dyn LogicProvider>`.)
//!
//! ## Status
//!
//! **Scaffold only.** The Cargo manifest declares `varisat` as a
//! dep and the binary stub starts up cleanly. The actual SAT
//! handler implementation is the next session — the seam is here so
//! the install path can be tested end-to-end now.

pub mod sat;

pub use sat::{check_entailment, check_satisfiability, SatError, SatResult};
