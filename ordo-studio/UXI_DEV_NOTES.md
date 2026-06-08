# Ordo UXI Dev Notes

These notes document the current working desktop UXI, how it was recovered,
and how to rebuild the same shape again if it breaks.

## Canonical Shape

- The desktop shell is a Tauri v2 app using WebView2 on Windows.
- The visible interface is React/HTML/CSS, mounted from `src/main.tsx`.
- `src/OrdoShell.tsx` is the canonical UXI surface.
- `src/api.ts` is the boundary between the UXI and the runtime/control data.
- `src-tauri/src/main.rs` must only create the Tauri WebView window.
- Do not add a separate Rust UI window. If Rust is needed, keep it headless
  and expose data through Tauri commands or the control API.

Expected operator result:

- One window opens.
- Window title is `Ordo`.
- The Ordo UXI paints immediately.
- No extra browser window opens.
- No separate Rust-native UI opens.
- Modes, plugins, and MCP inventory are surfaced in the UXI.

## Cron Jobs, Heartbeats, Dreaming, and Subagents

- The left rail exposes `Cron Jobs` directly. It is the operator-facing surface
  for scheduled/autonomous work.
- `Heartbeat` means a specific timed return/check-in for conversational or
  project continuity. It should resume with a concrete instruction and visible
  status, not vague background activity.
- `Dreaming` means advisory reflection: review completed work, failures,
  corrections, logs, and improvement candidates. Dreaming must propose changes;
  it must not silently edit memory, install tools, rewrite settings, or execute
  actions.
- Dreaming is also installed as a default-on mode at
  `user-files/modes/dreaming.json` and exposed as a left-rail tab. Its default
  tree RAG domains are `self_learning_tree`, `dreaming_reflections`,
  `corrections`, `failure_analysis`, `event_logs`, `promotion_candidates`,
  `approved_lessons`, and `architecture_drift`.
- Cron job records include kind, schedule, target mode, instruction, approval
  gate, subagent allowance, and max subagent count. They can be created, edited,
  paused/enabled, and deleted from the UXI.
- Current implementation is UXI-owned/persisted and debug-logged. Runtime
  execution must be wired through the native scheduler/agent supervisor before
  any hidden or detached work is allowed.
- Subagent collaboration remains consultation-based. The active mode must not
  read another mode's RAG or memory directly; it can only consult a target mode
  agent and use the bounded answer.

## What Broke

The damaged build had three related failures:

1. Multiple windows opened:
   - A Tauri/WebView window.
   - A Rust UI/runtime window.
   - Sometimes an external browser/dev URL window.

2. The WebView went blank:
   - The built `dist/index.html` had asset paths that did not resolve inside
     the Tauri asset origin.

3. Modes/plugins/MCPs disappeared:
   - The UXI only asked `http://127.0.0.1:4141/api/...`.
   - After the sidecar/runtime window was removed, no API listener was running
     there.
   - The local data still existed under root `user-files`, but the UXI had no
     fallback path to read it.

## Recovery That Worked

### 1. Remove Extra UI Launch Paths

The Tauri app should not spawn or manage a visible runtime UI.

Confirmed cleanup:

- Removed sidecar module wiring from `src-tauri/src/main.rs`.
- Removed shell plugin wiring that existed only for launching sidecars.
- Removed `externalBin` from `src-tauri/tauri.conf.json`.
- Removed shell permissions from `src-tauri/capabilities/default.json`.
- Removed stale root/debug launch artifacts that could start the wrong app.

Keep this invariant:

```text
Tauri creates the only visible desktop window.
React/HTML/CSS owns the UXI.
Rust can provide commands and data, but not a competing window.
```

### 2. Make Tauri Assets Resolve Correctly

`vite.config.ts` must include:

```ts
export default defineConfig({
  base: "./",
  // ...
});
```

Without `base: "./"`, the production WebView can load `index.html` but fail to
load JS/CSS assets, producing a blank dark window.

### 3. Keep The Known-Good React Plugin Stack

The working build uses (current shipped stack — see `package.json`):

```json
"@vitejs/plugin-react": "^6.0.2",
"vite": "^8.0.9"
```

