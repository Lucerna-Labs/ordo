# UI Extensions

UI extensions are optional frontend panels served by Ordo and mounted inside
the Studio UXI. They are for visual tools such as viewers, editors, dashboards,
or artifact helpers.

UI extensions are separate from MCP plugins:

- UI extensions provide interface surfaces.
- MCP plugins provide backend tools.
- A UI extension may call approved tools through the parent Studio bridge.

## Where Extensions Live

```text
user-files/ui-extensions/
```

Each extension lives in its own folder with a `ui.json` manifest.

Example:

```text
user-files/
  ui-extensions/
    artifact-viewer/
      ui.json
      index.html
      viewer.js
      style.css
```

## Manifest Example

```json
{
  "name": "artifact-viewer",
  "version": "0.1.0",
  "description": "Preview local artifacts inside Ordo.",
  "surfaces": [
    {
      "kind": "tab",
      "id": "viewer",
      "label": "Artifacts",
      "entry": "index.html"
    }
  ],
  "permissions": {
    "mcp_tools": ["artifact.*", "files.read_file"],
    "subscribe_events": []
  },
  "enabled": true
}
```

## Security Rules

- Extension files are served from their own directory only.
- Path traversal is rejected.
- Tool calls are brokered by the parent Studio.
- The parent checks the extension manifest before forwarding calls.
- Extension permissions should be narrow.
- Secrets should not be exposed to extensions.

## Current Placement

UI Extensions should not occupy prime left-rail space by default. Keep the
extension management surface under Settings, with Tech Specialist able to guide
or manage setup through approved tools.

## Retired Examples

Do not use retired `creative.*`, `seo.*`, or `cms.*` capability examples in new
UI extension docs or sample manifests.
