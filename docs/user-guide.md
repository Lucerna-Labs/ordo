# Ordo User Guide

This guide explains Ordo from the operator point of view.

## Start Ordo

On Windows, use the Servo launcher:

```powershell
.\Launch-Ordo-Servo.ps1
```

Or double-click:

```text
Launch-Ordo-Servo.cmd
```

The launcher starts the runtime, serves the Studio bundle from the local Ordo
control API, and opens the embedded Servo shell.

Default local URL:

```text
http://127.0.0.1:4141
```

## Main Idea

Ordo is designed so the assistant is useful without being given uncontrolled
access to the computer. It works through modes, skills, policies, review gates,
logs, and explicit user approval.

Important concepts:

- Provider setup controls which local or cloud models Ordo can use.
- General mode is the default everyday assistant mode.
- Agent Teams are for bounded multi-agent work.
- Tech Specialist handles setup, diagnostics, installs, and maintenance.
- Automation handles routines, hooks, cron-style schedules, webhooks, local
  events, and dreaming reviews.
- Remote Communication handles email and future communication channels.
- RAG and persistent memory help Ordo remember and retrieve useful local
  context.
- The artifact side view lets agents show files and outputs while chat stays
  visible.

## Assistant

Use Assistant for normal conversation and task steering.

The assistant surface includes:

- session selector
- mode selector
- workspace selector
- model selector
- thinking selector
- context usage indicator
- Agent Team working indicator when a team is active
- chat input
- file/image/folder upload controls
- dictation controls
- stop/interrupt behavior while a task is running
- artifact side-view support

Voice output and avatar-specific controls belong on the Avatar surface. The
assistant chatbox should only keep dictation and normal chat controls.

## Provider

Use Provider to configure local and cloud models.

Supported provider directions include:

- Ollama local
- Ollama Cloud/OpenAI-compatible
- LM Studio local
- OpenAI-compatible custom endpoints
- cloud providers configured through Ordo's credential system

When switching local providers or models, Ordo should unload/eject the previous
active local model before selecting the next one. This prevents LM Studio and
Ollama from leaving multiple heavy models loaded at the same time.

## Modes

Modes are scoped work environments. Each mode can have its own behavior,
memory, tools, and skills.

General mode should be selected by default when Ordo launches.

Common mode directions:

- General
- Rust/coding
- Research
- Security
- Business
- Dreaming
- Tech Specialist

Modes can consult one another, but one mode should not silently read another
mode's private memory or RAG.

## Agent Teams

Agent Teams let Ordo coordinate multiple specialist roles on a bounded task.

Use Agent Teams when a job naturally benefits from multiple roles, such as:

- planner
- builder
- reviewer
- researcher
- critic

Small local models should use smaller teams and narrower tasks. Larger local
models and flagship cloud models can handle richer team structures.

Each team role should have its own instructions and relevant skills.

## Skills

Skills are instruction packs.

Ordo supports:

- global skills
- per-mode skills
- Tech Specialist maintenance skills
- Agent Team setup skills

This prevents the active assistant from being overloaded with every possible
skill at once.

## Automation

Automation owns scheduled and event-driven work.

Automation includes:

- routines
- hooks
- cron-style jobs
- heartbeats
- webhooks
- local events
- dreaming reviews
- bounded coding automation

Automation should surface clear buttons for the automation type being created
instead of hiding everything behind a single vague dropdown.

## Hooks

Hooks are lifecycle guardrails. They can run before or after important events
such as tool use, permission requests, session starts, subagent starts, and
compaction.

Hooks now belong under Automation, and Tech Specialist should be able to help
users create, inspect, and troubleshoot hooks.

## Dreaming

Dreaming is Ordo's review and self-learning lane.

It can review:

- failed tasks
- corrections
- rejected outputs
- logs
- completed work
- recurring issues

Dreaming can propose lessons. It should not silently rewrite durable memory or
system behavior.

## Tech Specialist

Tech Specialist is the user-friendly maintenance mode for Ordo.

Ask Tech Specialist for help with:

- diagnostics
- logs
- model/provider setup
- MCP servers
- plugins
- apps
- skills
- webhooks
- automation
- hooks
- Agent Teams
- avatar setup
- SSH keys
- API keys
- local computer read/write tasks, when explicitly allowed

Tech Specialist should not see secrets directly. Secret entry and storage
belong to dedicated vault/UI paths.

Local computer access is denied by default. When access is needed, Ordo should
use explicit allow/deny UI instead of treating natural language as permission.

## Remote Communication

Remote Communication is where users configure communication accounts.

Email accounts should have account-level access controls:

- none
- read
- write

Ordo may also have its own remote communication identity if the operator wants
remote instructions or remote status messages. Signal and Telegram are planned
directions. SMS is intentionally excluded.

## Avatar

Avatar is the place for voice output, avatar behavior, appearance, and
companion-style customization.

Tech Specialist should be able to help configure the Avatar surface because
avatar setup can be difficult even for technical users.

## Review

Review is the human approval lane. Sensitive actions can require approval
before they complete.

Use Review when a workflow needs explicit human confirmation, edits, or denial.

## Settings

Settings belongs at the bottom of the rail. It should hold less-common setup
surfaces and manual controls for users who prefer to configure things
themselves.

Common settings can include:

- avatar
- plugins
- builds
- MCP
- projects
- Agent Teams
- extensions

## Logs And Events

Ordo should log:

- user actions
- provider choices
- model lifecycle decisions
- tool calls
- capability denials
- permission decisions
- retries
- failures
- recovery steps
- automation runs
- Agent Team activity

Logs should not include secrets.

## Validation

Use the standard test script when checking an Ordo build:

```powershell
.\Test-Ordo-Functions.ps1 -Suite standard -KeepGoing
```
