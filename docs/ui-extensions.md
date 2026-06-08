# UI Extensions

UI extensions are sandboxed iframes that mount into the Ordo
studio as new tabs, panels, or overlays. A word editor, image viewer,
kanban board, or brand-voice dashboard can all ship as a folder of
static HTML/JS/CSS with a manifest â€” no core rebuild, no source
changes to the studio.

UI extensions are the **frontend** sibling of MCP plugins. MCP plugins
are backend (subprocess, JSON-RPC, tool-call-centric). UI extensions
are frontend (iframe, postMessage, visual-centric). They pair: a word
editor is typically a UI extension that calls backend tools the core
already exposes, or tools an MCP plugin contributed.

## Data flow

```
  â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
  â”‚ Studio (Tauri or browser)                                    â”‚
  â”‚                                                              â”‚
  â”‚   <iframe                                                    â”‚
  â”‚     sandbox="allow-scripts"                                  â”‚
  â”‚     src="/api/ui-extensions/word-editor/files/index.html"> â—„â”€â”¼â”€â”€ control API
  â”‚   </iframe>                                                  â”‚   serves the
  â”‚          â–²                                                   â”‚   static files
  â”‚          â”‚ postMessage                                       â”‚   with its
  â”‚          â–¼                                                   â”‚   sandbox guard
  â”‚   ExtensionHost (parent)                                     â”‚
  â”‚     â”œâ”€ validates MessageEvent.source === iframe.contentWindowâ”‚
  â”‚     â”œâ”€ checks manifest.permissions.mcp_tools                 â”‚
  â”‚     â”œâ”€ forwards allowed tool.call â†’ POST /api/tools/:cap     â”‚
  â”‚     â”œâ”€ checks manifest.permissions.subscribe_events          â”‚
  â”‚     â”œâ”€ bridges allowed topics from /ws/review                â”‚
  â”‚     â””â”€ posts result / event back to iframe                   â”‚
  â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
```

**The iframe never touches the runtime directly.** Every request is
mediated by the parent studio, every request is validated against the
manifest. `sandbox="allow-scripts"` (no `allow-same-origin`, no
`allow-forms`, no `allow-popups`, no `allow-top-navigation`) is the
tightest sandbox that still allows JavaScript.

## Where extensions live

The default extension directory is **`user-files/ui-extensions/`**,
overridable with `ORDO_UI_EXTENSIONS_PATH`. Each immediate
subdirectory containing a `ui.json` is one extension:

```
user-files/
  ui-extensions/
    word-editor/
      ui.json
      index.html
      editor.js
      style.css
    image-viewer/
      ui.json
      index.html
```

## `ui.json` manifest

```json
{
  "name": "word-editor",
  "version": "0.1.0",
  "description": "Minimal markdown editor with live preview.",
  "author": "Ordo reference",
  "core_override": true,
  "surfaces": [
    {
      "kind": "tab",
      "id": "editor",
      "label": "Word",
      "entry": "index.html",
      "description": "Type markdown on the left, see the preview on the right."
    }
  ],
  "permissions": {
    "mcp_tools": [
      "filesystem.write_file",
      "creative.generate_copy"
    ],
    "subscribe_events": []
  },
  "enabled": true
}
```

| Field | Required | Description |
|---|---|---|
| `name` | yes | Unique id. Lowercase ASCII letters, digits, `-`, `_`. |
| `version` | no | Shown in the extension card. |
| `description` / `author` | no | Metadata for the extension host tooltip. |
| `surfaces` | yes | One or more UI surfaces the extension contributes. Today only `kind: "tab"` is recognized. |
| `permissions.mcp_tools` | no | Capability ids (or `lane.*` globs) the extension may call. An empty list means "read-only extension." |
| `permissions.subscribe_events` | no | Event topic patterns. Today the only bridged stream is `review.*`. |
| `core_override` | no | Bypass the reserved-lane guard. Only meaningful for first-party extensions that legitimately need `cloud.*`, `filesystem.*`, etc. |
| `enabled` | no | Manifest-level kill switch. Disabled extensions don't appear in the studio. |

### Surface: tab

```json
{
  "kind": "tab",
  "id": "editor",
  "label": "Word",
  "icon": "icon.svg",
  "entry": "index.html",
  "description": "â€¦"
}
```

- `id` â€” unique within the extension; combined with the extension name
  to produce the tab key (`ext:word-editor:editor`)
- `label` â€” text shown in the tab bar
- `icon` â€” optional static file inside the extension directory
- `entry` â€” the HTML file to load in the iframe; must be a relative
  path inside the extension directory (traversal rejected)

## Security model

UI extensions run under the tightest practical boundary:

- **`sandbox="allow-scripts"` only.** No same-origin, no forms, no
  popups, no top navigation. The iframe's origin is opaque.
- **Every request is brokered.** The iframe can't `fetch()` anything
  useful, so it has to go through `window.ordo.*`, which posts a
  message the parent studio validates.
- **Permission enforcement in the parent.** Before forwarding a
  `tools.call`, the ExtensionHost matches the capability against the
  manifest's `permissions.mcp_tools`. Wildcards (`creative.*`) match by
  prefix.
- **Reserved lanes.** `cloud.*`, `runtime.*`, `filesystem.*`,
  `self_heal.*`, `memory.*`, `knowledge.*` are core-only; a manifest
  declaring them fails to load unless `core_override: true` is set.
