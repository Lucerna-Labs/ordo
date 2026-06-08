//! First-order logic — bounded grounding into the propositional layer.
//!
//! ## Why bounded
//!
//! Full FOL satisfiability is undecidable. Over a *finite* domain, FOL
//! reduces to propositional logic by grounding: replace `∀x. P(x)`
//! over domain `{a, b, c}` with the conjunction `P(a) ∧ P(b) ∧ P(c)`,
//! `∃x. P(x)` with the disjunction. The result is a propositional
//! formula the existing truth-table prover handles.
//!
//! That's enough for the kinds of arguments operators actually want
//! to verify: syllogisms, class-membership chains, transitivity over
//! a small named set. "All birds have feathers, penguins are birds,
//! therefore penguins have feathers" works because the domain
//! collapses to `{penguin}` (the only ground term mentioned), grounds
//! to two propositional atoms (`Bird(penguin)`, `Feathered(penguin)`),
//! and the truth-table prover dispatches in microseconds.
//!
//! What this module *won't* attempt: arithmetic, equality reasoning
//! over an unbounded domain, function symbols with non-constant
//! ranges, modal operators. Those go in `logic-mcp` (eventually z3
//! or similar).
//!
//! ## Surface
//!
//! - [`Term`] — atomic terms: variables (bound by a quantifier) and
//!   constants (ground names that pin down the domain).
//! - [`FolExpr`] — the AST: predicates, equality, the usual logical
//!   connectives, plus `Forall` / `Exists`.
//! - [`parse`] — operator-friendly textual parser. Accepts both ASCII
//!   keywords (`forall x. Bird(x) -> Feathered(x)`) and Unicode
//!   symbols (`∀x. Bird(x) → Feathered(x)`).
//! - [`ground`] — given an FOL formula and a domain, produce a
//!   propositional [`Expr`](crate::propositional::Expr) suitable for
//!   the truth-table prover. Atomic groundings get stable propositional
//!   variable names like `Bird(penguin)`.
//! - [`entails`] — top-level convenience: ground premises + conclusion
//!   over the union of all ground constants they mention, run the
//!   propositional prover, return the result.
//!
//! ## Bounds
//!
//! - Domain size cap: [`MAX_DOMAIN`]. With nested quantifiers, an
//!   FOL formula's grounded size is `O(domain^arity * subformulas)`,
//!   so even `MAX_DOMAIN = 4` with a binary predicate quantified
//!   twice gives 16 atoms — already pushing the propositional 16-var
//!   cap. This is intentional. Operators dealing with bigger universes
//!   should install logic-mcp.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::propositional::{self, Expr as PExpr};

/// Maximum domain size we'll ground over. The grounded formula's
/// variable count is bounded by `predicates * domain^max_arity`, and
/// we want to stay under the propositional prover's 16-variable cap
/// for typical syllogism-scale inputs.
pub const MAX_DOMAIN: usize = 6;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FolError {
    #[error("parse error at position {pos}: {message}")]
    Parse { pos: usize, message: String },

    #[error("empty formula")]
    Empty,

    #[error(
        "domain has {size} elements; cap is {MAX_DOMAIN} (install logic-mcp for unbounded FOL)"
    )]
    DomainTooLarge { size: usize },

    #[error("grounding produced too many atoms ({atoms}); install logic-mcp for unbounded FOL")]
    TooManyAtoms { atoms: usize },

    #[error("propositional layer error: {0}")]
    Propositional(#[from] propositional::ProverError),

    #[error("unbound variable in formula: {0}")]
    UnboundVariable(String),
}

/// A first-order term. Either a quantified variable (`x` inside the
/// scope of `∀x.` or `∃x.`) or a ground constant (`penguin`, `eagle`,
/// any name that doesn't get bound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Term {
    Var(String),
    Const(String),
}

impl Term {
    /// Underlying identifier — the variable name for `Var(name)`
    /// or the constant name for `Const(name)`. Public so callers
    /// outside this crate can introspect a parsed term without
    /// pattern-matching the enum manually.
    pub fn name(&self) -> &str {
        match self {
            Term::Var(n) | Term::Const(n) => n,
        }
    }
}

