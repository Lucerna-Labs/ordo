# What is Ordo?

**One sentence:** Ordo is a local-first creative-operations
runtime â€” it captures briefs, plans campaigns, packages assets, audits
SEO, and talks to cloud LLMs when you want it to â€” all on your machine,
with every secret in the OS keychain and every artifact on your disk.

---

## The Who

Ordo is for three kinds of people.

### 1. Creative operators
Producers, copywriters, SEO leads, CMS editors, campaign managers â€”
anyone whose job is shepherding a creative brief from "we want to launch
a spring colorway" to "it's scheduled, SEO-packaged, and in the CMS."
The tool you want sits next to your editor, remembers your brand voice,
drafts the boring bits, lints the SEO before you paste it anywhere, and
keeps every artifact inside a folder you control.

### 2. Small studios and solo consultants
Teams who can't afford a SaaS stack for every adjacent job â€” brief
capture here, asset packaging there, SEO audits somewhere else, each
with its own login, its own data silo, its own subscription. Creative
Ordo rolls those lanes into one local-first runtime you can run on a
laptop or a private box.

### 3. Developers extending it
Ordo is capability-based: every lane (`creative.*`,
`workflow.*`, `seo.*`, `cms.*`, `cloud.*`, `ssh.*`, `api.*`, `rest.*`,
`memory.*`, `knowledge.*`, `self_heal.*`, `runtime.*`, `filesystem.*`)
is a discoverable surface on a shared Tokio bus. Adding a new lane or
swapping out a provider is a contained change, not a rewrite. See
`docs/interface-map.md` and `docs/domain-map.md` for the separation
rules.

---

## The What

### Architecture at a glance
- **19-crate Rust workspace** built on Tokio with a topic-based
  in-process bus (`ordo-bus`) as the communication spine.
- **`ordo-brain`** orchestrates requirements, runs, and tool calls over
  the bus. **`ordo-mcp-host`** is the capability host â€” every provider
  registers itself and advertises what it can do.
- **Persistent state lives in one local SQLite file** (`data/ordo.db`):
  RAG corpus, working/pinned memory, self-heal history, runtime
  settings, cloud credential metadata. Shared migrations in
  `ordo-store`.
- **`ordo-control`** serves a local HTTP control API (default
  `127.0.0.1:4141`) with a browser dashboard and JSON endpoints.
- **`ordo-studio`** is the Tauri desktop shell â€” a Liquid Glass
  2026 UX with live RAG lane visualization, bridge telemetry, the
  medbay self-heal chat, and the cloud-credentials tab.
- **`ordo-cli`** is the headless operator surface (`ordo serve`,
  `ordo cloud list|add|delete|test`, `ordo runtime status`).
- **Real outbound HTTP** is isolated in `ordo-cloud`. Secrets live in
  the OS keychain via the `keyring` crate; SQLite only holds a
  `keyring:v1` sentinel.

### Capability lanes (53 capabilities across 13 lanes)

| Lane | Shape | What it is |
|---|---|---|
| `creative.*` | domain | Brief capture, campaign planning, asset packaging, deliverables summaries, LLM copy drafting |
| `workflow.*` | domain | Review routing, revision requests, stage advancement, release scheduling, LLM note drafting |
| `seo.*` | domain | Metadata packaging, real SEO linter with severity-typed findings, LLM metadata suggestions |
| `cms.*` | domain | Field mapping, publish-readiness checks, LLM field suggestions |
| `ssh.*` | interface | Remote host description, remote command planning, workspace sync planning (pure-data) |
| `api.*` | interface | Generic API client planning, auth refresh, webhook dispatch planning (pure-data) |
| `rest.*` | interface | REST endpoint description, request preparation, response validation, resource sync (pure-data) |
| `cloud.*` | interface | **Real outbound HTTP**: OpenAI chat/embed, Anthropic messages, generic authenticated REST, credential CRUD |
| `memory.*` | runtime | Working + pinned memory with separate retention budgets |
| `knowledge.*` | runtime | RAG-informed summarization and follow-up extraction |
| `self_heal.*` | runtime | Incident planning, fix memory, replay/export |
| `filesystem.*` | runtime | Sandboxed file read/write under `user-files/` |
| `runtime.*` | runtime | Profile, storage, settings introspection and update |

### The local-first boundary
Everything that touches the network lives in **`cloud.*`**. A capability
like `creative.generate_copy` only reaches an API when (a) an operator
has stored a credential via `cloud.credentials.upsert` and (b) that
credential's secret was successfully retrieved from the vault. Remove
the credential and the capability returns a structured
`not_configured` error instead â€” the rest of the platform keeps
working.

---

## The Why

### Why local-first?
Because most of creative ops is private-by-default: client briefs,
unreleased campaigns, draft copy, customer feedback. SaaS tools push
that data into their pipelines so their models can improve. Creative
Ordo inverts it â€” the platform is on your machine, your data stays
there, and the model calls you make are the exception, not the default.

