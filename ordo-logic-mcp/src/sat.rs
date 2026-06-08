//! varisat-backed SAT functions.
//!
//! Wraps the propositional AST + parser from `ordo-logic` (so syntax
//! stays consistent across runtime + MCP) and routes the heavy
//! lifting through varisat. The varisat solver works on CNF clauses,
//! so we Tseitin-transform our richer AST (NOT/AND/OR/IMPLIES/IFF)
//! before handing it off.
//!
//! Status: **scaffold**. The CNF transform + varisat plumbing is the
//! follow-up; for now this module returns `unimplemented` so the
//! crate compiles and the install/uninstall flow can be tested
//! end-to-end without the heavy dep work blocking.

use std::collections::BTreeMap;

use thiserror::Error;

use ordo_logic::propositional::Expr;

#[derive(Debug, Error)]
pub enum SatError {
    #[error("not yet implemented — install this MCP server is wired but the SAT handler is the next session")]
    NotImplemented,

    #[error("varisat error: {0}")]
    Solver(String),
}

#[derive(Debug, Clone)]
pub struct SatResult {
    pub satisfiable: bool,
    pub model: Option<BTreeMap<String, bool>>,
}

/// Check if a single propositional formula is satisfiable. Returns a
/// witness assignment when yes.
pub fn check_satisfiability(_formula: &Expr) -> Result<SatResult, SatError> {
    Err(SatError::NotImplemented)
}

/// Check whether a conjunction of premises entails a conclusion.
/// Equivalent to: is `(P1 ∧ P2 ∧ … ∧ PN) ∧ ¬C` unsatisfiable?
pub fn check_entailment(_premises: &[Expr], _conclusion: &Expr) -> Result<SatResult, SatError> {
    Err(SatError::NotImplemented)
}
