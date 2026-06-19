# Plugins

Ordo plugins are MCP-backed capability providers. They can add tools to the
runtime, but they are untrusted until inspected and approved.

## Where Plugins Live

Default directory:

```text
user-files/plugins/
```

Each plugin lives in its own folder with a `plugin.json` manifest.

Example:

```text
user-files/
  plugins/
    local-doc-tools/
      plugin.json
      server.py
```

## Manifest Example

```json
{
  "name": "local-doc-tools",
  "version": "0.1.0",
  "description": "Local document helper tools.",
  "command": "python",
  "args": ["server.py"],
  "expected_lanes": ["artifact.", "files."],
  "required_env": [],
  "env": {},
  "enabled": false
}
```

## Manifest Rules

- `name` must be unique.
- `command` is the executable to spawn.
- `args` are passed to the command.
- `expected_lanes` declares which capability prefixes the plugin may provide.
- `enabled` should default to false for newly installed plugins.
- Secrets should not be placed in `env`; use Ordo credential/vault paths.

Plugins must not contribute reserved or sensitive core lanes unless explicitly
trusted as first-party system components.

## Security Model

Ordo's plugin/MCP posture includes:

- disabled-by-default install flow
- manifest review
- expected-lane enforcement
- reserved-lane enforcement
- trust states
- quarantine
- drift checks when tool catalogs change
- provenance tracking
- pre/post-call scanning
- redacted audit logs
- kill-on-runtime-exit process cleanup

## Tech Specialist

Normal Assistant mode should not install or modify plugins. Tech Specialist may
help install, inspect, enable, disable, repair, or remove plugins through
approved maintenance capabilities and explicit operator approval.

Manual plugin controls should remain available for users who prefer to manage
plugins themselves.

## Retired Examples

Do not use retired `creative.*`, `seo.*`, or `cms.*` lanes in new plugin
examples. Those domains are not part of the current Ordo beta direction.
