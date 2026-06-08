//! Propositional logic — tiny self-contained prover for the runtime.
//!
//! Scope: zero-deps SAT-equivalent for small propositional formulas.
//! We enumerate all 2^n variable assignments and evaluate the formula
//! against each. That's truth-table semantics — guaranteed correct,
//! no solver to misconfigure, but capped: the eval bails on formulas
//! with more than [`MAX_VARS`] distinct variables. Larger problems
//! belong in `logic-mcp` (varisat / SMT) and route through the
//! external MCP install path the runtime already supports.
//!
//! Why this exists in the runtime instead of being deferred entirely:
//!
//! - Memory layer needs a fast contradiction check on the fact store
//!   (assistant.recall_memory, future planner integration). That's a
//!   hot path; an MCP round-trip is too heavy.
//! - Syllogism-scale validation (`validate_chain` over <8 premises) is
//!   the most common formal-logic ask, and 2^7 is 128 evaluations —
//!   trivial.
//! - Adds ~5 KB to ordo.exe vs. ~500 KB for varisat. We respect the
//!   "stay light" budget.
//!
//! What's deliberately NOT here:
//! - First-order logic (quantifiers). FOL goes in logic-mcp.
//! - SMT (theories — arithmetic, equality, etc.). Same.
//! - Variables beyond [`MAX_VARS`]. Returns `ProverError::TooLarge`.
//! - Any kind of CNF conversion / unit propagation / clause learning.
//!   We don't need them for problems this size.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hard cap on distinct variables. 2^16 = 65,536 truth-table rows;
/// fits easily under 100 ms on consumer hardware. Larger formulas
/// are flagged with `ProverError::TooLarge` so the caller can route
/// to a real SAT solver via logic-mcp.
pub const MAX_VARS: usize = 16;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProverError {
    #[error("parse error at position {pos}: {message}")]
    Parse { pos: usize, message: String },

    #[error("formula has {vars} variables; this prover caps at {MAX_VARS} (install logic-mcp for full SAT)")]
    TooLarge { vars: usize },

    #[error("empty formula")]
    Empty,
}

/// Propositional AST. Operators are: NOT, AND, OR, IMPLIES, IFF.
/// Operands are atomic variables (lowercase identifiers) or the
/// constants TRUE / FALSE.
///
/// We don't put a `#[serde(tag)]` on this because it has tuple
/// variants — serde rejects that combination. Default external
/// tagging works fine for the cases we serialize this (which is
/// rare; mostly used internally as the AST output of `parse`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    Var(String),
    Const(bool),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Implies(Box<Expr>, Box<Expr>),
    Iff(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Collect variable names in left-to-right first-occurrence order.
    pub fn variables(&self) -> Vec<String> {
        let mut seen = Vec::new();
        let mut acc = Vec::new();
        self.collect_vars(&mut seen, &mut acc);
        acc
    }

    fn collect_vars(&self, seen: &mut Vec<String>, acc: &mut Vec<String>) {
        match self {
            Expr::Var(v) => {
                if !seen.contains(v) {
                    seen.push(v.clone());
                    acc.push(v.clone());
                }
            }
            Expr::Const(_) => {}
            Expr::Not(e) => e.collect_vars(seen, acc),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Implies(a, b) | Expr::Iff(a, b) => {
                a.collect_vars(seen, acc);
                b.collect_vars(seen, acc);
            }
        }
    }

    /// Evaluate against a variable→bool assignment. Missing variables
    /// default to false (caller is responsible for completeness).
    pub fn eval(&self, env: &BTreeMap<String, bool>) -> bool {
        match self {
            Expr::Var(v) => *env.get(v).unwrap_or(&false),
            Expr::Const(b) => *b,
            Expr::Not(e) => !e.eval(env),
            Expr::And(a, b) => a.eval(env) && b.eval(env),
            Expr::Or(a, b) => a.eval(env) || b.eval(env),
            Expr::Implies(a, b) => !a.eval(env) || b.eval(env),
            Expr::Iff(a, b) => a.eval(env) == b.eval(env),
        }
    }
}