/// First-order formula AST.
///
/// `Predicate` accepts a name and a list of argument terms. We treat
/// 0-ary predicates ("propositions") as a special case: the parser
/// accepts `Bird` as a 0-ary predicate the same way it accepts
/// `Bird(x)` as 1-ary. That way pure-propositional formulas parse
/// naturally as FOL with no quantifiers and no arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolExpr {
    Predicate(String, Vec<Term>),
    Equal(Term, Term),
    Const(bool),
    Not(Box<FolExpr>),
    And(Box<FolExpr>, Box<FolExpr>),
    Or(Box<FolExpr>, Box<FolExpr>),
    Implies(Box<FolExpr>, Box<FolExpr>),
    Iff(Box<FolExpr>, Box<FolExpr>),
    Forall(String, Box<FolExpr>),
    Exists(String, Box<FolExpr>),
}

impl FolExpr {
    /// Walk the AST, return every Term::Const name. These are the
    /// ground constants that define the domain when no domain is
    /// supplied explicitly.
    pub fn constants(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.collect_consts(&mut out);
        out
    }

    fn collect_consts(&self, out: &mut BTreeSet<String>) {
        match self {
            FolExpr::Predicate(_, args) => {
                for a in args {
                    if let Term::Const(c) = a {
                        out.insert(c.clone());
                    }
                }
            }
            FolExpr::Equal(a, b) => {
                if let Term::Const(c) = a {
                    out.insert(c.clone());
                }
                if let Term::Const(c) = b {
                    out.insert(c.clone());
                }
            }
            FolExpr::Const(_) => {}
            FolExpr::Not(e) => e.collect_consts(out),
            FolExpr::And(a, b)
            | FolExpr::Or(a, b)
            | FolExpr::Implies(a, b)
            | FolExpr::Iff(a, b) => {
                a.collect_consts(out);
                b.collect_consts(out);
            }
            FolExpr::Forall(_, body) | FolExpr::Exists(_, body) => {
                body.collect_consts(out);
            }
        }
    }
}

// ─── Grounding ───────────────────────────────────────────────────
//
// Convert FOL → propositional by enumerating quantifiers over the
// supplied domain. Every grounded predicate becomes a propositional
// variable whose name encodes the predicate name + argument constants
// in a stable order: `Bird(penguin)`, `Loves(alice,bob)`. The
// propositional prover treats those names as opaque atoms.

/// Substitute a variable with a constant throughout a formula. Used
/// while grounding quantifiers: each quantifier eliminator picks one
/// domain element at a time and substitutes.
fn substitute(expr: &FolExpr, var: &str, value: &str) -> FolExpr {
    let sub_term = |t: &Term| -> Term {
        match t {
            Term::Var(n) if n == var => Term::Const(value.to_string()),
            other => other.clone(),
        }
    };
    match expr {
        FolExpr::Predicate(name, args) => {
            FolExpr::Predicate(name.clone(), args.iter().map(sub_term).collect())
        }
        FolExpr::Equal(a, b) => FolExpr::Equal(sub_term(a), sub_term(b)),
        FolExpr::Const(b) => FolExpr::Const(*b),
        FolExpr::Not(e) => FolExpr::Not(Box::new(substitute(e, var, value))),
        FolExpr::And(a, b) => FolExpr::And(
            Box::new(substitute(a, var, value)),
            Box::new(substitute(b, var, value)),
        ),
        FolExpr::Or(a, b) => FolExpr::Or(
            Box::new(substitute(a, var, value)),
            Box::new(substitute(b, var, value)),
        ),
        FolExpr::Implies(a, b) => FolExpr::Implies(
            Box::new(substitute(a, var, value)),
            Box::new(substitute(b, var, value)),
        ),
        FolExpr::Iff(a, b) => FolExpr::Iff(
            Box::new(substitute(a, var, value)),
            Box::new(substitute(b, var, value)),
        ),
        FolExpr::Forall(v, body) if v == var => {
            // Inner quantifier shadows; don't substitute inside it.
            FolExpr::Forall(v.clone(), body.clone())
        }
        FolExpr::Exists(v, body) if v == var => FolExpr::Exists(v.clone(), body.clone()),
        FolExpr::Forall(v, body) => {
            FolExpr::Forall(v.clone(), Box::new(substitute(body, var, value)))
        }
        FolExpr::Exists(v, body) => {
            FolExpr::Exists(v.clone(), Box::new(substitute(body, var, value)))
        }
    }
}