Do not replace the React plugin stack casually while repairing the UXI. (This
note previously pinned plugin-react ^4.3.3 / older Vite; it has since been
upgraded to the rolldown-based Vite 8 stack above, which builds and paints.)
Treat a working UXI as more important than chasing cosmetic build warnings
during emergency recovery.

### 4. Route Runtime API Calls Correctly

Inside Tauri, `fetch("/api/...")` does not automatically reach the runtime.
`src/api.ts` now detects the Tauri asset origin and maps runtime paths to:

```text
http://127.0.0.1:4141/api/...
```

The helper shape is:

```ts
const CONTROL_API_ORIGIN = "http://127.0.0.1:4141";

function apiUrl(path: string): string {
  if (isTauriAssetOrigin() && (path.startsWith("/api") || path === "/health")) {
    return `${CONTROL_API_ORIGIN}${path}`;
  }
  return path;
}
```

This allows the UXI to use the same API functions in browser dev and Tauri.

### 5. Add Local Tauri Fallbacks For Inventory

Modes, plugins, MCP servers, and capability inventory need to surface even when
the control API is offline.

Rust commands live in `src-tauri/src/backend.rs`:

- `list_local_modes`
- `list_local_plugins`
- `list_local_mcp_servers`
- `list_local_capabilities`

They are registered in `src-tauri/src/main.rs`.

Frontend fallbacks live in `src/api.ts`:

- `listAssistantModes`
- `listPlugins`
- `listMcpServers`
- `fetchCapabilities`

Pattern:

```ts
try {
  return await api.get(...);
} catch (err) {
  if (!isTauriAssetOrigin()) throw err;
  return await invokeLocal("list_local_...");
}
```

This is the preferred recovery pattern. Do not reintroduce a visible runtime
window just to get the inventory back.

### 6. Keep Plugins And MCP Separate

The Plugins tab and MCP tab are separate operator surfaces and must not share
inventory.

Plugins tab:

- Uses `listPlugins`.
- Reads plugin manifests from `user-files/plugins/<plugin>/plugin.json`.
- Shows research plugins such as `research-arxiv`.

MCP tab:

- Uses `listMcpServers` for server rows.
- Uses `fetchMcpCapabilities` for the MCP tool list.
- Reads MCP package manifests from `mcp-servers/<server>/manifest.json`.
- Must not call the broad `fetchCapabilities` helper for its tool list.
- Must not show `research-*` plugin entries.
- Must not show removed content/channel workflow packages or tools.

If research plugins appear under the MCP tab again, check that
`McpSurface.refresh()` is still calling `fetchMcpCapabilities()` rather than
`fetchCapabilities()`, and check that the Tauri fallback command is
`list_local_mcp_capabilities`.

Removed workflow class:

- No CMS MCP package.
- No WordPress MCP package.
- No Bluesky MCP package.
- No Mastodon MCP package.
- No mode files for Lucerna Media, Nuntius, Warped Reality, or generative
  image-training workspaces.
- No Connections catalog entries for social/channel posting surfaces.

## Local Data Sources

The fallback readers intentionally look in root project data first.

Modes:

```text
user-files/modes
ordo-studio/user-files/modes
```

Plugins:

```text
user-files/plugins/<plugin>/plugin.json
ordo-studio/user-files/plugins/<plugin>/plugin.json
```

MCP package manifests:

```text
mcp-servers/<server>/manifest.json
```

The root `user-files` folder is important. Do not assume
`ordo-studio/user-files` is complete.

## UXI Identity

Keep visible naming simple:

- Product name: `Ordo`
- Window title: `Ordo`
- HTML title: `Ordo`

Do not render internal labels such as "official UXI" in the operator-facing
interface.

## 2026-06-03 UXI Management Surfaces

- Added the left-rail `Modes` surface for manual mode selection, per-mode
  enable/disable state, and a local RAG storage budget control.
- Added the left-rail `Settings` surface as a one-page index for every Ordo
  surface, while the left rail itself only exposes the daily-use tabs.
