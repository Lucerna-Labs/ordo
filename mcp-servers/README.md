# MCP servers

Each subdirectory is a standalone Rust crate that compiles to a WASM module,
packaged with a manifest, installable into Ordo's MCP runtime via
`ordo mcp install` or `mcp.servers.install`.

## Available packages

| Package | What it does |
|---|---|
| `capstone/` | Binary disassembly support. |
| `exe/` | Executable metadata inspection and verification. |
| `fasttext/` | Local text classification helpers. |
| `goblin/` | Binary parsing, symbols, and library inspection. |
| `mitre-atlas/` | AI threat technique detection and audit helpers. |
| `ort/` | Local ONNX Runtime classification and embedding helpers. |
| `syara-x/` | Semantic YARA-style scanning for prompt/security threats. |

## Package boundary

This platform build keeps the MCP tab limited to local server, security,
analysis, and tooling packages. Non-tooling publication or channel automation
packages do not belong in this package set.

## Build a package

```bash
cargo build --release --target wasm32-unknown-unknown \
  --manifest-path mcp-servers/<name>/Cargo.toml
```

The output is package-specific under:

```text
mcp-servers/<name>/target/wasm32-unknown-unknown/release/
```
