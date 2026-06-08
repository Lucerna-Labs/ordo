# Defense in Depth: Ordo's Prompt-Injection Architecture

**Thesis and blueprint covering the Strainer, the Cup, and the
Grounding Floor**
*Lucerna Labs · internal architecture document*
*Companion to: `strainer-thesis-and-blueprint.md`, `grounding-floor.md`*

---

## Thesis

Prompt injection is not one problem. It's four problems wearing
the same costume, and trying to solve them with one mechanism
guarantees you fail at three of them. The honest framing is
**defense in depth across orthogonal attack surfaces**, where
each layer is calibrated to catch what the others structurally
cannot.

The four problems:

1. **Structural attacks** — hidden HTML, encoding tricks,
   special-token sequences, polyglot payloads. The page is
   designed to look one way to a reviewer and decode another way
   to the model.
2. **Slow attacks** — content that doesn't try to override
   behavior immediately but plants context that biases the model
   many turns later. The injection sits dormant inside the
   conversation history until a sensitive action surfaces.
3. **Semantic attacks** — false facts presented in plain prose.
   Nothing structurally distinguishable from real content; just
   not true.
4. **Tokenizer attacks** — model-specific quirks where text that
   reads benignly to a reviewer tokenizes to a different
   instruction for the specific model. Per-model and hard to
   enumerate.

Three Ordo layers, each catching different surfaces:

| Layer | Catches | Pattern |
|---|---|---|
| **Strainer** (`ordo-strainer`) | structural, tokenizer, polyglot | deterministic transforms on incoming content |
| **Cup** (taint propagation) | slow injections | provenance-aware action gating |
| **Grounding Floor** (`ordo-logic` + system rule) | semantic injections | architectural deference for high-stakes claims |

None is bulletproof. The point is not perfection; the point is
that each layer makes the others sufficient.

---

## What lives where in the codebase

```
ordo-strainer/                              ← Layer 1: the Strainer
├── extract.rs        Stage 1 — main content
├── strip.rs          Stage 2 — invisible content
├── markdown.rs       Stage 3 — normalize structure
├── normalize.rs      Stage 3.5 — encoding + polyglot defense (Phase A)
├── wrap.rs           Stage 4 — boundary tags
└── capability.rs     web.strain bus surface

ordo-protocol/src/mcp.rs::Taint             ← Layer 2: the Cup (data)
  + UntrustedWeb { source_url, fetched_at }

ordo-assistant/src/service.rs               ← Layer 2: the Cup (logic)
  + session_taint tracker
  + detect_untrusted_web_taints()
  + is_sensitive_capability()
  + auto-mark-taint at turn() entry
  + sensitive-capability gate at dispatch boundary

ordo-control/src/lib.rs                     ← Layer 2: the Cup (API)
  + GET /api/assistant/sessions/:id/taint
  + POST /api/assistant/sessions/:id/taint/clear

ordo-studio                                 ← Layer 2: the Cup (UI)
  + chat-header taint indicator
  + clear-taint button
  + 5s poll of session taint state

ordo-logic/                                 ← Layer 3: the Grounding Floor
  + classify_claim_domain capability
  + LlmLogicProvider impl
  + capability descriptor on /api/capabilities

ordo-assistant/src/prompt.rs                ← Layer 3: paired system rule
  + UntrustedStrictness preset (off/low/medium/high)
  + High-stakes claim rule (Grounding Floor)

docs/                                       ← The framing
├── strainer-thesis-and-blueprint.md
├── grounding-floor.md
└── prompt-injection-defense.md             (this document)
```

---

## Layer 1: The Strainer

### Premise

There is no sanitizer that makes arbitrary web content safe for
an LLM to read. Detection-based approaches fail because
identifying "instruction-shaped text trying to hijack the model"
requires understanding intent, and understanding intent requires
a model — which is itself injectable. **Detection is unsolvable
at the layer below the LLM.**

So detection isn't the goal. **Transformation is.** The strainer
is a coffee strainer, not a security filter — calibrated to
catch what ruins the experience while letting through what
doesn't matter. Lossy on purpose.

### Pipeline

```
raw HTML (from web fetch)
       ↓
Stage 1 — extract main content    (article > role=main > main > body − chrome)
       ↓
Stage 2 — strip invisible          (scripts, hidden, ZWS, RTL overrides, on*, data-*)
       ↓
Stage 3 — HTML → markdown          (custom converter, link-scheme allowlist)
       ↓
Stage 3.5 — normalize text         (NFKC, homoglyph fold, special-token strip,
                                    bidi balance, base64 blob shorten)
       ↓
Stage 4 — boundary wrap            (<untrusted_web_content source fetched_at sha256>)
```

