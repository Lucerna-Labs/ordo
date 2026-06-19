# Domain Map

Ordo should keep runtime domains clear so features do not collapse into one
generic assistant bucket.

## Active Domains

- `assistant.*`
  - sessions, turns, memory recall, knowledge lookup, model-facing tool use
- `agent_team.*`
  - team setup, role definitions, role skills, team activity, bounded
    multi-agent work
- `tech_specialist.*`
  - diagnostics, setup, installs, repairs, provider help, MCP/plugin upkeep,
    automation help, avatar setup, local computer tasks with approval
- `automation.*`
  - routines, hooks, cron-style schedules, heartbeats, webhooks, local events,
    dreaming reviews, bounded coding automation
- `memory.*`
  - working memory, pinned memory, operator facts, memory curation
- `rag.*` / `knowledge.*`
  - local retrieval, self-knowledge, document context, hash fallback,
    optional embedding-backed search
- `provider.*` / `cloud.*`
  - local model providers, cloud model providers, credentials, model lifecycle
- `remote_comm.*`
  - email accounts, remote communication identities, read/write/none access
    controls, future Signal/Telegram support
- `artifact.*` / `files.*`
  - generated files, PDFs, docs, spreadsheets, email views, side-view artifacts
- `mcp.*` / `plugins.*` / `skills.*`
  - installable capability surfaces and their trust/security state
- `review.*`
  - operator approval queues and human gates
- `runtime.*`
  - profile, storage, health, settings, and component state

## Retired Domains

The old `creative.*`, `seo.*`, and `cms.*` product lanes are retired. Do not add
new capabilities or docs under those domains unless the product direction is
explicitly changed again.

## Separation Rules

- Capability names should make ownership clear.
- Runtime primitives such as memory, retrieval, filesystem, transport,
  security, and review remain shared infrastructure.
- Tech Specialist can manage setup surfaces, but it should not bypass secrets,
  approval, or permission UI.
- Provider/model work belongs under provider/cloud lifecycle surfaces, not
  inside normal assistant chat.
- Remote communication belongs under Remote Communication, not under general
  automation.
- Agent Team setup belongs to Agent Teams and Tech Specialist, not to hidden
  Studio-only state.

## Why This Matters

- Users can understand where a feature belongs.
- Skills can stay scoped instead of overwhelming small models.
- Logs and approvals can name the correct subsystem.
- Retired product directions do not quietly reappear.