### Why not local-only?
Because "local-only" is impractical. You want GPT-5 or Claude for copy
drafting; you want to ship metadata to a hosted CMS; you want Anthropic
to help summarize a meeting. Ordo keeps the *default* local
and the *escape hatch* explicit. The moment a capability needs the
network, it's under `cloud.*`, it's gated on a configured credential,
and the operator sees exactly which outbound call was made and with
what auth style.

### Why capability-based?
Because hard-wiring the planner into specific providers doesn't scale.
Ordo's orchestrator (`ordo-brain`) doesn't know what
"seo.audit_readiness" is â€” it just knows some provider advertised that
capability. This means:

- a pure-data provider can own a capability today
- an LLM-backed provider can take over tomorrow
- a cloud-backed provider can take over after that
- the dashboard and the studio keep working through every swap

It also means **LLM-backed lanes degrade gracefully**. When a
credential is configured, `creative.generate_copy` writes real copy.
When one isn't, the platform returns a `not_configured` error but
every deterministic lane (capture_brief, plan_campaign,
package_assets, audit_readiness, field_mapping) keeps shipping.

### Why a self-heal lane?
Because runtimes crash, paths get misconfigured, credentials rotate,
hosts go unreachable. Ordo's self-heal lane remembers the
repair for each incident fingerprint. The second time the same problem
shows up, the platform replays the remembered fix instead of starting
from scratch. You can pin, export, or forget individual cases from the
dashboard.

### Why RAG-grounded LLM output?
Because generic LLM output is off-brand. Ordo's
`CreativeLlmProvider` automatically pre-queries the local RAG lane
before every chat call, injects the top-K snippets into the system
prompt, and reports `rag_context_hits` + `rag_context_sources` on the
response. The LLM stays grounded in your own corpus (brief templates,
brand voice docs, prior campaigns) without any per-call coordination.
Pass `rag: false` to opt out.

### Why a keychain-backed credential vault?
Because plaintext secrets in a SQLite file are a liability.
`ordo-cloud`'s vault stores secrets in the OS keychain (macOS Keychain,
Windows Credential Manager, Linux Secret Service) and keeps only a
`keyring:v1` sentinel in SQLite. Every read path redacts â€” no secret
appears in a dashboard response, a log line, a screenshot, or a
diagnostic export.

---

## What Ordo actually does (functional scope)

### Creative lane
- **`creative.capture_brief`** â€” captures title, goal, audience,
  deliverables; writes `user-files/briefs/<slug>.md` with the brief
  as formatted markdown; returns `artifact_path`.
- **`creative.plan_campaign`** â€” splits deliverables into discovery /
  production / launch phases; writes
  `user-files/campaigns/<slug>.md`.
- **`creative.package_assets`** â€” walks a real `input_directory`
  under `user-files/`, infers kind (image/video/copy/email/other)
  from extension, reports size and mtime, writes
  `user-files/assets/<name>.json` manifest. Path-traversal sandboxed.
- **`creative.summarize_deliverables`** â€” kind-counted summary,
  persisted as markdown.
- **`creative.generate_copy`** â€” LLM-backed copy drafting with
  automatic RAG grounding. Uses `cloud.openai.chat` for bearer
  credentials, `cloud.anthropic.messages` for anthropic-style
  credentials.

### Workflow lane
- **`workflow.route_review`** â€” determines next reviewer and next
  stage for any review stage.
- **`workflow.request_revision`** â€” persists a timestamped revision
  request as markdown.
- **`workflow.advance_stage`** â€” validates stage transitions
  (draft â†’ creative-review â†’ editorial-review â†’ seo-review â†’
  publish-ready â†’ scheduled â†’ released).
- **`workflow.schedule_release`** â€” generates a stage-by-stage
  release schedule backing out from a target date; writes
  `user-files/releases/<label>.md`.
- **`workflow.draft_notes`** â€” LLM-backed reviewer-note drafting,
  RAG-grounded.

### SEO lane
- **`seo.package_metadata`** â€” bundles title, description,
  keywords, and slug into a tag bundle.
- **`seo.audit_readiness`** â€” real linter. Severity-typed findings
  (error / warn / info) with structured codes:
  `title_empty`, `title_too_short`, `title_too_long`,
  `title_whitespace`, `title_all_caps`, `description_empty`,
  `description_too_short`, `description_too_long`,
  `slug_empty`, `slug_format`, `slug_edge_hyphen`,
  `slug_double_hyphen`, `slug_too_long`, `keywords_missing`,
  `keywords_too_many`, `keyword_not_covered`. Returns a boolean
  `ready` flag and per-finding `message` strings you can show
  directly.
- **`seo.suggest_metadata`** â€” LLM-backed metadata suggestion with
  RAG grounding and JSON-only output.

### CMS lane
- **`cms.field_mapping`** â€” maps source field names
  (`headline`/`name` â†’ `title`, `body`/`content` â†’ `body`,
  `slug`/`uri`/`url` â†’ `slug`, etc.) to a canonical CMS schema.