- Added a New Chat control to the Assistant surface. It creates a new assistant
  session, clears pending attachments, clears the current taint banner, and
  keeps the selected mode.
- Reworked Skills, Connectors, and Plugins into a shared Directory-style UXI:
  sidebar navigation between the three, search, sort controls, and card grids.
- Skills now separates built-in catalog skills from user-added skills. User
  skills expose edit, pause/resume, and delete controls.
- Installed `ordo_self_improvement` under `user-files/skills/` as an
  Ordo-exclusive self-improvement governance skill. It adapts the useful parts
  of correction learning, reflection, scoped memory, hook-triggered review, and
  heartbeat maintenance into Ordo's own global/project/mode model. It must not
  silently promote rules, store secrets, or learn from silence.
- Installed `ordo_automation_ops` under `user-files/skills/` as an
  Ordo-exclusive automation operations skill. It defines native guidance for
  cron jobs, heartbeats, routines, webhook/API triggers, local event triggers,
  failure policies, permissions, UXI surfacing, and debug/event logger
  integration. It is not an n8n clone and should not create hidden background
  behavior or removed workflow templates.
- Installed `ordo_document_ops` under `user-files/skills/` as an Ordo-native
  document operations skill. It covers Markdown, plain text, PDF, and DOCX
  reading/editing/conversion with recoverable edits, format-aware tooling,
  render or structural verification, event logger entries, and explicit
  warnings for lossy conversion or unverifiable redaction.
- Installed `ordo_deep_research` under `user-files/skills/` as an Ordo-native
  deep research skill. It defines the research method for report-style,
  Perplexity/Gemini Research-like work: question decomposition, source quality
  ranking, evidence matrices, contradiction review, citation discipline,
  recency checks, confidence scoring, and debug/event logger entries. A plugin
  should only be added later for tool infrastructure such as search APIs,
  crawling, citation storage, local indexing, or source graphs.
- Installed `ordo_research_index` under `user-files/plugins/` as the native
  deep research tool provider. Its manifest advertises `research.search`,
  `research.crawl`, `research.index`, `research.retrieve`, and
  `research.sources` lanes. The local runner supports URL crawling, direct
  source indexing, SQLite FTS retrieval, index status, and a configurable JSON
  search endpoint through `ORDO_RESEARCH_SEARCH_ENDPOINT`.
- Installed `ordo_research_summarizer` under `user-files/skills/` as the
  research distillation layer. It extracts what is important, relevant,
  supported, disputed, duplicative, or actionable from research sources before
  final synthesis. `ordo_deep_research` should use it after source retrieval
  and before final report writing.
- Installed `ordo_rust_architecture` under `user-files/skills/` as the
  model-facing Rust architecture rulebook for Ordo coding work. It teaches
  owning-crate selection, workspace responsibility, root-cause fixes, Tauri
  backend/UXI boundaries, dependency rules, event logging, tests, and Cargo
  verification gates. It should be assigned to coding/runtime architecture
  modes later, not every mode by default.
- Installed `ordo_primitive_orchestrator` under `user-files/skills/` as the
  model-facing guide for building reusable primitive kits and wiring them into
  Ordo's orchestrator. It defines the primitive/adapter/provider/orchestrator/
  UXI layers, capability lane naming, security/review requirements, storage
  rules, event logging, and mode assignment guidance for features meant to be
  reused by other engines, functions, modes, plugins, MCP servers, jobs,
  providers, or devices.
- Installed `ordo_math_primitive_reconstruction` under `user-files/skills/` as
  the operator's decomposition technique. It teaches models to observe a
  system, reduce it to variables, equations, constraints, invariants, and
  primitive operations, then rebuild the function from the required original
  primitives with behavioral verification. Use it before
  `ordo_primitive_orchestrator` when the primitive has not been discovered yet.
- Installed `ordo_codec_adapter_research` under `user-files/skills/` as the
  research/build guide for codec adapters. It teaches models to find the
  canonical contract, preserve framed envelope or payload shapes, keep codec
  primitives separate from thin adapters, avoid parallel formats, document
  lossy behavior, and verify with round-trip, malformed-input, streaming,
  compatibility, and fallback tests.