/// Result of an entailment check. `holds` mirrors `ChainValidation`'s
/// field name; `counterexample` (when present) is a witness assignment
/// where the premises are all true but the conclusion is false.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntailmentResult {
    pub holds: bool,
    #[serde(default)]
    pub counterexample: Option<BTreeMap<String, bool>>,
    /// Number of variable assignments enumerated. Useful for telemetry
    /// and as a sanity-check that we exercised the full space.
    pub assignments_checked: u32,
}

/// Enumerate every variable assignment, return the first one where
/// `f` evaluates to false (a counterexample to validity), or `None`
/// if the formula is a tautology.
fn find_falsifying_assignment(
    f: &Expr,
    vars: &[String],
) -> Result<(Option<BTreeMap<String, bool>>, u32), ProverError> {
    if vars.len() > MAX_VARS {
        return Err(ProverError::TooLarge { vars: vars.len() });
    }
    let total: u32 = if vars.is_empty() {
        1
    } else {
        1u32 << vars.len()
    };
    for assignment in 0..total {
        let mut env = BTreeMap::new();
        for (i, name) in vars.iter().enumerate() {
            let bit = (assignment >> i) & 1 == 1;
            env.insert(name.clone(), bit);
        }
        if !f.eval(&env) {
            return Ok((Some(env), assignment + 1));
        }
    }
    Ok((None, total))
}

/// Does the conjunction of `premises` entail `conclusion`?
///
/// Equivalent to checking whether `(p1 ∧ p2 ∧ … ∧ pN) → conclusion`
/// is a tautology. If yes, `holds: true`; if no, `holds: false` and
/// `counterexample` carries an assignment that makes the premises
/// true but the conclusion false.
pub fn entails(premises: &[Expr], conclusion: &Expr) -> Result<EntailmentResult, ProverError> {
    if premises.is_empty() && matches!(conclusion, Expr::Const(true)) {
        return Ok(EntailmentResult {
            holds: true,
            counterexample: None,
            assignments_checked: 1,
        });
    }
    // Build (P1 AND P2 AND ... AND PN) IMPLIES C as a single Expr,
    // then check it for tautology. Empty premises ⇒ formula reduces
    // to bare conclusion.
    let antecedent = premises
        .iter()
        .cloned()
        .reduce(|a, b| Expr::And(Box::new(a), Box::new(b)));
    let formula = match antecedent {
        Some(ant) => Expr::Implies(Box::new(ant), Box::new(conclusion.clone())),
        None => conclusion.clone(),
    };
    let vars = formula.variables();
    let (counter, checked) = find_falsifying_assignment(&formula, &vars)?;
    Ok(EntailmentResult {
        holds: counter.is_none(),
        counterexample: counter,
        assignments_checked: checked,
    })
}

/// Check whether a set of formulas is collectively satisfiable.
/// Used by future memory-layer contradiction detection: feed in the
/// fact store, get back `Some(witness)` if consistent, `None` if
/// contradictory.
pub fn satisfiable(formulas: &[Expr]) -> Result<Option<BTreeMap<String, bool>>, ProverError> {
    let conj = formulas
        .iter()
        .cloned()
        .reduce(|a, b| Expr::And(Box::new(a), Box::new(b)));
    let formula = match conj {
        Some(c) => c,
        None => return Ok(Some(BTreeMap::new())),
    };
    let vars = formula.variables();
    if vars.len() > MAX_VARS {
        return Err(ProverError::TooLarge { vars: vars.len() });
    }
    let total: u32 = if vars.is_empty() {
        1
    } else {
        1u32 << vars.len()
    };
    for assignment in 0..total {
        let mut env = BTreeMap::new();
        for (i, name) in vars.iter().enumerate() {
            let bit = (assignment >> i) & 1 == 1;
            env.insert(name.clone(), bit);
        }
        if formula.eval(&env) {
            return Ok(Some(env));
        }
    }
    Ok(None)
}

// ─── Parser ──────────────────────────────────────────────────────
//
// Accepts the vocabulary the LLM is most likely to emit when asked
// to formalize:
//
//   AND  &  ∧
//   OR   |  ∨
//   NOT  !  ¬  ~
//   IMPLIES  ->  →  =>
//   IFF  <->  ↔  <=>
//   TRUE / FALSE / 1 / 0
//   identifiers: [a-z_][a-z0-9_]*
//   parentheses
//
// Precedence (low → high): IFF, IMPLIES, OR, AND, NOT, ATOM.

