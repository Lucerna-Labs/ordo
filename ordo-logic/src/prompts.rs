//! Prompt templates for the LLM-backed `LogicProvider`.
//!
//! Each prompt asks for strict JSON output so the parser doesn't
//! have to be clever. We include a one-shot example in the system
//! prompt to anchor the schema; reasoning models are happy to think
//! aloud as long as we tell them to put the answer in the JSON
//! block at the end.

/// Wrap a prompt body so the model emits a JSON object/array we can
/// parse. The closing instruction is repeated at the end (recency
/// bias on long prompts) and tells the model to extract code-fence
/// content if it likes — the parser strips fences either way.
pub(crate) fn json_envelope(body: &str, schema_hint: &str) -> String {
    format!(
        "{body}\n\n\
         Output requirements:\n\
         - Reply with strict JSON matching this shape:\n{schema_hint}\n\
         - The JSON may be wrapped in a ```json fence; bare JSON is also fine.\n\
         - Do not add commentary outside the JSON.\n",
    )
}

pub(crate) const IDENTIFY_CLAIMS_SCHEMA: &str =
    "  {\"claims\": [{\"statement\": string, \"weight\": 0..1, \"support\": [string, ...]}]}";

pub(crate) fn identify_claims(text: &str) -> String {
    json_envelope(
        &format!(
            "Read this passage and list every distinct explicit claim it makes. \
             Do not include claims the passage merely implies — those go in a \
             separate 'assumption audit' pass. Each claim should be one sentence \
             rephrased in plain assertive form.\n\n\
             Passage:\n```\n{text}\n```\n\n\
             For each claim, include:\n\
             - statement: the claim, one sentence\n\
             - weight: 0..1 — how directly the passage states it (1.0 = quoted)\n\
             - support: 0+ short verbatim spans from the passage that anchor the claim",
        ),
        IDENTIFY_CLAIMS_SCHEMA,
    )
}

pub(crate) const FIND_FALLACIES_SCHEMA: &str =
    "  {\"fallacies\": [{\"kind\": string, \"explanation\": string, \"quote\": string, \"severity\": \"minor\"|\"moderate\"|\"critical\"}]}";

pub(crate) fn find_fallacies(argument: &str) -> String {
    json_envelope(
        &format!(
            "Inspect this argument for logical fallacies. Treat 'fallacy' \
             strictly — actual reasoning errors, not stylistic flaws. Common \
             kinds include: ad hominem, straw man, false dichotomy, slippery \
             slope, appeal to authority, equivocation, hasty generalization, \
             post hoc, composition/division, circular reasoning. If the \
             argument is clean, return an empty list — that is a normal \
             outcome.\n\n\
             Argument:\n```\n{argument}\n```\n\n\
             For each fallacy:\n\
             - kind: short label (free-form; e.g. 'ad hominem')\n\
             - explanation: 1–2 sentences on why it qualifies\n\
             - quote: verbatim span from the argument\n\
             - severity: 'minor' | 'moderate' | 'critical'",
        ),
        FIND_FALLACIES_SCHEMA,
    )
}

pub(crate) const VALIDATE_CHAIN_SCHEMA: &str =
    "  {\"holds\": boolean, \"gaps\": [string, ...], \"notes\": [string, ...]}";

