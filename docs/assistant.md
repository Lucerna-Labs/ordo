# Assistant

The Assistant is the main operator-facing layer of Ordo. It owns chat sessions,
turn execution, memory recall, skill routing, model calls, tool use, and
operator-visible progress events.

## Responsibilities

- Keep conversation sessions durable.
- Start in General mode by default.
- Respect the active mode's instructions, memory scope, tools, and skills.
- Load only relevant global and mode-specific skills.
- Route provider/model calls through Ordo's provider layer.
- Preserve review, security, and permission gates.
- Surface context usage and active Agent Team state.
- Support stop/interrupt behavior for active turns.
- Keep dictation in the chatbox while avatar voice output stays on Avatar.

## Turn Loop

A normal turn follows this shape:

1. Read the active session, mode, workspace, and model choice.
2. Gather relevant memory and RAG context.
3. Load global skills plus active-mode skills.
4. Build the model prompt.
5. Call the selected local or cloud provider.
6. Execute approved tool calls through Ordo capabilities when allowed.
7. Stream or persist the assistant response.
8. Log provider decisions, tool calls, errors, and recovery actions.

Sensitive or high-risk work can route through Review or Tech Specialist rather
than normal general-assistant execution.

## Memory And RAG

The Assistant can use:

- session history
- persistent facts
- pinned memory
- working memory
- RAG/self-knowledge collections

When no embedding model is configured, Ordo uses the hashing fallback. The user
should be told when an embedding model would improve retrieval quality.

## Skills

Skills are not dumped into every prompt. Ordo separates:

- global skills
- per-mode skills
- Tech Specialist maintenance skills
- Agent Team role skills

This keeps small models usable and reduces irrelevant instructions.

## Provider And Model Choice

The Assistant uses the active model selected through Provider controls.

Provider switching should:

- save the selected provider/model
- unload/eject the previous local model when supported
- avoid leaving LM Studio and Ollama models loaded at the same time
- log lifecycle outcomes
- report failures clearly in the UI

## Agent Teams

When an Agent Team is active, the assistant surface should show a clear visual
indicator around or inside the chat composer. Team roles can have separate
instructions and skills.

Agent Teams should work with both local and cloud models, but small local
models should use smaller teams and narrower tasks.

## Boundaries

The general Assistant should not install or modify:

- MCP servers
- plugins
- apps
- webhooks
- SSH keys
- API keys
- local computer access rules
- core security settings

Those tasks belong to Tech Specialist with explicit approval and safe manual
controls.

## Voice Boundary

The Assistant chatbox keeps dictation/STT controls. Voice output, TTS model
selection, avatar pop-out, and avatar behavior belong on the Avatar surface.

## Failure Behavior

Failures should be shown as useful operator-facing messages, not raw provider
blobs when avoidable. Logs should retain enough detail for Tech Specialist to
diagnose without exposing secrets.
