# Ordo Operator Simulator

The operator simulator is a pre-ship check that behaves more like a human
operator than a unit test. It drives the live local control API, opens a chat
session, sends a small assistant turn, and reads the surfaces users are likely
to touch before release.

Run it against a live Ordo runtime:

```powershell
cargo run -p ordo-operator-sim -- --origin http://127.0.0.1:4141
```

Reports are written to:

```text
target/operator-sim/operator-sim-report.json
target/operator-sim/operator-sim-report.md
```

Use `--strict` for a release gate. In strict mode, warnings become a failed
verdict. Without strict mode, missing optional provider setup is reported as a
warning so local-only builds can still be inspected.

Use `--voice` when cloud text-to-speech credentials are configured and you want
the simulator to test the speech endpoint.

The simulator checks:

- health and runtime profile
- storage budgets
- capability inventory
- assistant modes and sessions
- MCP, plugins, skills, automations, security, review, files, apps, connections
- assistant session creation and one visible assistant turn
- optional voice speech

## Rust Vibe Coder Preflight

Use the preflight harness when you want a release gate that also checks whether
the Rust Vibe Coder skill stack is surfaced and usable:

```powershell
.\scripts\ordo-preflight.ps1 -Origin http://127.0.0.1:4141
```

The preflight harness:

- checks the local control API
- verifies `rust_vibe_coder` is registered as a mode
- verifies the required Rust architecture skills are visible
- verifies pinned long-term memory anchors for primitive kit, bus-first,
  warning-denied verification, and the anti prompt-injection strainer
- asks Rust Vibe Coder for a no-write architecture response unless
  `-SkipCoderTurn` is supplied
- verifies the Rust Vibe Coder contract includes native rebuilds, zero-warning
  gates, launch-for-confirmation, and automated human-like usage testing
- generates a tiny Rust lite app in `target/ordo-preflight`
- builds, tests, and lints that app with warnings denied

Reports are written to:

```text
target/ordo-preflight/ordo-preflight-report.json
target/ordo-preflight/ordo-preflight-report.md
```

Use `-Strict` when warnings should fail the preflight verdict.

This does not replace UXI visual testing. It gives the UXI, CLI, and release
process one shared machine-readable report about the runtime surfaces a human
operator depends on.