- Installed `ordo_rf_signal_reconstruction` under `user-files/skills/` as the
  RF-specific research and reconstruction skill. It teaches lawful/authorized
  RF signal intake, I/Q and spectrum inspection, primitive DSP decomposition,
  modulation hypotheses, reconstruction models, tolerance-based verification,
  event logging, and the boundary against unauthorized interception, spoofing,
  jamming, tracking, or bypassing protected systems.
- Installed `lucerna-media` under `user-files/skills/` from the operator's
  supplied definition as the canonical Lucerna ecosystem and strategy skill.
  It defines Lucerna Media, Nodus, Ordo, Imperium, Nuntius, Warped Reality, The
  Station, Lucerna Labs, Creative Claw, Project Mother, Latin naming, the
  two-tier revenue/community model, Nodus publisher surfaces, and the strategic
  relationship between the media company and the software stack.
- Installed `warped-reality-persona` under `user-files/skills/` from the
  operator's supplied definition as the Warped Reality political/social
  commentary persona. It defines the rhetorical role, hypocrisy probe,
  motivational dissection, logical inversion, reverse psychology, mandatory
  verification before factual claims, first-person voice, and no-em-dash
  punctuation constraint.
- Installed `queer-horror-craft` under `user-files/skills/` for the operator's
  personal creative-writing use. It is explicitly marked `visibility: personal`
  and `release: exclude`; it must not be bundled, marketed, surfaced, or
  included in a public Ordo release unless the operator changes that rule.
- Installed `writing_humanizer` under `user-files/skills/` as a general writing
  polish skill. It humanizes drafts by preserving meaning, facts, stance, and
  uncertainty while reducing robotic phrasing, generic assistant texture, and
  over-polished corporate tone. It explicitly forbids inventing personal
  details, credentials, citations, emotion, or anecdotes.
- Installed `ordo_synthetic_memory_author` under `user-files/skills/` as the
  governance skill for creating constructed memory-like instruction records.
  It labels them as synthetic instruction memories, records provenance and
  scope, keeps them editable/removable, and prevents them from being presented
  as factual lived history.
- Installed `ordo_rust_project_instruction_memory` under `user-files/skills/`
  for Rust-project synthetic memory anchors. It creates coding-mode/project
  instruction memories for root-cause repair, owning-crate selection, workspace
  responsibility, capability boundaries, Studio/Tauri boundaries, and
  verification gates.
- Plugins now exposes install, edit, pause/resume, delete, refresh, search, and
  sort from the Plugin tab. Plugin data remains separate from MCP server data.
- Connectors now expose search, sort, refresh, and live test actions in the
  Directory layout.
- Added a standalone left-rail `Hooks` tab for lifecycle guardrails. It manages
  global hooks and per-mode hooks for tool use, permissions, sessions,
  compaction, user prompts, and subagents with add/edit/delete,
  enable/disable, matcher presets, file filters, decisions, timeouts, local
  persistence, scope filters, and export preview.
- Hook Manager actions emit structured `ordo.hooks.*` debug events, dispatch a
  browser-level `ordo:debug-event`, write to `console.debug`, persist a bounded
  local event trail, and surface that trail in the Hooks tab under
  `Debug / event logger`.
- Added Codex-style settings pages under the Settings index:
  - `General` for work mode, permissions, environment, shell, language, speed,
    code review, suggested prompts, import, licenses, and notifications.
  - `MCP servers` for connecting a custom MCP by STDIO or Streamable HTTP.
  - `Connections` for local-control posture and SSH device connection setup.
  - Placeholder settings pages for Profile, Appearance, Configuration,
    Personalization, Keyboard shortcuts, Browser, Computer use, Git,
    Environments, and Worktrees.
- Added agent/workspace surfaces for `Routines`, `Projects`, `Artifacts`, and
  `Archived chats`.
- Provider tab now includes `Anthropic Local Env` as a first-class quick
  provider. It saves an Anthropic-shaped credential with
  `auth_source=environment` and `env_var=ANTHROPIC_API_KEY`, so the runtime
  reads the key from the process environment at call time instead of asking the
  operator to paste or store an API key in Ordo. The configured row should show
  the `env:` marker, and the edit modal should show an Environment variable
  field instead of an API Key field.
