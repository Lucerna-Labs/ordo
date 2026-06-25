# Contributing to Ordo

Ordo is in active beta development. Thanks for being here.

## Reporting problems

The most valuable thing you can do right now is tell me what's broken.

**[→ Open an issue](https://github.com/Lucerna-Labs/ordo/issues)**

For bugs, include:
- Steps to reproduce
- What happened vs. what you expected
- OS and install method
- Error output or `runtime-servo.err.log` contents

## Feature ideas

Open an issue with `[FEATURE]` in the title. Describe what you want Ordo to do
and what problem it solves — the "why" matters more than the "how."

## Development setup

```bash
git clone https://github.com/Lucerna-Labs/ordo.git
cd ordo
cargo check -p ordo-cli   # verify the workspace compiles
cd ordo-studio && npm install && npm run build   # build the UI
cargo run -- serve   # launch
```

The workspace is Rust 2021 edition, pinned to Rust 1.93.0 via
`rust-toolchain.toml`.

## Code style

- Run `cargo fmt` before submitting
- Run `cargo clippy --workspace --all-targets` — zero warnings expected
- Run `cargo test --workspace --no-fail-fast` — all tests should pass

## Architecture overview

The codebase is ~100k lines across 62 Rust crates:

- **ordo-runtime**: boots and supervises all components
- **ordo-control**: local HTTP API + static UI serving (14 route modules)
- **ordo-assistant**: assistant sessions, turn loop, memory, tools
- **ordo-mcp-host**: 12 capability providers (filesystem, cloud, LLM, etc.)
- **ordo-studio**: React operator UI (Vite + TypeScript)

See `docs/architecture.md` for the full map.