Six normalization passes added in Phase A:

1. **Unicode NFKC normalization** — collapses compatibility forms
   (full-width Latin → half-width, ligatures → component letters).
2. **Homoglyph fold** — Cyrillic / Greek / Cherokee codepoints
   that visually duplicate ASCII Latin, mapped to Latin **only
   when the surrounding token is Latin-context-dominant**. Pure
   non-Latin words pass through untouched.
3. **Special-token strip** — known tokenizer-special strings
   (`<|im_start|>`, `<|endoftext|>`, `[INST]`, …) removed from
   text content.
4. **Bidi balance** — directional override codepoints stripped
   even when unmatched. Defense in depth on top of Stage 2.
5. **Code-fence handling** — Stage 3 only emits triple-backticks
   around legitimate `<pre><code>`; non-code text never produces
   stray fences.
6. **Base64 blob shorten** — runs of ≥120 base64-alphabet chars
   in prose replaced with a placeholder. Real prose never has
   long uninterrupted base64 runs; legitimate base64 lives in
   code blocks.

### What it catches

- Hidden elements (`display:none`, `visibility:hidden`, `opacity:0`,
  `font-size:0`, off-screen positioning, `aria-hidden="true"`)
- Inline scripts, styles, iframes, embeds, comments
- Event-handler attributes (`onclick`, `onload`, …) and `data-*`
- Zero-width / format-control / RTL-override Unicode characters
- Encoding tricks: full-width Latin, homoglyphs, ligatures
- Known model-special-token strings
- `javascript:` / `data:` / `file:` URL schemes (display text kept)
- Base64-shape payloads dropped into prose
- Hostile URLs trying to break out of the boundary tag's
  attribute (escaped on wrap)

### What it doesn't catch

- New per-model tokenizer quirks not on the special-token list
- A polyglot we haven't seen before (each new shape needs a new
  pass; the doc accepts this — trust the cup for residuals)
- Anything that's structurally indistinguishable from real
  content — see Layer 3

### Cost

| | |
|---|---|
| Source lines | ~1,500 |
| Compiled binary growth | ~1 MB (scraper + html5ever + unicode-normalization) |
| Latency per call | <10 ms for typical articles |
| Tests | 55 unit + 1 integration, all pass |

---

## Layer 2: The Cup (taint propagation)

### Premise

Once strained content has entered a conversation, the boundary
tag is the only structural reminder. After three or four turns
of normal conversation, the strained content is buried in
history — but the model still has it in context. A sufficiently
patient injection can wait for an unrelated turn where a
sensitive action becomes plausible, then nudge the model toward
it.

The right defense is at the **provenance layer**, not the
content layer. Mark the conversation as "this has been touched
by untrusted content" once, propagate that mark, and gate
sensitive actions on it for the conversation's lifetime.

### What it does

- **Detection.** At the start of every turn, scan
  `request.user_message` for `<untrusted_web_content>` boundary
  tags. Each tag becomes a `Taint::UntrustedWeb { source_url,
  fetched_at }` event attached to the session.
- **Persistence.** The session's taint set is in-memory for the
  runtime's lifetime. Restart wipes it (sessions don't survive
  restarts either, so persistence would be theater).
- **Gating.** Before each tool dispatch, check the session's
  taint state. If untrusted, reject sensitive capabilities with
  a clear error pointing the operator to the clear-taint action.
- **Surface.** Studio polls the taint state every 5s, renders a
  lamp-hot indicator above the chat with the source URL in a
  tooltip, and a `CLEAR TAINT` button.

### Sensitive-capability allowlist

Reads / analysis stay open. The model can still summarize, recall,
look up, validate. What it can't do under taint is **actuate**:

| Category | Examples |
|---|---|
| Outbound dispatch | `api.dispatch_webhook`, `api.sync_resource`, `ssh.prepare_command` |
| State-escaping writes | `files.upload`, `files.delete`, `filesystem.write_file`, `apps.publish` |
| Credential surface | `cloud.credentials.upsert`, `cloud.credentials.delete` |
| Memory persistence | `memory.pin_note`, `assistant.remember_fact` |
| MCP install/admin | `mcp.servers.install`, `mcp.servers.uninstall` |
| Review-queue actuation | `review.approve`, `review.deny`, `review.edit` |
| Webhook config | `webhooks.register`, `webhooks.update`, `webhooks.delete` |

The slow-injection vehicle's most attractive target —
`assistant.remember_fact` — sits squarely in the blocked list.
A planted fact can't get persisted past the session's lifetime
without the operator clearing taint first.

### What it catches