pub(crate) fn validate_chain(premises: &[String], conclusion: &str) -> String {
    let bullets: String = premises
        .iter()
        .enumerate()
        .map(|(i, p)| format!("  {}. {}", i + 1, p.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    json_envelope(
        &format!(
            "Determine whether the conclusion follows from the listed premises \
             under standard rules of inference. Answer strictly: 'holds' is \
             true only if the premises (taken at face value) are sufficient \
             to derive the conclusion. If they're insufficient, list what's \
             missing in 'gaps' (each gap is a premise that, if added, would \
             close the chain).\n\n\
             Premises:\n{bullets}\n\n\
             Conclusion: {}\n\n\
             - holds: boolean\n\
             - gaps: 0+ strings; each is a missing premise the chain needs\n\
             - notes: 0+ strings; nuance worth surfacing (modal scope, \
               equivocation, definitional ambiguity, …)",
            conclusion.trim(),
        ),
        VALIDATE_CHAIN_SCHEMA,
    )
}

pub(crate) const FORMALIZE_CHAIN_SCHEMA: &str =
    "  {\"formalizable\": boolean, \"layer\": \"fol\"|\"propositional\", \"premises\": [string, ...], \"conclusion\": string, \"vocabulary\": {var: \"meaning\", ...}, \"reason_if_not\": string}";

pub(crate) fn formalize_chain(premises: &[String], conclusion: &str) -> String {
    let bullets: String = premises
        .iter()
        .enumerate()
        .map(|(i, p)| format!("  {}. {}", i + 1, p.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    json_envelope(
        &format!(
            "Translate this argument into formal logic, if possible. The \
             prover supports two layers — pick whichever fits better:\n\n\
             **FOL (first-order logic)** — required when the argument \
             quantifies over a class. Syntax:\n\
             - Quantifiers: `forall x.` / `exists x.` (or `∀x.` / `∃x.`)\n\
             - Predicates: `Bird(x)`, `Loves(alice, bob)`, `Mortal(socrates)`. \
               Capitalize predicate names. Lowercase for variables and \
               constants.\n\
             - Connectives: AND OR NOT IMPLIES IFF (or ∧ ∨ ¬ → ↔)\n\
             - Example: `forall x. Bird(x) -> Feathered(x)` plus \
               `Bird(penguin)` entails `Feathered(penguin)`.\n\n\
             **Propositional** — when there are no quantifiers and atomic \
             propositions are sufficient. Syntax: lowercase identifiers as \
             atoms (`p`, `raining`, `cold`), same connectives.\n\
             - Example: `cold` and `cold -> wear_coat` entails `wear_coat`.\n\n\
             Mark `formalizable: false` and explain in `reason_if_not` if \
             the argument is **normative** (uses 'should'/'ought'), \
             **modal** (uses 'must'/'might' in a way classical logic can't \
             capture), relies on **vague predicates** ('Ollama is reliable' \
             — what threshold?), needs **arithmetic or numeric reasoning**, \
             or otherwise doesn't reduce to a clean formal form. Do not \
             force a fit — incorrect formalization is worse than an honest \
             punt.\n\n\
             Premises:\n{bullets}\n\n\
             Conclusion: {}\n\n\
             Output:\n\
             - formalizable: boolean\n\
             - layer: \"fol\" or \"propositional\" (only when \
               formalizable=true)\n\
             - premises: array of formalized premise strings (only when \
               formalizable=true)\n\
             - conclusion: formalized conclusion string (only when \
               formalizable=true)\n\
             - vocabulary: short dictionary mapping each predicate or \
               atom name to its plain-English meaning (only when \
               formalizable=true)\n\
             - reason_if_not: 1 sentence on why formalization failed \
               (only when formalizable=false)",
            conclusion.trim(),
        ),
        FORMALIZE_CHAIN_SCHEMA,
    )
}

pub(crate) const CLASSIFY_CLAIM_SCHEMA: &str =
    "  {\"domains\": [string, ...], \"stakes\": \"low\"|\"medium\"|\"high\", \"requires_authoritative_source\": boolean, \"rationale\": string}";

pub(crate) fn classify_claim_domain(claim: &str) -> String {
    json_envelope(
        &format!(
            "Classify this claim by domain and stakes. The output \
             feeds Ordo's Grounding Floor — when a claim is in a \
             high-stakes domain (legal, medical, financial, safety, \
             dosage, scientific consensus), the assistant must not \
             commit to it based on bare web content. So your job is \
             to flag exactly when this routing should fire.\n\n\
             Claim:\n```\n{claim}\n```\n\n\
             Decision rules:\n\
             - domains: zero or more of: legal, medical, financial, \
               safety, dosage, scientific_consensus, regulatory, \
               privacy_law, security, election, or `general` if no \
               specialized domain fits. Use multiple tags when the \
               claim spans (e.g. a drug-pricing claim is both \
               medical and financial).\n\
             - stakes: low if acting on a wrong belief is cheaply \
               reversible; medium if recoverable but costs time / \
               money / reputation; high if irreversible harm is \
               possible (legal liability, medical injury, financial \
               ruin, safety incident).\n\
             - requires_authoritative_source: true when the claim \
               is specific and high-stakes enough that an operator \
               should NOT act on it from web content alone. False \
               for opinion, general knowledge, or trivially \
               verifiable claims.\n\
             - rationale: one sentence — why you classified it this \
               way.\n\n\
             Calibration:\n\
             - \"the Supreme Court ruled X is now legal\" → legal, \
               high stakes, requires authoritative source.\n\
             - \"aspirin can interact with warfarin\" → medical, \
               medium-high stakes, requires authoritative source \
               for specifics.\n\
             - \"Paris is the capital of France\" → general, low, \
               no authoritative source required.\n\
             - \"Apple's stock closed at $X yesterday\" → financial, \
               medium, requires authoritative source for the price.\n\
             - \"this article argues that X\" → general (it's a \
               quotation, not a claim by you), low.",
        ),
        CLASSIFY_CLAIM_SCHEMA,
    )
}

pub(crate) fn steel_man(argument: &str) -> String {
    // No JSON envelope — the entire reply IS the steel-manned
    // argument, plain prose. Reasoning models often emit literal
    // newlines inside JSON strings (invalid JSON), and there's no
    // structure here worth defending against that. Just ask for
    // prose and return what the model says.
    format!(
        "Produce the strongest, most charitable version of this argument. \
         Repair weak phrasings, fill in obvious supporting steps, and \
         present it as the original author would on their best day — \
         without changing the conclusion or smuggling in different \
         claims. If the argument is already strong, lightly polish it \
         rather than rewrite.\n\n\
         Argument:\n```\n{argument}\n```\n\n\
         Output requirements:\n\
         - Plain prose, 1–4 paragraphs depending on complexity.\n\
         - No JSON, no fences, no preamble — just the steel-manned \
           argument as you'd write it for the author.",
    )
}