pub fn parse(input: &str) -> Result<Expr, ProverError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ProverError::Empty);
    }
    let tokens = lex(trimmed)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_iff()?;
    if parser.pos < parser.tokens.len() {
        return Err(ProverError::Parse {
            pos: parser.pos,
            message: format!("unexpected token: {:?}", parser.tokens[parser.pos]),
        });
    }
    Ok(expr)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    Not,
    And,
    Or,
    Implies,
    Iff,
    True,
    False,
    Ident(String),
}

fn lex(input: &str) -> Result<Vec<Tok>, ProverError> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Multi-character punctuation (longest match first).
        if i + 2 < chars.len() && chars[i] == '<' && chars[i + 1] == '-' && chars[i + 2] == '>' {
            out.push(Tok::Iff);
            i += 3;
            continue;
        }
        if i + 2 < chars.len() && chars[i] == '<' && chars[i + 1] == '=' && chars[i + 2] == '>' {
            out.push(Tok::Iff);
            i += 3;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] == '>' {
            out.push(Tok::Implies);
            i += 2;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '=' && chars[i + 1] == '>' {
            out.push(Tok::Implies);
            i += 2;
            continue;
        }
        match c {
            '(' => out.push(Tok::LParen),
            ')' => out.push(Tok::RParen),
            '!' | '~' | '¬' => out.push(Tok::Not),
            '&' | '∧' => out.push(Tok::And),
            '|' | '∨' => out.push(Tok::Or),
            '→' => out.push(Tok::Implies),
            '↔' => out.push(Tok::Iff),
            // Identifier or keyword.
            ch if ch.is_alphabetic() || ch == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let upper = word.to_ascii_uppercase();
                let tok = match upper.as_str() {
                    "AND" => Tok::And,
                    "OR" => Tok::Or,
                    "NOT" => Tok::Not,
                    "IMPLIES" => Tok::Implies,
                    "IFF" => Tok::Iff,
                    "TRUE" => Tok::True,
                    "FALSE" => Tok::False,
                    _ => Tok::Ident(word.to_ascii_lowercase()),
                };
                out.push(tok);
                continue;
            }
            '0' => out.push(Tok::False),
            '1' => out.push(Tok::True),
            other => {
                return Err(ProverError::Parse {
                    pos: i,
                    message: format!("unexpected character {other:?}"),
                });
            }
        }
        i += 1;
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn consume(&mut self, expected: &Tok) -> Result<(), ProverError> {
        if self.peek() == Some(expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ProverError::Parse {
                pos: self.pos,
                message: format!("expected {expected:?}, got {:?}", self.peek()),
            })
        }
    }

    fn parse_iff(&mut self) -> Result<Expr, ProverError> {
        let mut left = self.parse_implies()?;
        while matches!(self.peek(), Some(Tok::Iff)) {
            self.pos += 1;
            let right = self.parse_implies()?;
            left = Expr::Iff(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_implies(&mut self) -> Result<Expr, ProverError> {
        // Right-associative: a -> b -> c ≡ a -> (b -> c)
        let left = self.parse_or()?;
        if matches!(self.peek(), Some(Tok::Implies)) {
            self.pos += 1;
            let right = self.parse_implies()?;
            return Ok(Expr::Implies(Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, ProverError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ProverError> {
        let mut left = self.parse_not()?;
        while matches!(self.peek(), Some(Tok::And)) {
            self.pos += 1;
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ProverError> {
        if matches!(self.peek(), Some(Tok::Not)) {
            self.pos += 1;
            let inner = self.parse_not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, ProverError> {
        match self.peek().cloned() {
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.parse_iff()?;
                self.consume(&Tok::RParen)?;
                Ok(inner)
            }
            Some(Tok::True) => {
                self.pos += 1;
                Ok(Expr::Const(true))
            }
            Some(Tok::False) => {
                self.pos += 1;
                Ok(Expr::Const(false))
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                Ok(Expr::Var(name))
            }
            other => Err(ProverError::Parse {
                pos: self.pos,
                message: format!("expected atom, got {other:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Expr {
        parse(s).expect("parse")
    }

    #[test]
    fn parses_basic_formulas() {
        assert_eq!(p("p"), Expr::Var("p".into()));
        assert_eq!(p("TRUE"), Expr::Const(true));
        assert_eq!(p("!p"), Expr::Not(Box::new(Expr::Var("p".into()))));
    }

    #[test]
    fn parses_unicode_operators() {
        // `¬p ∧ (q ∨ r) → s`
        let e = parse("¬p ∧ (q ∨ r) → s").expect("parse");
        let vars = e.variables();
        assert_eq!(vars, vec!["p", "q", "r", "s"]);
    }

    #[test]
    fn ascii_keywords_work() {
        let e = parse("NOT p AND (q OR r) IMPLIES s").expect("parse");
        let vars = e.variables();
        assert_eq!(vars, vec!["p", "q", "r", "s"]);
    }

    #[test]
    fn proves_modus_ponens() {
        // p, p -> q ⊢ q
        let premises = vec![p("p"), p("p -> q")];
        let conclusion = p("q");
        let r = entails(&premises, &conclusion).expect("eval");
        assert!(r.holds);
        assert!(r.counterexample.is_none());
    }

    #[test]
    fn rejects_affirming_consequent() {
        // p -> q, q ⊬ p (affirming the consequent)
        let premises = vec![p("p -> q"), p("q")];
        let conclusion = p("p");
        let r = entails(&premises, &conclusion).expect("eval");
        assert!(!r.holds);
        let cx = r.counterexample.expect("counterexample");
        // p=false, q=true falsifies the chain.
        assert_eq!(cx.get("p"), Some(&false));
        assert_eq!(cx.get("q"), Some(&true));
    }

    #[test]
    fn proves_contrapositive() {
        // p -> q, !q ⊢ !p (modus tollens)
        let premises = vec![p("p -> q"), p("!q")];
        let conclusion = p("!p");
        assert!(entails(&premises, &conclusion).expect("eval").holds);
    }

    #[test]
    fn satisfiable_consistent_set() {
        let s = satisfiable(&[p("p"), p("q"), p("p -> q")]).expect("sat");
        assert!(s.is_some());
    }

    #[test]
    fn detects_contradiction() {
        let s = satisfiable(&[p("p"), p("!p")]).expect("sat");
        assert!(s.is_none());
    }

    #[test]
    fn flags_too_large_formulas() {
        // 17 variables → over the cap.
        let big = (0..17)
            .map(|i| format!("v{i}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let e = parse(&big).expect("parse");
        assert!(matches!(
            entails(&[], &e),
            Err(ProverError::TooLarge { vars: 17 })
        ));
    }

    #[test]
    fn iff_works() {
        // p <-> p is a tautology.
        let r = entails(&[], &p("p <-> p")).expect("eval");
        assert!(r.holds);
        // p <-> q is not.
        let r = entails(&[], &p("p <-> q")).expect("eval");
        assert!(!r.holds);
    }

    #[test]
    fn precedence_correct() {
        // `!p AND q` should parse as `(!p) AND q`, not `!(p AND q)`.
        // Verify by checking the AST shape: the root must be And,
        // with the left side being Not(Var("p")). If the parse went
        // wrong it would be Not(And(...)) instead.
        let e = parse("!p AND q").expect("parse");
        match e {
            Expr::And(left, right) => {
                assert!(matches!(*left, Expr::Not(_)), "left should be Not(p)");
                assert_eq!(*right, Expr::Var("q".into()));
            }
            other => panic!("expected And at root, got {other:?}"),
        }
        // And confirm the formula is satisfiable (p=false, q=true)
        // but not a tautology.
        let r = entails(&[], &parse("!p AND q").unwrap()).expect("eval");
        assert!(!r.holds);
        let s = satisfiable(&[parse("!p AND q").unwrap()]).expect("sat");
        let env = s.expect("at least one satisfying assignment");
        assert_eq!(env.get("p"), Some(&false));
        assert_eq!(env.get("q"), Some(&true));
    }
}