- Provider tab now uses the simplified operator-facing layout:
  `Default providers` surfaces `Ollama`, `Ollama Cloud Models`, and `LM Studio` first,
  while `Other providers` contains Anthropic/Claude, OpenAI/Codex, Gemini,
  OpenRouter, and custom OpenAI-compatible APIs. `Ollama Cloud Models` is backed
  by the signed-in local Ollama daemon at `http://localhost:11434/v1` and prefers
  discovered `*-cloud` models. The separate `Ollama Cloud API` provider uses
  Ollama's OpenAI-compatible cloud surface at `https://ollama.com/v1` with an
  `OLLAMA_API_KEY` (not the native `https://ollama.com/api` shape).
- Provider credentials are runtime-owned data. The Provider tab must read
  `/api/cloud/credentials` first, even in packaged Tauri. The old Tauri
  placeholder command returned an empty list and made saved provider changes
  disappear from the UXI. It remains only as an offline empty fallback.
- Local provider auto-detect no longer depends on the Vite-only `/proxy/ollama`
  and `/proxy/lmstudio` paths. Packaged Tauri now uses a native localhost
  `/v1/models` probe for Ollama and LM Studio before falling back to dev proxy
  behavior.
- Added a settings-index `Remote Communication` surface. This is the single
  home for out-of-band command/reply channels:
  - `Email` is the first native channel, backed by `ordo-email` UI wiring for
    IMAP command intake, SMTP replies, command prefix, authorized sender list,
    field validation, and `ordo.email.*` debug events.
  - `Signal`, `Matrix`, `Telegram`, and `SMS` are surfaced as planned channel
    skeletons only. They emit `ordo.remote_communication.*` debug events, but
    do not install runtime bridges, plugins, MCP servers, or workflows until
    their native backends are deliberately built.
  The Tauri local connection catalog includes the `email` type so desktop/dev
  mode does not hide the channel when the HTTP control API is offline.
- The old Directory-style service catalog is now routed through `Connectors`.
  The settings-only `Connections` page is reserved for device/local/SSH
  connectivity so operator-facing service connectors and device connections do
  not share a tab.
- The Settings index must not list the Settings tab itself. Settings-managed
  pages expose a Back control that returns to the last non-settings tab and a
  Refresh control that emits `ordo.settings.settings_refresh_requested`; pages
  that have live local catalog reads should include the refresh key in their
  effects.
- General settings, custom MCP setup, SSH connection setup, routines, projects,
  artifacts, and archive actions dispatch browser-level `ordo:debug-event`
  records and write to `console.debug` using source names such as
  `ordo.settings.general`, `ordo.settings.mcp`, `ordo.settings.connections`,
  `ordo.routines`, `ordo.projects`, `ordo.artifacts`, and `ordo.archives`.
- These new pages are native React UXI surfaces. As of this note, most are
  frontend scaffolds with event logging; persistence/backend wiring should be
  added through `src/api.ts` and Tauri/runtime commands instead of separate UI
  windows.
- Every new tab added to this UXI must match Ordo's existing color scheme and
  component language. Use the Ordo dark surface, parchment text, restrained
  borders, and lamp/jade accent vocabulary instead of importing the palette of
  a reference screenshot.
- `src/api.ts` uses a local-first desktop read path for packaged Tauri and
  Tauri dev. In Tauri dev (`localhost:1420`), read/list endpoints and local
  session hydration must degrade quietly when the control API on
  `127.0.0.1:4141` is offline. Do not reintroduce repeated `failed to fetch`
  or Vite proxy-noise behavior for tabs that can show local/empty state.
- No removed workflow catalog entries or examples were added. Keep removed
  workflow categories out of the runtime, settings, plugins, MCPs, routines,
  and connector inventory.
- The implementation lives in `src/OrdoShell.tsx` and uses the existing
  `src/api.ts` runtime calls. Do not add a second Rust UI or another window for
  these controls.