- A planted instruction in turn 1 trying to fire a webhook in
  turn 5
- A planted fact attempting to escape into long-term memory
- A subtle nudge to install an MCP server, modify credentials, or
  approve a review — all blocked while the session has tainted
  ancestry

### What it doesn't catch

- A claim that doesn't trigger a tool call but does shape what
  the model SAYS to the operator. The operator might still act
  on a confidently asserted falsehood that the cup couldn't see —
  see Layer 3.
- An operator who clears taint and explicitly proceeds. By
  design — the cup gates the model, not the operator.
- A turn where strained content was injected via some channel
  that didn't produce the boundary tag. Today the only path is
  `web.strain`, so this is a single-source detection. As more
  sources land (e.g. RSS readers, search-result ingest), each
  needs to emit the boundary tag for the cup to see it.

### Cost

| | |
|---|---|
| Source lines | ~250 (Rust) + ~80 (TS) |
| Latency per turn | <1 ms (mutex peek) |
| Memory | one HashSet per active session |
| Tests | 3 inline + e2e flow verified live |

---

## Layer 3: The Grounding Floor

### Premise

Semantic injection — false facts in plain prose — is structurally
indistinguishable from real content. The strainer can't see it.
The cup gates actuation but doesn't stop confident assertion. The
model can't determine truth from its own training cutoff.

The right defense is **architectural deference**. When the
assistant is about to commit to a high-stakes claim, route
through citation discipline. We don't ask the model to
fact-check; we ask it to attribute. A quote is fine; an
unmoored assertion isn't.

### What it does (Layer 1 — shipped)

- **`logic.classify_claim_domain` capability** — takes a claim,
  returns `{ domains, stakes, requires_authoritative_source,
  rationale }`. LLM-driven (same pattern as the rest of
  `ordo-logic`).
- **Paired system prompt rule** — added to `BOOTSTRAP_BASE` so
  it's present at every strictness level. Three behaviors:
  1. Hedge wording when claim came from `<untrusted_*>` content:
     "the article asserts...", never bare assertion.
  2. Decline when asked to ACT on a high-stakes claim grounded
     only in untrusted web content; ask for an authoritative
     source.
  3. Call `logic.classify_claim_domain` when in doubt.

### What it doesn't do (Layers 2 + 3 — deferred)

- **Layer 2: source-attributed memory.** Facts persisted to
  `assistant.remember_fact` would carry a `FactProvenance` field
  showing whether they came from operator input, a tool call, or
  a strained source. Recall ranking would downweight strained
  sources so they surface but rarely outrank operator memory.
  Multi-day work, schema migration; framed in
  `docs/grounding-floor.md`.
- **Layer 3: authoritative-source registry.** Operator-authored
  TOML mapping high-stakes domain → trusted source URL. When the
  classifier flags a claim as high-stakes, the assistant pulls
  from the registry instead of inferring from arbitrary web
  content. Week+; same doc for the framing.

### What it catches (today, Layer 1)

- Unintentional confidence on high-stakes domains. The model
  hedges instead of speaking with unwarranted authority.
- Naive paraphrase of false claims as fact. The classifier
  catches the domain; the system rule fires on the
  `requires_authoritative_source` flag.

### What it doesn't catch

- An operator who explicitly says "trust the article and act."
  At that point the operator has chosen to endorse it; the
  model can hedge but can't refuse without being uncooperative
  on legitimate use.
- Novel high-stakes domains the classifier hasn't been calibrated
  for. The classifier is LLM-driven, not table-driven, so
  coverage is determined by how well the prompt's calibration
  examples generalize.
- A determined operator chain: read false content → manually
  pin as a fact → ask the model to act on the pinned fact.
  Layer 2 (source-attributed memory) closes this; Layer 1 alone
  doesn't.

### Cost

| | |
|---|---|
| Source lines | ~350 (Rust) + system prompt addendum |
| Latency per call | one LLM round-trip per classification (operator-toggleable) |
| Tests | passes existing 37 ordo-logic tests + live verified |

---

## The model is not the layer

A note to settle the framing: **the model is external to Ordo**.
Ordo connects to it via the cloud-credential plumbing. For a
local Ollama setup, the model (qwen3.6:35b weights, the inference
engine) is loaded by Ollama in a separate process, and Ordo
talks to it over `http://localhost:11434/v1`.

This means:

- All three Ordo layers (strainer, cup, grounding floor) are
  **model-independent**. They work the same way regardless of
  which model the operator wires in.
- Whatever safety training the model brings (RLHF refusals,
  built-in jailbreak resistance, prompt-injection awareness)
  is a **separate, additive** layer the operator inherits from
  their model choice. Ordo doesn't ship it, can't enforce it,
  can't remove it.
