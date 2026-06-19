# Code execution (sandboxed workspace)

Every mode — and the assistant itself — can **write and run code** in a confined
workspace. This is exposed as five capabilities, reachable by the Brain
(`Brain::invoke_tool` / `POST /api/tools/<capability>`) and by the assistant's
`ToolGateway`.

## Capabilities

| Capability | Tier | What it does |
|---|---|---|
| `workspace.write_file` | Core | Write a UTF-8 file into the workspace (creates parent dirs). |
| `workspace.read_file` | Core | Read a file from the workspace. |
| `workspace.list` | Optional | List the workspace (or a subdir). |
| `code.run` | Optional | Run a **compiled WASM module** in the in-process sandbox (fuel/memory/wall-clock limited; **no fs/network**). Pure compute only. |
| `code.run_native` | Heavy | Run a **native command** (cargo/rustc/python/node/pwsh/cmd) in the workspace. **Network allowed.** Opt-in + gated. |

All paths are confined to a single workspace directory
(`ORDO_CODE_WORKSPACE_PATH`, default `<user_files>/workspace`).

### `code.run_native` arguments

Provide either a `language` (with `source` for a quick snippet, or `args`), or an
explicit `program` + `args`:

```jsonc
// inline snippet
{ "language": "python", "source": "print(6*7)" }
// run a file you wrote first
{ "language": "python", "program": "python", "args": ["script.py"] }
// shell one-liner
{ "language": "shell", "source": "echo hi" }
// a Rust project written via workspace.write_file
{ "language": "rust", "program": "cargo", "args": ["run"], "cwd": "myproj" }
```

Optional: `cwd` (subdir relative to the workspace), `stdin`, `timeout_ms`.
Returns `{ backend, exit_code, success, stdout, stderr, duration_ms }`. A non-zero
exit is a **successful run that returned an error** (you get `exit_code` + `stderr`),
not a tool failure.

Typical flow: `workspace.write_file` to author files → `code.run_native` to run them.

## Architecture

Mirrors the `ordo-files` seam — no new process boundary except the deliberate
code-runner subprocess.

- **`ordo-sandbox`** owns the `Sandbox` trait. `WasmtimeSandbox` (feature
  `wasmtime`) backs `code.run`. **`SubprocessSandbox`** (feature `subprocess`,
  `subprocess_impl.rs`) is the native runner — it runs the *exact* command it is
  handed, confined to a root, with a timeout, output caps, and process-tree kill.
- **`ordo-code`** (new crate) — headless `CodeService` (owns the workspace root +
  two `Arc<dyn Sandbox>` backends + `CodePolicy`) and `CodeProvider`. The
  language→program/args mapping and source-file materialization live here.
- **`ordo-mcp-host`** — `CodeCapabilityAdapter` bridges `CodeProvider` into the
  `CapabilityProvider` surface (owns the `code.` *and* `workspace.` prefixes).
- **`ordo-runtime`** — constructs the backends (feature-gated; `NullSandbox` when
  off) and registers the adapter via `security.gate(.., "code")`.
- **`ordo-assistant`** — `code.` / `workspace.` added to `DEFAULT_ALLOWED_LANES`.
- **modes** — every mode's `allowed_tool_lanes` includes `code.` + `workspace.`.

No `ordo-protocol` changes: the native run spec tunnels through the existing
`SandboxRequest.input: Value`, and results come back in `SandboxExecution.output`.

## Security model

`workspace.*` and `code.run` are low-risk. **`code.run_native` runs arbitrary
native code with the host's network reach**, so it is gated on FOUR independent
layers (all off in the crate defaults):

1. **Compile gate** — the `native-exec` cargo feature must be enabled, else
   `SubprocessSandbox` isn't compiled and the native backend is `NullSandbox`.
2. **Runtime gate** — `ORDO_CODE_ALLOW_NATIVE=true` must be set; otherwise
   `code.run_native` returns an actionable "disabled" error (`CodePolicy.allow_native
   = cfg!(feature="native-exec") && config.code_allow_native`).
3. **Program allowlist** — only `cargo, rustc, python, node, pwsh, powershell, cmd,
   sh` may be spawned; anything else is rejected before spawn.
4. **Workspace confinement** — every `workspace.*` path and the native `cwd` are
   resolved under the workspace root and rejected on escape (absolute, `..`, and
   Windows drive-relative `C:foo` / prefixed paths). The child runs with
   `current_dir` = workspace, `kill_on_drop`, and `CREATE_NO_WINDOW` on Windows.

Plus the standard `security.gate(.., "code")` (classifiers + audit), and every tool
call is recorded in the memory archive.

**This is workspace-confinement + resource limits + an allowlist + audit — NOT a
hard security boundary against malicious code.** The truly isolated runner is
`code.run` (WASM: no fs/network). Treat `code.run_native` as "run trusted-ish code
conveniently," which is why it is quadruple-gated and network-allowed by your
explicit choice.

### Per-mode policy

`allowed_tool_lanes` is prefix-matched; `blocked_tool_capabilities` is exact-matched
and wins. Two read-only modes deliberately keep their stance — `diagnostic` and
`security_research` list `code.run_native` and `workspace.write_file` in
`blocked_tool_capabilities`, so they get `code.run` + `workspace.read_file/list` but
**not** native execution or file writes. To restrict any other mode, add those exact
capability names to its `blocked_tool_capabilities`. (The on-disk JSONs under
`ordo-runtime/user-files/modes/` are authoritative; `ordo-modes/src/defaults.rs`
seeds fresh installs.)

## Configuration

| Env var | Default | Meaning |
|---|---|---|
| `ORDO_CODE_WORKSPACE_PATH` | `<user_files>/workspace` | Confined workspace dir (created at boot). |
| `ORDO_CODE_ALLOW_NATIVE` | `false` | Arms `code.run_native` (needs `native-exec` too). |
| `ORDO_CODE_LANGUAGES` | empty = all | Comma-separated allowlist of native languages. |
| `ORDO_CODE_TIMEOUT_MS` | `30000` | Default wall-clock cap per run. |

| Cargo feature (on `ordo-runtime` / `ordo-cli`) | Effect |
|---|---|
| `sandbox-wasm` | Compiles in `WasmtimeSandbox` for `code.run`. |
| `native-exec` | Compiles in `SubprocessSandbox` for `code.run_native`. |

The Servo launcher is the supported beta launch path. Native execution remains
controlled by the `native-exec` feature and `ORDO_CODE_ALLOW_NATIVE`. To disable
native execution while keeping the WASM runner and workspace read/write, set
`ORDO_CODE_ALLOW_NATIVE=false` or build without `native-exec`.

## Known limitations / follow-ups

- `code.run` requires a **pre-compiled** WASM module (`wasm_base64`) — it can't
  compile Rust/etc. to WASM for you. Dependency-fetching languages run only on the
  native lane.
- Process-tree kill on timeout uses `taskkill /T /F` (Windows). The non-Windows
  fallback is best-effort (process group + pid).
- A future hardening could move the native runner into a real OS sandbox (Job
  Object resource limits, or a container) for a true security boundary.