## Things Not To Do

- Do not add a Rust-native UI as a second operator surface.
- Do not spawn a sidecar that opens its own window.
- Do not depend on `localhost` dev server URLs for packaged desktop builds.
- Do not hardcode absolute user-machine paths into source.
- Do not replace the canonical `OrdoShell.tsx` design with a parallel design
  system.
- Do not remove local inventory fallbacks just because the control API works on
  one machine.

## Fast Recovery Checklist

If the UXI breaks again:

1. Close all running Ordo processes.

```powershell
Get-Process Ordo -ErrorAction SilentlyContinue | Stop-Process -Force
```

2. Confirm there is only one Tauri window configured.

```powershell
Get-Content src-tauri\tauri.conf.json
```

3. Confirm Vite asset base is relative.

```powershell
Select-String -Path vite.config.ts -Pattern 'base: "./"'
```

4. Build the WebView assets.

```powershell
npm run build
```

5. Check the Tauri backend.

```powershell
npm run check:tauri
```

6. Rebuild the debug desktop app.

```powershell
npm run tauri build -- --debug
```

7. Launch the known binary.

```powershell
.\src-tauri\target\debug\Ordo.exe
```

8. Verify these surfaces:

- Assistant mode picker shows local modes.
- Plugins tab shows `user-files/plugins` entries.
- MCP tab shows `mcp-servers` packages and available tools.
- Settings index opens General, MCP servers, Connections, Hooks, Routines,
  Projects, Artifacts, and Archived chats.
- No extra Rust UI window opens.
- No external browser window opens.

## Provider Auth Rule

The Provider tab should default to environment-backed OpenAI API access:

- OpenAI API uses `OPENAI_API_KEY` from the environment that launches Ordo.
- OpenAI-compatible fallbacks should also prefer environment variables
  (`OPENROUTER_API_KEY`, `GROQ_API_KEY`, or an operator-selected env var).
- Do not surface every OpenAI-compatible vendor as its own card. The visible
  provider tab should show only `OpenAI API` by default, plus a `Customize API`
  button. Custom endpoints are created through the modal's API-shape dropdown,
  then listed under configured providers and made selectable in the provider
  dropdown.
- Ordo should not ask the operator to paste an API key unless the provider is
  not compatible with the env-backed OpenAI-style path or the operator chooses a
  custom stored-key provider.
- The runtime secret resolver in `ordo-cloud` must support env-backed bearer,
  basic, header-key, query-key, and Anthropic auth paths. Do not make this only
  a UXI label.
- Provider actions should emit `ordo.provider` debug events so the event logger
  shows template selection, saves, deletes, and test results.

### Operator API Key Wizard

The API key installer is operator-only secret plumbing, not a model-facing
runtime tool.

- The visible Provider tab still shows only `OpenAI API`, `Customize API`, and
  configured provider rows. It must not reopen the old provider catalog by
  default.
- The wizard may create or update a provider profile, but that profile stores
  only environment metadata such as `auth_source=environment` and `env_var`.
  It must never store the raw key in the provider extras, debug events, MCP
  tool output, chat context, or logs.
- Do not publish a debug/event-log entry for the raw key install action. Normal
  provider save, delete, selection, and test events can remain logged because
  they do not contain the key.
- The native command auto-detects the OS:
  - Windows: updates the current process, writes the user environment variable
    under HKCU `Environment`, and writes Ordo's local env file.
  - Linux/macOS: updates the current process and writes Ordo's local env file
    under `$XDG_CONFIG_HOME/ordo/env/api-keys.json` or
    `$HOME/.config/ordo/env/api-keys.json`.
- The runtime resolver reads the launched process environment first, then
  Ordo's local env file. This makes packaged Ordo work even when a shell profile
  was not edited.
- The wizard result may show platform, env var, and local install path. It must
  never echo the key back to the operator.

## Optional RAG Catalog For Modes

The Modes tab owns an optional RAG catalog for domain-specific knowledge such
as physics, chemistry, RF/signals, medicine, humanities, and business.

- Catalog entries are definitions only. They do not create folders, indexes,
  crawlers, or storage reservations by existing in the catalog.