- A weaker model means Ordo's layers have to do more work alone.
  A stronger model means defense in depth gets thicker. Either
  way, Ordo's contribution is the same — the operator's model
  choice is the variable.

This is why the strictness preset starts at `medium` by default
and `off` is explicitly labeled DEBUG. With strictness off,
Ordo is still doing strainer transformations and cup gating —
those are model-independent — but the system-prompt rule
(which the model interprets) is absent, and you're trusting
whatever the operator's model brings on its own.

---

## How the four threats land against the three layers

| Threat | Strainer | Cup | Grounding Floor |
|---|---|---|---|
| Hidden HTML / encoding | **catches** | partial | — |
| Polyglot | **catches** | partial | — |
| Special-token injection | **catches** | partial | — |
| Slow influence (planted fact in turn 1, exploit in turn 5) | partial | **catches** | partial |
| Tool-actuated slow injection (planted instruction → webhook fire) | partial | **catches** | — |
| Semantic injection (false facts in plain prose) | — | partial | **catches Layer 1** |
| Confident-but-wrong fact assertion in reply | — | partial | **catches Layer 1** |
| Persisted falsehood via remember_fact | — | **catches** | needs Layer 2 |
| High-stakes wrong action without authoritative source | — | partial | needs Layer 3 |

"Catches" means the layer is the primary defense for the threat.
"Partial" means the layer narrows the surface but isn't the
primary mitigation. "—" means the layer is structurally unable to
help with this category.

---

## Build order recommendation

For a new operator wiring Ordo into a fresh deployment, the order
to enable in:

1. **Ship Phase A + Phase B + Phase C-Layer 1 from day one.** All
   three are already in `main`. Together they cover the bulk of
   prompt-injection surface with sub-millisecond latency overhead.
2. **Set strictness to `medium` or `high`.** Off is debug; low is
   for low-friction non-adversarial use.
3. **Build Layer 2 (source-attributed memory) when you start
   pinning facts from web content.** Until then, the cup's
   block on `assistant.remember_fact` is enough.
4. **Build Layer 3 (authoritative-source registry) when you have
   a specific high-stakes use case.** Legal advisor, medical
   triage, financial advice — those domains need it before you
   ship to operators. General-purpose creative-ops doesn't.

---

## Anti-patterns refused

The strainer doc names these; they apply to the cup and grounding
floor too:

- **No detection LLM in any layer.** The classifier in Layer 3 is
  LLM-driven but it operates in classification mode, not detection
  mode. There's a difference: classification asks "what kind of
  claim is this?", detection asks "is this an attack?". The
  former is well-posed; the latter is not.
- **No injection-phrase blocklist.** Attackers iterate faster than
  blocklists.
- **No bypass for trusted sources.** There is no such thing on the
  open web. Every fetch goes through the strainer, period.
- **No optional boundary tags.** The strainer's output IS the
  cleaned content plus the boundary tags. Stage 4 is non-optional.
- **No silent mode.** Off-strictness exists as a debug tool but
  is loudly labeled and visually marked (red border, danger badge)
  in the UI.

---

## Honest limitations

- The model is the operator's choice. Ordo cannot make a weaker
  model safer than its training. It can only narrow the surface.
- The cup is in-memory. Cross-runtime persistence would let a
  taint propagate through a runtime restart, but it would also
  let a stuck taint persist past where it's useful. We chose
  ephemerality.
- The grounding floor's Layer 1 is a system prompt rule. A
  jailbroken model that ignores its system prompt ignores this
  too. Layers 2 and 3 are deterministic backstops for that
  failure mode.
- The doc framing is "defense in depth, not perfection." If a
  given threat slips through all three layers, that's the cup
  failing to catch what the strainer and grounding floor missed
  — and we should add a transform / a gate / a system rule to
  close it. The architecture supports incremental tightening.

---

## Closing note

Prompt injection is not a problem that gets solved. It's a
problem that gets bounded. The strainer bounds the structural
surface. The cup bounds the temporal surface. The grounding floor
bounds the semantic surface. Together they bound enough that the
operator can run Ordo against the open web and trust the
defaults.

Built right, the operator should never think about any of this —
pages just come back cleaner, conversations stay coherent across
turns, claims get cited or hedged appropriately. Built wrong, it
becomes a babysitter that breaks legitimate work while still
missing novel attacks.

The doc is small. The build is small. The trust comes from the
transforms not detecting; from the cup gating not pre-empting;
from the grounding floor deferring not censoring. Each layer
small, each layer dumb, each layer in its right lane.