- **Static-file sandbox.** The `/api/ui-extensions/:name/files/*path`
  route rejects absolute paths and `..` traversal; files served always
  live under the extension's own directory.
- **Sandboxed static files too.** The static-file handler sets
  `Cache-Control: no-cache` so manifest changes take effect on reload.

This is not a sandbox escape prevention layer â€” if you install a UI
extension written by an adversary, the JavaScript runs, and anything
your manifest granted is fair game. It is a defense against
**accidental over-reach** and against extensions that try to do more
than they declared.

## The `window.ordo` bridge API

The parent studio injects no globals directly; instead, extensions
load a shared bridge script:

```html
<script src="/api/ui-extensions/_bridge.js"></script>
```

After that loads, `window.ordo` is available:

```js
// Invoke any permitted capability. Resolves with the parsed JSON
// response, or rejects with an Error whose message is either the
// permission denial from the parent or the control-API error text.
const result = await window.ordo.tools.call("creative.generate_copy", {
  prompt: "spring colorway launch post",
});

// List capabilities the current extension is allowed to call.
const { capabilities } = await window.ordo.tools.list();

// Subscribe to a permitted event topic.
const unsubscribe = window.ordo.events.on("review.opened", (payload) => {
  console.log("new review request:", payload.request);
});
// ...later
unsubscribe();

// Close the extension's tab.
window.ordo.ui.close();

// Show a toast in the parent.
window.ordo.ui.toast("Saved", "success");

// Read the manifest the parent loaded you under.
const manifest = await window.ordo.ready;
console.log(manifest.name, manifest.version);
```

### Message envelope

Requests carry an `id`; responses correlate by id. Everything else is
notifications.

| Direction | Shape | Purpose |
|---|---|---|
| child â†’ parent | `{id, type: "call", method, params}` | Tool call / list |
| parent â†’ child | `{id, type: "result", result}` | Success response |
| parent â†’ child | `{id, type: "error", error}` | Failure (permission or upstream) |
| child â†’ parent | `{type: "subscribe"\|"unsubscribe", topic}` | Event stream |
| parent â†’ child | `{type: "event", topic, payload}` | Pushed event |
| child â†’ parent | `{type: "hello"}` | Child ready |
| parent â†’ child | `{type: "ready", manifest}` | Manifest handshake |
| child â†’ parent | `{type: "ui.close"}` | Ask parent to close tab |
| child â†’ parent | `{type: "ui.toast", text, tone}` | Surface a toast |

Unknown methods return `error: "unknown method ..."`. Parent-initiated
events arriving for unsubscribed topics are silently dropped.

## Control API surface

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/ui-extensions` | Manifest inventory |
| GET | `/api/ui-extensions/_bridge.js` | Shared bridge script (served inline) |
| GET | `/api/ui-extensions/:name/files/*path` | Static file serving, sandboxed to the extension dir |

The manifest list response shape:

```json
{
  "extensions_dir": "â€¦/user-files/ui-extensions",
  "extensions": [
    {
      "name": "word-editor",
      "version": "0.1.0",
      "surfaces": [
        {
          "kind": "tab",
          "id": "editor",
          "label": "Word",
          "entry_url": "/api/ui-extensions/word-editor/files/index.html",
          "description": "â€¦"
        }
      ],
      "permissions": { "mcp_tools": [...], "subscribe_events": [...] },
      "enabled": true
    }
  ],
  "errors": []
}
```

Malformed manifests land in `errors` with the parse error, so the
studio can show "this extension failed to load" without blocking
every other extension.

## Authoring an extension

Minimum viable structure:

```
my-extension/
  ui.json
  index.html
```

With any text editor, no build step required:

```html
<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>My extension</title>
    <script src="/api/ui-extensions/_bridge.js"></script>
  </head>
  <body>
    <button id="go">Generate</button>
    <pre id="out"></pre>
    <script>
      document.getElementById("go").addEventListener("click", async () => {
        try {
          const result = await ordo.tools.call("creative.generate_copy", {
            prompt: "one sentence about spring running",
          });
          document.getElementById("out").textContent =
            result.assistant_message ?? JSON.stringify(result, null, 2);
        } catch (err) {
          document.getElementById("out").textContent = String(err);
        }
      });
    </script>
  </body>
</html>
```

Drop the folder into `user-files/ui-extensions/`, restart the studio
(or refresh the extensions list via the parent's refresh button when
hot-reload lands), and a new tab appears.

## Reference extension

`examples/ui-extensions/word-editor/` ships as a working example â€” a
minimal markdown editor with live preview, a Save button that calls
`filesystem.write_file`, and a Draft button that calls
`creative.generate_copy`. Copy it into `user-files/ui-extensions/` to
see the whole loop.

## Roadmap

- **More surface kinds**: `panel` (right-side dock) and `overlay`
  (modal).
- **Hot reload**: a `reload` command the studio sends when the
  manifest changes on disk.
- **Extension registry**: same story as the MCP plugin marketplace â€”
  one-click install from a curated index.
- **Bundle format**: a `.ordo-ui-extension.tar.gz` archive the
  installer fetches + validates by SHA-256.
- **Granular event bridging**: today the only bridged stream is
  `review.*`. Security-audit, plugin-status, and capability-changed
  events are candidates.
- **Shared assets**: a `window.ordo.assets.get(path)` that returns a
  data URL for files inside another (declared) extension, enabling
  shared libraries.