- When a domain is added to a mode, it starts as `enabled=false` and
  `storageLimitMb=0`.
- Disabled optional RAGs must not be retrieved from and must not consume
  storage.
- Enabling an optional RAG should assign a positive storage budget; disabling
  it should return the budget to `0`.
- Duplicate labels across parent groups are canonicalized to one internal
  `rag.*` ID with multiple parent groups. For example, a field may appear under
  both Biology and Medicine without becoming two different datasets.
- Assistant turns include `rag_storage_budget_mb` and only enabled optional RAGs
  in `metadata.optional_rag_domains`. Runtime retrieval should treat that
  metadata as the active-mode allowlist when the backend enforcement is wired.

## Cross-Mode Collaboration Rule

Mode collaboration is agent consultation, not RAG or memory sharing.

- The active mode must never read another mode's RAG collections directly.
- The active mode must never import another mode's memory scope directly.
- If a task needs cross-domain expertise, Ordo should consult the target mode's
  agent inside that target mode's own boundary, then return a bounded answer to
  the active mode.
- Planner/classifier recommendations should surface as suggestions or approval
  requests according to the active mode's collaboration policy.
- User-requested collaboration is a one-turn request. It does not permanently
  link the modes.
- Turn metadata uses `cross_mode_collaboration.mechanism =
  "consult_mode_agent"` and `isolation = "no_cross_rag_or_memory_borrow"` for
  this reason.
- Do not build future collaboration UX around raw `assistant.borrow_from_mode`
  behavior unless that backend path is changed to an agent-consultation
  implementation. Raw memory/RAG borrow risks mode contamination.

## Chat Markdown Export

The chatbox includes a Markdown export action for the current visible
conversation.

- Export is client-side and uses the in-memory chat transcript so it still works
  when the runtime control API is offline.
- The export includes session id, active mode, timestamp, message role, message
  time, visible message text, and visible message metadata.
- It does not export provider secrets, API keys, hidden runtime state, raw RAG
  stores, or mode-private data beyond what is already visible in the current
  conversation.
- Keep this as a download/export action in the chatbox toolbar, not a model
  tool. The model should not need file-system write access to export the user's
  visible conversation.

## Chat Context, Model, And Thinking Strip

The strip directly above the chatbox is an operator status strip, not a message
preview.

- The Assistant surface should stay vertically tight. Keep only a compact
  `ordo planner` locator above the mode/consult rail; do not restore the large
  "conversation is the control surface" headline/subtitle block.
- The mode/consult rail belongs near the top of the Assistant tab so the
  conversation surface gets the vertical room.
- The chat composer is a multiline writing surface. Enter sends; Shift+Enter
  inserts a newline.
- Do not render `ASSISTANT` or a last-message preview in that strip. It is
  reserved for context usage, model choice, thinking effort, and compaction
  status.
- The context meter is an estimate from the visible in-memory conversation plus
  the current draft input. It is not an exact provider tokenizer count.
- The context budget comes from the active credential's `extras.context_window`
  when available, with a 128k fallback when no provider is configured or the
  credential list is unreachable.
- The backend already has mechanical prompt compaction in
  `ordo-assistant/src/prompt.rs` through `CompactionConfig`. The UXI surfaces
  this as `auto compact`. Do not add a manual compact button until the runtime
  exposes a real manual compaction command.
- The thinking dropdown is an operator preference with `off`, `medium`, and
  `high`. It is persisted in `localStorage` as `ordo:thinking_effort` and sent
  on each assistant turn as `metadata.thinking_effort` plus
  `metadata.reasoning_effort`.
- The model dropdown edits the active provider credential's `extras.model`
  without touching secrets. It uses configured/default model values and live
  local discovery for Ollama/LM Studio where available. Cloud model discovery
  should be wired here when the runtime exposes provider model listing.
- Assistant turn metadata includes `requested_model` and `context_estimate` so
  debug/event logs match the operator-visible strip.

## Workspace Selection And Sandbox

The Projects tab owns the active workspace scope for assistant turns.