/// Stable propositional variable name for a grounded predicate. We
/// keep the original-case predicate name and join args with commas,
/// so `Bird(penguin)` becomes the literal string `Bird(penguin)`.
/// The propositional layer's parser accepts identifiers but not
/// parenthesized atoms; we don't go through the parser — we build
/// the propositional `Expr` directly.
fn ground_atom_name(predicate: &str, args: &[Term]) -> Result<String, FolError> {
    let mut out = predicate.to_string();
    if args.is_empty() {
        return Ok(out);
    }
    out.push('(');
    for (i, t) in args.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match t {
            Term::Const(c) => out.push_str(c),
            Term::Var(v) => return Err(FolError::UnboundVariable(v.clone())),
        }
    }
    out.push(')');
    Ok(out)
}

/// Recursively ground an FOL formula into propositional form. Returns
/// a `propositional::Expr` whose `Var(name)` leaves are stringified
/// grounded predicates.
pub fn ground(expr: &FolExpr, domain: &[String]) -> Result<PExpr, FolError> {
    if domain.len() > MAX_DOMAIN {
        return Err(FolError::DomainTooLarge { size: domain.len() });
    }
    match expr {
        FolExpr::Predicate(name, args) => {
            let var_name = ground_atom_name(name, args)?;
            Ok(PExpr::Var(var_name))
        }
        FolExpr::Equal(a, b) => {
            // Treat equality syntactically: c = c is true, c = d
            // (different consts) is false. No equality reasoning
            // beyond that — that needs SMT.
            match (a, b) {
                (Term::Const(x), Term::Const(y)) => Ok(PExpr::Const(x == y)),
                (Term::Var(v), _) | (_, Term::Var(v)) => Err(FolError::UnboundVariable(v.clone())),
            }
        }
        FolExpr::Const(b) => Ok(PExpr::Const(*b)),
        FolExpr::Not(e) => Ok(PExpr::Not(Box::new(ground(e, domain)?))),
        FolExpr::And(a, b) => Ok(PExpr::And(
            Box::new(ground(a, domain)?),
            Box::new(ground(b, domain)?),
        )),
        FolExpr::Or(a, b) => Ok(PExpr::Or(
            Box::new(ground(a, domain)?),
            Box::new(ground(b, domain)?),
        )),
        FolExpr::Implies(a, b) => Ok(PExpr::Implies(
            Box::new(ground(a, domain)?),
            Box::new(ground(b, domain)?),
        )),
        FolExpr::Iff(a, b) => Ok(PExpr::Iff(
            Box::new(ground(a, domain)?),
            Box::new(ground(b, domain)?),
        )),
        FolExpr::Forall(var, body) => {
            // ∀x. φ(x) over {a, b, c} ≡ φ(a) ∧ φ(b) ∧ φ(c)
            // Empty domain: vacuously true.
            if domain.is_empty() {
                return Ok(PExpr::Const(true));
            }
            let mut acc: Option<PExpr> = None;
            for value in domain {
                let substituted = substitute(body, var, value);
                let grounded = ground(&substituted, domain)?;
                acc = Some(match acc {
                    None => grounded,
                    Some(prev) => PExpr::And(Box::new(prev), Box::new(grounded)),
                });
            }
            Ok(acc.unwrap_or(PExpr::Const(true)))
        }
        FolExpr::Exists(var, body) => {
            // ∃x. φ(x) over {a, b, c} ≡ φ(a) ∨ φ(b) ∨ φ(c)
            // Empty domain: vacuously false.
            if domain.is_empty() {
                return Ok(PExpr::Const(false));
            }
            let mut acc: Option<PExpr> = None;
            for value in domain {
                let substituted = substitute(body, var, value);
                let grounded = ground(&substituted, domain)?;
                acc = Some(match acc {
                    None => grounded,
                    Some(prev) => PExpr::Or(Box::new(prev), Box::new(grounded)),
                });
            }
            Ok(acc.unwrap_or(PExpr::Const(false)))
        }
    }
}