- **`cms.publish_readiness`** â€” checks for required fields
  (title, body, slug, publish_at) and reports which are missing.
- **`cms.suggest_fields`** â€” LLM-backed CMS value suggestion.

### Cloud lane
- **`cloud.openai.chat`** â€” real OpenAI `POST /chat/completions`.
- **`cloud.openai.embed`** â€” real OpenAI `POST /embeddings`.
- **`cloud.anthropic.messages`** â€” real Anthropic `POST /messages`.
- **`cloud.rest.request`** â€” authenticated generic HTTP against any
  configured vendor with your choice of auth style:
  `bearer`, `basic`, `api_key_header`, `api_key_query`, or
  `anthropic`.
- **`cloud.credentials.list/upsert/delete`** â€” per-service
  credential CRUD. Secrets written through the keychain vault;
  SQLite holds only a sentinel; every read redacts.

### Memory lane
- **`memory.pin_note` / `memory.unpin_note`** â€” operator-controlled
  pinned-memory lane with its own retention budget.
- **`memory.remember_note`** â€” working-memory lane (separate
  budget).
- **`memory.list_pinned` / `memory.list_working`** â€” inventory.

### Knowledge lane
- **`knowledge.summarize`** â€” RAG-informed summarization task.
- **`knowledge.extract_follow_ups`** â€” extract next steps from a
  RAG-hydrated context.

### Self-heal lane
- **`self_heal.list_cases`** â€” inventory of remembered fixes.
- **`self_heal.pin_case` / `self_heal.forget_case`** â€” curation.
- **`self_heal.replay_case`** â€” replay a remembered fix by
  fingerprint instead of re-planning from scratch.
- **`self_heal.export_case`** â€” export a case as a markdown
  repair pack.

### Filesystem lane
- **`filesystem.read_file` / `filesystem.write_file`** â€” sandboxed
  reads and writes constrained to the configured `user-files/`
  root.

### Runtime lane
- **`runtime.describe_profile`** â€” the active profile (standard,
  lean, full), control-API bind, RAG/knowledge activation modes.
- **`runtime.describe_storage`** â€” storage budget reporting.
- **`runtime.describe_settings`** â€” persisted + effective runtime
  settings.
- **`runtime.update_settings`** â€” mutate the persisted settings
  with validation.

### Operator surfaces
- **Browser dashboard** at `http://127.0.0.1:4141/` â€” capability
  inventory, RAG lane preview, memory curation, self-heal review,
  cloud credential management.
- **Tauri studio** â€” Liquid Glass 2026 desktop shell with the same
  control-API surface plus the niche-composer and bridge telemetry.
- **Headless CLI** â€” `ordo serve`, `ordo cloud list|add|delete|test`,
  `ordo runtime status`, `ordo demo`, `ordo help`.
- **Generic HTTP tool endpoint** â€” `POST /api/tools/:capability`
  lets any script drive every registered capability with a single
  `curl`.

### Observability
- **`ORDO_LOG`** / `RUST_LOG` â€” standard `tracing` filter (`info`
  by default).
- **`ORDO_LOG_JSON=1`** â€” structured JSON log lines for log
  collectors.

---

## What Ordo is *not*

- **Not a SaaS replacement for CMS itself.** It maps and packages
  content for a CMS; it doesn't host one.
- **Not a design tool.** It doesn't edit pixels or compose layouts;
  it coordinates the workflow around them.
- **Not a project management tool.** Review routing and release
  scheduling are planning aids, not substitutes for Jira/Asana.
- **Not a model vendor.** Ordo uses whatever cloud LLM the
  operator configured. The local `llama.cpp` adapter for self-heal
  is the only bundled model path, and it's optional.
- **Not a cloud platform.** It runs on your machine or a private
  host. There is no hosted Ordo instance.

---

## Current state

- **19 Rust crates**, **128 passing tests** (`cargo test --workspace`),
  `cargo fmt --all` clean, zero clippy warnings in the crates this
  project owns.
- **Runtime profile**: standard by default; lean profile for
  low-footprint installs.
- **CI**: GitHub Actions â€” fmt, test, clippy, Tauri studio
  type-check on Linux / macOS / Windows.
- **Release pipeline**: tag-triggered build of cross-platform `ordo`
  CLI binaries (`linux-x86_64`, `windows-x86_64`, `macos-x86_64`,
  `macos-aarch64`) and Tauri bundles for the studio shell.
- **End-to-end smoke test**: boots the whole runtime against a temp
  SQLite workspace + ephemeral control-API port, hits the capability
  inventory, round-trips a cloud credential, and invokes a creative
  capability through the brain.

Ordo is **prototype-ready**: it boots, it serves, it actually
produces artifacts, it grounds LLM output in your own corpus, it
remembers its own repairs, and it ships as a real binary. The work
that's left is product work â€” the specific creative workflows
individual users want to run â€” not infrastructure.
