# Plugins

Ordo plugins are **MCP servers** â€” subprocesses that speak
[Model Context Protocol](https://modelcontextprotocol.io) over stdin/
stdout. The host (Ordo) spawns the plugin, runs the MCP
`initialize` + `tools/list` handshake, and registers every advertised
tool as a first-class capability on the shared bus. To the rest of the
runtime, a plugin-provided tool is indistinguishable from a built-in.

This is the "browser extension for an AI runtime" story: drop a folder
into your plugins directory, review the manifest, enable it, restart
the runtime, and your new capabilities show up in `/api/capabilities`
next to the built-ins.

## Where plugins live

The default plugin directory is **`user-files/plugins/`** (per-workspace,
travels with the rest of your local state). Each immediate subdirectory
containing a `plugin.json` is treated as one plugin.

```
user-files/
  plugins/
    seo-advanced/
      plugin.json
      server.py
    brand-voice/
      plugin.json
      brand-voice.exe
```

Override via `ORDO_PLUGINS_PATH=/path/to/plugins` if you prefer a
shared plugin set across workspaces (e.g. `~/.ordo/plugins/`).

## The `plugin.json` manifest

```json
{
  "name": "seo-advanced",
  "version": "0.2.0",
  "description": "Keyword density, SERP preview, schema.org linting",
  "command": "python",
  "args": ["server.py"],
  "expected_lanes": ["seo.", "example."],
  "required_env": ["OPENAI_API_KEY"],
  "env": {
    "SEO_INDEX_URL": "https://internal.example/seo-index"
  },
  "enabled": false
}
```

| Field | Required | Description |
|---|---|---|
| `name` | yes | Unique identifier. Lowercase ASCII letters, digits, `-`, `_`. |
| `version` | no | Plugin version string. Not validated; shown in UI. |
| `description` | no | One-liner shown in `ordo plugins list` and the studio. |
| `command` | yes | Executable to spawn. Relative paths resolve against the plugin's own directory; absolute paths are used as-is. |
| `args` | no | Arguments passed to `command`. |
| `expected_lanes` | no | Capability prefixes the plugin is allowed to contribute to (e.g. `["seo.", "creative."]`). **Must not** include reserved core prefixes (`cloud.`, `runtime.`, `filesystem.`, `memory.`, `knowledge.`, `self_heal.`) unless `core_override: true`. |
| `required_env` | no | Environment variables to forward from the parent process. Everything else is scrubbed. |
| `env` | no | Literal key/value env vars set on the child. Never use this for secrets â€” use the `cloud.*` credential vault instead. |
| `enabled` | no | Whether the plugin is allowed to run. Defaults to `true`, but `ordo plugins install` forces this to `false` on new installs so the operator has to explicitly approve. |
| `core_override` | no | Escape hatch for first-party plugins that need reserved prefixes. Leave unset unless you trust the plugin with core lanes. |

## Security model

Plugins run as **isolated subprocesses** with:

- **Cleared environment**: `env_clear()` is called before spawn. Only
  `PATH` + `SystemRoot` (on Windows) are restored, plus anything the
  manifest lists in `required_env`. The operator's API keys, cloud
  credentials, and personal env vars are never visible to a plugin
  unless they are explicitly named in the manifest.
- **Reserved-lane enforcement, twice**: the manifest is validated at
  load time, and every advertised tool name is re-checked at runtime.
  A plugin that tries to sneak in `cloud.anything` gets rejected.
- **Expected-lanes enforcement**: every tool the plugin advertises
  must fall under one of the declared prefixes. A plugin can't
  quietly register capabilities the operator didn't opt into.
- **Disabled-by-default on install**: `ordo plugins install` always
  lands the plugin with `enabled: false`. The operator has to run
  `ordo plugins enable <name>` after reviewing the manifest.
- **Kill-on-drop**: if the runtime exits, the subprocess is killed
  rather than orphaned.

The broader MCP/plugin security posture is research-informed and
defense-in-depth:

- **Signed lockfiles and provenance**: installed MCP servers should carry a
  stable record of identity, source, declared tools, and expected capability
  shape.
- **Trust graduation**: new servers start untrusted or disabled until reviewed.
  Trust is a state the operator grants, not a side effect of installation.
- **Drift re-authorization**: if a server changes its tool catalog or declared
  capability shape, Ordo should require re-authorization before treating the new
  surface as trusted.
- **Quarantine**: suspicious, invalid, or changed servers can be isolated from
  normal tool use while remaining inspectable.
- **Pre/post-call scanning**: arguments and results can be scanned for prompt
  injection, path escape, secret leakage, large exfiltration-shaped payloads,
  and other policy findings.
- **Redacted audit logs**: security findings should be operator-readable while
  hiding matched secret values.

## Operator workflow

```bash
# List what's installed (enabled or not)
ordo plugins list

# Install a local plugin directory (lands disabled)
ordo plugins install ./my-plugin

# Review the manifest, then enable it
ordo plugins enable my-plugin

# Remove
ordo plugins uninstall my-plugin
```

Equivalents through the control API:

```bash
curl -s http://127.0.0.1:4141/api/plugins
curl -s -X POST http://127.0.0.1:4141/api/plugins/my-plugin/enabled \
    -H 'content-type: application/json' \
    -d '{"enabled": true}'
curl -s -X DELETE http://127.0.0.1:4141/api/plugins/my-plugin/enabled
```

The studio has a **Plugins** tab with the same controls plus live
state (`active` / `disabled` / `failed` / `invalid`) and the exact
capability list each plugin contributes.

## Writing a plugin

Plugins don't have to be Rust â€” any language that can read/write
newline-delimited JSON on stdio works.

### Minimum MCP subset

You need to respond to three methods:

| Method | Kind | Response |
|---|---|---|
| `initialize` | request | `{ protocolVersion: "2024-11-05", capabilities: {tools: {}}, serverInfo: {name, version} }` |
| `notifications/initialized` | notification | No response. |
| `tools/list` | request | `{ tools: [{ name, description, inputSchema }, ...] }` |
| `tools/call` | request | `{ content: [{type: "text", text: "..."}], isError: false }` |

### Python skeleton

```python
import json, sys

def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()

def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        method = msg.get("method", "")
        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "my-plugin", "version": "0.1.0"},
                },
            })
        elif method == "notifications/initialized":
            pass  # no response
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "tools": [{
                        "name": "example.greet",
                        "description": "Say hi",
                        "inputSchema": {"type": "object"},
                    }],
                },
            })
        elif method == "tools/call":
            args = msg.get("params", {}).get("arguments", {})
            send({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "content": [{"type": "text", "text": f"Hello {args.get('name', 'world')}"}],
                    "isError": False,
                },
            })

if __name__ == "__main__":
    main()
```

Pair with a manifest:

```json
{
  "name": "my-plugin",
  "version": "0.1.0",
  "description": "Hello-world plugin",
  "command": "python",
  "args": ["server.py"],
  "expected_lanes": ["example."]
}
```

### Rust skeleton

See `ordo-plugins/src/bin/example-echo-plugin.rs` for a complete Rust
reference. It's ~80 lines, single-file, and compiles into a standalone
binary the manifest can point at directly â€” no Python needed.

## Reference plugins in this workspace

The `ordo-plugins` crate ships two reference plugins you can use as
templates:

- **`example-echo-plugin`** â€” one-tool (`example.echo`) MCP server that
  round-trips its arguments. The minimum viable plugin.
- **`example-brand-voice-plugin`** â€” contributes `brand.voice_check`
  that scores copy against hedge-word, clichÃ©, and exclamation-density
  heuristics. Shows how a plugin can introduce a **new lane** (`brand.*`)
  without touching core runtime code.

Build them:

```bash
cargo build -p ordo-plugins --bin example-echo-plugin
cargo build -p ordo-plugins --bin example-brand-voice-plugin
```

Install the echo one:

```bash
mkdir -p user-files/plugins/echo
cp target/debug/example-echo-plugin.exe user-files/plugins/echo/
cat > user-files/plugins/echo/plugin.json <<'EOF'
{
  "name": "echo",
  "version": "0.1.0",
  "description": "Reference echo plugin",
  "command": "example-echo-plugin.exe",
  "expected_lanes": ["example."],
  "enabled": true
}
EOF

ordo plugins list
ordo serve
curl -s -X POST http://127.0.0.1:4141/api/tools/example.echo \
  -H 'content-type: application/json' \
  -d '{"text": "hello plugins"}'
```

## Roadmap

- **Hot reload**: `ordo plugins reload <name>` restarts a single plugin
  subprocess without bouncing the runtime.
- **URL-based install**: `ordo plugins install https://...` that
  downloads, validates, and installs a signed plugin bundle.
- **Plugin marketplace**: a curated index (`plugins.ordo.dev`)
  with discoverability through the studio.
- **Capability approval**: per-capability (not just per-plugin) allow
  lists so an operator can accept `seo.suggest_metadata` but decline
  `seo.submit_to_google`.
- **Structured log surfacing**: `/api/plugins/:name/logs` tailing
  stderr so operators can diagnose failing plugins from the studio.