/// FOL entailment: do the premises imply the conclusion?
///
/// Domain is the union of ground constants the formulas mention. If
/// the result is decidably valid over that domain, the answer is
/// formally proved (universal in the bounded sense). Caller is
/// responsible for understanding the semantics: "valid over this
/// finite domain" is not the same as "valid over all possible
/// universes" — but for the syllogism-scale arguments operators ask
/// about, the two coincide.
pub fn entails(premises: &[FolExpr], conclusion: &FolExpr) -> Result<EntailmentResult, FolError> {
    // Union of constants from every formula, plus a synthetic witness
    // if no constants are mentioned (so quantifiers don't ground to
    // the empty domain — which would make ∀ vacuously true and ∃
    // vacuously false).
    let mut domain: BTreeSet<String> = BTreeSet::new();
    for p in premises {
        domain.extend(p.constants());
    }
    domain.extend(conclusion.constants());
    if domain.is_empty() {
        // Pure-quantified formula with no constants. Add a single
        // witness so quantifiers have something to range over.
        domain.insert("_witness_".to_string());
    }
    let domain: Vec<String> = domain.into_iter().collect();
    if domain.len() > MAX_DOMAIN {
        return Err(FolError::DomainTooLarge { size: domain.len() });
    }

    let mut grounded_premises = Vec::with_capacity(premises.len());
    for p in premises {
        grounded_premises.push(ground(p, &domain)?);
    }
    let grounded_conclusion = ground(conclusion, &domain)?;

    // Check we haven't exploded past the propositional cap. The
    // propositional layer surfaces this on its own, but we can give
    // a friendlier error here.
    let antecedent: PExpr = grounded_premises
        .iter()
        .cloned()
        .reduce(|a, b| PExpr::And(Box::new(a), Box::new(b)))
        .unwrap_or(PExpr::Const(true));
    let combined = PExpr::Implies(Box::new(antecedent), Box::new(grounded_conclusion.clone()));
    let var_count = combined.variables().len();
    if var_count > propositional::MAX_VARS {
        return Err(FolError::TooManyAtoms { atoms: var_count });
    }

    let prop_result = propositional::entails(&grounded_premises, &grounded_conclusion)?;
    Ok(EntailmentResult {
        holds: prop_result.holds,
        counterexample: prop_result.counterexample,
        domain,
        atom_count: var_count,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntailmentResult {
    pub holds: bool,
    #[serde(default)]
    pub counterexample: Option<BTreeMap<String, bool>>,
    pub domain: Vec<String>,
    pub atom_count: usize,
}

// ─── Parser ──────────────────────────────────────────────────────
//
// Accepts the syntax the LLM is most likely to emit when asked to
// formalize:
//
//   forall x. φ           ∀x. φ
//   exists x. φ           ∃x. φ
//   Bird(x)               (predicate, 1-ary)
//   Bird                  (0-ary predicate / proposition)
//   Loves(alice, bob)     (2-ary)
//   x = y                 (equality, only between Term::Const at
//                         ground time — variables get substituted
//                         away during grounding)
//   AND OR NOT IMPLIES IFF, with symbols ∧ ∨ ¬ → ↔
//   parens for grouping
//
// Predicate / function names: any identifier (case-insensitive).
// Terms: lowercase identifiers are variables when bound by a
// quantifier in scope; otherwise they're constants. A variable is
// "in scope" when we're inside the body of a forall/exists that
// declared it.

pub fn parse(input: &str) -> Result<FolExpr, FolError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(FolError::Empty);
    }
    let tokens = lex(trimmed)?;
    let mut parser = Parser {
        tokens,
        pos: 0,
        bound_vars: Vec::new(),
    };
    let expr = parser.parse_iff()?;
    if parser.pos < parser.tokens.len() {
        return Err(FolError::Parse {
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
    Comma,
    Dot,
    Eq,
    Not,
    And,
    Or,
    Implies,
    Iff,
    Forall,
    Exists,
    True,
    False,
    Ident(String),
}

fn lex(input: &str) -> Result<Vec<Tok>, FolError> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Multi-char punctuation, longest-first.
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
            ',' => out.push(Tok::Comma),
            '.' => out.push(Tok::Dot),
            '=' => out.push(Tok::Eq),
            '!' | '~' | '¬' => out.push(Tok::Not),
            '&' | '∧' => out.push(Tok::And),
            '|' | '∨' => out.push(Tok::Or),
            '→' => out.push(Tok::Implies),
            '↔' => out.push(Tok::Iff),
            '∀' => out.push(Tok::Forall),
            '∃' => out.push(Tok::Exists),
            ch if ch.is_alphabetic() || ch == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let lower = word.to_ascii_lowercase();
                let tok = match lower.as_str() {
                    "and" => Tok::And,
                    "or" => Tok::Or,
                    "not" => Tok::Not,
                    "implies" => Tok::Implies,
                    "iff" => Tok::Iff,
                    "true" => Tok::True,
                    "false" => Tok::False,
                    "forall" | "all" | "every" => Tok::Forall,
                    "exists" | "some" => Tok::Exists,
                    _ => Tok::Ident(word),
                };
                out.push(tok);
                continue;
            }
            other => {
                return Err(FolError::Parse {
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
    /// Stack of currently in-scope quantified variables. When parsing
    /// a term like `x` inside `forall x. Bird(x)`, we look this up to
    /// decide whether `x` is a Var (bound) or a Const (free name).
    bound_vars: Vec<String>,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn advance(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn consume(&mut self, expected: &Tok) -> Result<(), FolError> {
        if self.peek() == Some(expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err(FolError::Parse {
                pos: self.pos,
                message: format!("expected {expected:?}, got {:?}", self.peek()),
            })
        }
    }

    fn parse_iff(&mut self) -> Result<FolExpr, FolError> {
        let mut left = self.parse_implies()?;
        while matches!(self.peek(), Some(Tok::Iff)) {
            self.pos += 1;
            let right = self.parse_implies()?;
            left = FolExpr::Iff(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_implies(&mut self) -> Result<FolExpr, FolError> {
        let left = self.parse_or()?;
        if matches!(self.peek(), Some(Tok::Implies)) {
            self.pos += 1;
            let right = self.parse_implies()?;
            return Ok(FolExpr::Implies(Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<FolExpr, FolError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = FolExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<FolExpr, FolError> {
        let mut left = self.parse_unary()?;
        while matches!(self.peek(), Some(Tok::And)) {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = FolExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<FolExpr, FolError> {
        match self.peek() {
            Some(Tok::Not) => {
                self.pos += 1;
                let inner = self.parse_unary()?;
                Ok(FolExpr::Not(Box::new(inner)))
            }
            Some(Tok::Forall) => {
                // Quantifiers have the LOWEST precedence — the body
                // extends as far right as the surrounding context
                // allows. Parsing `parse_iff` (the top of the
                // precedence chain) means `forall x. Bird(x) ->
                // Feathered(x)` parses as Forall(x, Bird(x) ->
                // Feathered(x)), not (Forall(x, Bird(x))) -> ….
                // Operator-friendly: matches how the LLM and most
                // logic textbooks phrase it.
                self.pos += 1;
                let var = self.parse_quant_var()?;
                self.bound_vars.push(var.clone());
                let body = self.parse_iff()?;
                self.bound_vars.pop();
                Ok(FolExpr::Forall(var, Box::new(body)))
            }
            Some(Tok::Exists) => {
                self.pos += 1;
                let var = self.parse_quant_var()?;
                self.bound_vars.push(var.clone());
                let body = self.parse_iff()?;
                self.bound_vars.pop();
                Ok(FolExpr::Exists(var, Box::new(body)))
            }
            _ => self.parse_atom(),
        }
    }

    /// Parse the variable name + optional `.` after a quantifier.
    /// Accepts both `forall x. body` and `forall x body` (LLMs vary).
    fn parse_quant_var(&mut self) -> Result<String, FolError> {
        let var = match self.advance() {
            Some(Tok::Ident(name)) => name,
            other => {
                return Err(FolError::Parse {
                    pos: self.pos,
                    message: format!("expected variable after quantifier, got {other:?}"),
                });
            }
        };
        // Optional dot separator.
        if matches!(self.peek(), Some(Tok::Dot)) {
            self.pos += 1;
        }
        Ok(var)
    }

    fn parse_atom(&mut self) -> Result<FolExpr, FolError> {
        match self.peek().cloned() {
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.parse_iff()?;
                self.consume(&Tok::RParen)?;
                Ok(inner)
            }
            Some(Tok::True) => {
                self.pos += 1;
                Ok(FolExpr::Const(true))
            }
            Some(Tok::False) => {
                self.pos += 1;
                Ok(FolExpr::Const(false))
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                // Three forms:
                //   Bird(x)          — predicate with args
                //   Bird             — 0-ary predicate
                //   x = penguin      — equality (we look ahead for `=`)
                if matches!(self.peek(), Some(Tok::LParen)) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            let term = self.parse_term()?;
                            args.push(term);
                            if matches!(self.peek(), Some(Tok::Comma)) {
                                self.pos += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    self.consume(&Tok::RParen)?;
                    return Ok(FolExpr::Predicate(name, args));
                }
                if matches!(self.peek(), Some(Tok::Eq)) {
                    self.pos += 1;
                    let rhs = self.parse_term()?;
                    let lhs = self.term_for_ident(&name);
                    return Ok(FolExpr::Equal(lhs, rhs));
                }
                // Bare identifier — treat as 0-ary predicate.
                Ok(FolExpr::Predicate(name, Vec::new()))
            }
            other => Err(FolError::Parse {
                pos: self.pos,
                message: format!("expected atom, got {other:?}"),
            }),
        }
    }

    fn parse_term(&mut self) -> Result<Term, FolError> {
        match self.advance() {
            Some(Tok::Ident(name)) => Ok(self.term_for_ident(&name)),
            other => Err(FolError::Parse {
                pos: self.pos,
                message: format!("expected term, got {other:?}"),
            }),
        }
    }

    /// Resolve a bare identifier inside a term position: if a
    /// quantifier in scope has bound this name, it's a Var; otherwise
    /// it's a ground Const.
    fn term_for_ident(&self, name: &str) -> Term {
        if self.bound_vars.iter().any(|v| v == name) {
            Term::Var(name.to_string())
        } else {
            Term::Const(name.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> FolExpr {
        parse(s).unwrap_or_else(|e| panic!("parse failed for {s:?}: {e}"))
    }

    #[test]
    fn parses_zero_ary_predicate() {
        // No quantifiers, no args — pure-propositional case as FOL.
        match p("Bird") {
            FolExpr::Predicate(name, args) => {
                assert_eq!(name, "Bird");
                assert!(args.is_empty());
            }
            other => panic!("expected predicate, got {other:?}"),
        }
    }

    #[test]
    fn parses_predicate_with_args() {
        match p("Loves(alice, bob)") {
            FolExpr::Predicate(name, args) => {
                assert_eq!(name, "Loves");
                assert_eq!(args.len(), 2);
                // Outside any quantifier, both are constants.
                assert_eq!(args[0], Term::Const("alice".into()));
                assert_eq!(args[1], Term::Const("bob".into()));
            }
            other => panic!("expected predicate, got {other:?}"),
        }
    }

    #[test]
    fn parses_quantified_with_correct_var_resolution() {
        // x is bound; penguin is free → Const.
        match p("forall x. Bird(x)") {
            FolExpr::Forall(var, body) => {
                assert_eq!(var, "x");
                match *body {
                    FolExpr::Predicate(name, args) => {
                        assert_eq!(name, "Bird");
                        assert_eq!(args, vec![Term::Var("x".into())]);
                    }
                    other => panic!("expected predicate body, got {other:?}"),
                }
            }
            other => panic!("expected forall, got {other:?}"),
        }
        match p("Bird(penguin)") {
            FolExpr::Predicate(_, args) => {
                assert_eq!(args, vec![Term::Const("penguin".into())]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parses_unicode_quantifiers() {
        let e = p("∀x. Bird(x) → Feathered(x)");
        match e {
            FolExpr::Forall(var, _) => assert_eq!(var, "x"),
            other => panic!("expected forall, got {other:?}"),
        }
    }

    #[test]
    fn ground_substitutes_bound_vars() {
        // forall x. Bird(x) over {penguin} → Bird(penguin)
        let e = p("forall x. Bird(x)");
        let domain = vec!["penguin".to_string()];
        let g = ground(&e, &domain).expect("ground");
        match g {
            PExpr::Var(name) => assert_eq!(name, "Bird(penguin)"),
            other => panic!("expected single grounded var, got {other:?}"),
        }
    }

    #[test]
    fn ground_two_element_domain_yields_conjunction() {
        // forall x. Bird(x) over {penguin, eagle}
        //   → Bird(penguin) AND Bird(eagle)
        let e = p("forall x. Bird(x)");
        let domain = vec!["penguin".to_string(), "eagle".to_string()];
        let g = ground(&e, &domain).expect("ground");
        match g {
            PExpr::And(a, b) => {
                assert_eq!(*a, PExpr::Var("Bird(penguin)".into()));
                assert_eq!(*b, PExpr::Var("Bird(eagle)".into()));
            }
            other => panic!("expected conjunction, got {other:?}"),
        }
    }

    #[test]
    fn ground_existential_yields_disjunction() {
        let e = p("exists x. Bird(x)");
        let domain = vec!["penguin".to_string(), "eagle".to_string()];
        let g = ground(&e, &domain).expect("ground");
        match g {
            PExpr::Or(_, _) => {}
            other => panic!("expected disjunction, got {other:?}"),
        }
    }

    #[test]
    fn entails_proves_penguin_syllogism() {
        // The motivating example from the previous session: this should
        // now prove Formal where the propositional-only path punted.
        let p1 = p("forall x. Bird(x) -> Feathered(x)");
        let p2 = p("Bird(penguin)");
        let conclusion = p("Feathered(penguin)");
        let r = entails(&[p1, p2], &conclusion).expect("entails");
        assert!(r.holds, "penguin syllogism should prove formally");
        assert!(r.counterexample.is_none());
        assert_eq!(r.domain, vec!["penguin".to_string()]);
    }

    #[test]
    fn entails_proves_two_constant_syllogism() {
        // All birds have feathers; penguins and eagles are birds;
        // therefore both have feathers.
        let p1 = p("forall x. Bird(x) -> Feathered(x)");
        let p2 = p("Bird(penguin)");
        let p3 = p("Bird(eagle)");
        let c = p("Feathered(penguin) AND Feathered(eagle)");
        let r = entails(&[p1, p2, p3], &c).expect("entails");
        assert!(r.holds);
    }

    #[test]
    fn entails_rejects_non_sequitur() {
        // Bird(penguin), but no premise links Bird → Feathered.
        // Feathered(penguin) does NOT follow.
        let p1 = p("Bird(penguin)");
        let conclusion = p("Feathered(penguin)");
        let r = entails(&[p1], &conclusion).expect("entails");
        assert!(!r.holds);
        assert!(r.counterexample.is_some(), "should produce counterexample");
    }

    #[test]
    fn entails_handles_existential_in_premise() {
        // exists x. Bird(x), and forall x. Bird(x) -> Feathered(x).
        // Conclude: exists x. Feathered(x). Holds.
        // Domain has no constants; we synthesize a witness.
        let p1 = p("exists x. Bird(x)");
        let p2 = p("forall x. Bird(x) -> Feathered(x)");
        let c = p("exists x. Feathered(x)");
        let r = entails(&[p1, p2], &c).expect("entails");
        assert!(r.holds);
    }

    #[test]
    fn flags_domain_too_large() {
        // Manually build a formula mentioning 7 constants.
        let mut formula = p("True");
        for name in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta"] {
            let pred = FolExpr::Predicate("P".into(), vec![Term::Const(name.into())]);
            formula = FolExpr::And(Box::new(formula), Box::new(pred));
        }
        let conclusion = p("True");
        let r = entails(&[formula], &conclusion);
        assert!(matches!(r, Err(FolError::DomainTooLarge { size: 7 })));
    }

    #[test]
    fn equality_between_distinct_constants_is_false() {
        // penguin = eagle should ground to false (no equality
        // reasoning beyond syntactic identity).
        let e = p("penguin = eagle");
        let g = ground(&e, &["penguin".into(), "eagle".into()]).expect("ground");
        match g {
            PExpr::Const(false) => {}
            other => panic!("expected false constant, got {other:?}"),
        }
    }

    #[test]
    fn equality_same_constant_is_true() {
        let e = p("penguin = penguin");
        let g = ground(&e, &["penguin".into()]).expect("ground");
        match g {
            PExpr::Const(true) => {}
            other => panic!("expected true constant, got {other:?}"),
        }
    }
}