- Supported scopes are `ordo` internal, `local` project folder, and `cloud`
  project reference.
- The Assistant rail displays the current workspace next to mode/consult so the
  operator sees the active boundary before sending.
- Changing workspace scope clears the foreground session id. This prevents a
  chat from silently spanning unrelated project roots.
- Local folder selection uses the native Tauri directory picker when available,
  with a manual path field as fallback.
- When the active scope is local or cloud, assistant turns send
  `use_rag=false` and include `metadata.workspace_scope.retrieval` with
  `disable_internal_rag_by_default=true`.
- Sandbox policy is sent under `metadata.workspace_scope.sandbox`: selected
  root, no parent traversal, no outside-root access, and explicit write opt-in.
- Runtime/tool adapters must enforce the sandbox boundary before reading or
  writing project files. Do not treat the UXI folder picker alone as sufficient
  filesystem isolation.
- Cloud workspace references currently capture provider plus repo/dataset id.
  Future GitHub/Hugging Face sync/indexing should consume the same
  `workspace_scope` shape rather than adding a parallel project selector.

## Docs Rail Placement

Docs are first-class left-rail tabs, not Settings-only surfaces.

- `Docs` is for operator usage documentation: getting started, workspaces,
  modes, skills/plugins/MCP, hooks, and export behavior.
- `Dev Docs` is for development/recovery documentation: UXI notes, project
  rules, architecture maps, verification gates, sandbox enforcement, and design
  rules.
- Both tabs live in the `docs` rail group, rendered after Advanced, so they sit
  at the very bottom of the left rail.
- Keep `Docs` and `Dev Docs` surfaced in `LEFT_RAIL_TAB_IDS`; do not bury them
  inside Settings.

## Mid-Task Steering Controls

The Assistant composer remains writable while an assistant turn is active.

- Pressing Send during an active turn opens a modal with three choices:
  `Steer`, `Queue next`, and `Interrupt and send`.
- `Steer` places the message at the front of the local next-turn queue and
  marks metadata with `mid_task_action=steer` plus `steering_guidance=true`.
  This is the UXI contract for live steering; until the runtime exposes a true
  mid-token steering lane, it is prioritized next-turn guidance.
- `Queue next` appends the message to the local queued-turn list with
  `mid_task_action=queue`.
- `Interrupt and send` calls `cancelAssistantTurn(session_id)`, marks the
  visible streaming bubble interrupted, queues the replacement instruction at
  the front, and logs the action as a warning-level debug event.
- Every choice publishes through `publishUxiDebugEvent("ordo.assistant", ...)`
  so the Debug/Event logger can explain what happened.

## Theme And Cross-OS Compatibility

Ordo defaults to dark mode, but the Appearance settings surface owns a persisted
bright-mode toggle.

- Theme preference is stored in `localStorage` as `ordo:theme`.
- The shell applies `ordo-theme-dark` or `ordo-theme-bright` and all shared UXI
  surfaces should use CSS variables from `src/index.css` / `src/ui.tsx`, not
  hard-coded one-off colors.
- New tabs must work in both themes. Use `UI.cardBg`, `UI.cardBgRaised`,
  `UI.cardBorder`, `UI.inputBg`, `UI.parchment`, `UI.textMuted`, and
  `UI.primary*` tokens.
- Dark remains the first-run default because it is Ordo's primary aesthetic.

Cross-OS support should be treated as a release discipline:

- Keep the UXI web layer OS-neutral. Do not hard-code `C:\`, PowerShell, `.exe`,
  backslashes, or Windows-only assumptions into React state or labels unless the
  selected environment is Windows.
- Runtime and Tauri helpers should detect Windows, Linux, or macOS before
  writing environment variables, launchers, paths, shell commands, or app
  locations.
- Prefer Tauri APIs, Rust `PathBuf`, and typed platform adapters over string
  concatenation for filesystem paths.
- Store user project locations as opaque paths and display labels separately.
- Verify release builds on each OS target with the corresponding Tauri bundle
  command, not only `tauri dev` on Windows.

Build expectations remain: `npm run build` and `npm run check:tauri` should
exit cleanly before calling this surface done.
