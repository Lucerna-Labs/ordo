# Incident 2026-06-07 — runtime exits `0xFFFFFFFF` (−1) after a runaway `cloud.credentials.list` loop

**Status:** Fixes landed (two P0). Two follow-ups (P1 launcher, P2 client-side) open.
**Component:** `ordo serve` runtime (`target\debug\ordo.exe serve`), Windows portable build.
**Severity:** Runtime termination during a diagnostic-mode session; no data loss.

---

## 1. Summary

While using Ordo, the runtime process terminated. A first "diagnostics mode" pass
(by a model that had **not** read the crash logs) blamed a *Tauri WebView hard-fail*
/ *.NET native crash* / *streaming OOM*. That is wrong on its face: the Ordo runtime
is single-process Rust on a Tokio bus with **no Tauri, no webview, no .NET**. (The
"Tauri" confusion most likely came from a *different* checkout on this machine,
`Ordo-github-clean-…\ordo-studio\src-tauri`, whose studio UI is Tauri-based — but that
is not this runtime.)

Reading the actual logs revealed **two independent problems**:

- **Q1 — a runaway tool loop.** In diagnostic mode the runtime served the tool
  `cloud.credentials.list` **62 times** (the only other call in the session was one
  `assistant.new_session`). These were **62 separate inbound `GET /api/cloud/credentials`
  HTTP requests** from an external client — the control-API → `Brain::invoke_tool` path
  had **no rate cap, no dedup, and no loop-break**.
- **Q2 — the process died with exit code `0xFFFFFFFF` (−1).** This is **not** a Rust
  panic, **not** a native fault, and **not** a clean shutdown — it is the signature of an
  **external OS-level termination** that supplied `−1`, most consistent with a
  **non-intercepted Windows console-control event** (close/logoff/shutdown) on the
  minimized `cargo run` console.

---

## 2. Evidence (ground truth)

From `runtime-portable.err.log` (entire contents):

```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.74s
 Running `target\debug\ordo.exe serve`
error: process didn't exit successfully: `target\debug\ordo.exe serve` (exit code: 0xffffffff)
```

`0xFFFFFFFF` == `4294967295` == `−1` as `i32`. **No panic message and no backtrace** on
stderr.

From `runtime-portable.out.log` (329 lines):

- Normal boot (17 modes, memory online, MCP host with 18 providers, secrets tier3,
  control API on `127.0.0.1:4141`, `"[runtime] ready. waiting for Ctrl+C..."`).
- One `assistant.new_session` with `"mode":"diagnostic"`.
- Then **62×** the cycle: `[Brain] Invoking tool 'cloud.credentials.list'` →
  `[Memory] Archiving: tool requested` → `[Brain] Tool … completed` →
  `[Memory] Archiving: tool completed -> {credential json}`.
- The log ends **on a complete success cycle** (line 329 is a normal `Archiving: tool
  completed`), then the process exited `−1`. The 62nd read+write **both succeeded**.

Filesystem at crash time: `data\ordo.db-wal` was **3.6 MB and uncheckpointed** (newer than
`ordo.db` / `ordo.db-shm`) → abrupt termination, no clean shutdown. **WAL mode keeps this
data safe** — it is replayed on the next open, so the kill lost no committed work.

Windows Event Log around the crash time: **no `ordo.exe` Application Error (1000), no WER
report, no crash dump, and no System-log shutdown/Kernel-Power events.** A native fault
always logs a 1000 event; its absence corroborates "external kill, not native fault."

---

## 3. Investigation method

- Multi-agent workflow: 6 parallel investigators across the relevant crates (exit path,
  Brain loop, diagnostic mode, serve driver, memory archive, cloud client) → synthesis →
  3 adversarial verifiers → report.
- The adversarial pass **measured exit codes on this exact toolchain** (rustc 1.93.0,
  Win11) with a throwaway crate: Rust panic (unwind) = `101` + a stderr panic line; native
  access violation = `0xC0000005`; native abort = `0xC0000409`; `std::process::exit(-1)` =
  exactly `0xFFFFFFFF`. (Those `exitcheck.exe` Application-Error entries at ~14:51–14:53 in
  the Event Log are this measurement, not the incident.)
- Cross-checked against the Windows Event Log / WER (above).

This **corrected an early hypothesis** (a native SQLite/keyring fault producing `−1`):
a native fault would surface as `0xC0000005`/`0xC0000409`, and the log ends on a *complete*
cycle, so nothing died mid-write.

---

## 4. Root cause

### Q1 — why the runaway was unbounded (PROVEN by source)

`Brain::invoke_tool` ([ordo-brain/src/lib.rs](../../ordo-brain/src/lib.rs)) is a single
publish-and-await-correlation bus round-trip with **no max-iteration cap, no duplicate
detection, and no loop-break** (only a 300 s per-call timeout). Its only production callers
are the control-API handlers `list_cloud_credentials` and `invoke_tool_by_name`
([ordo-control/src/lib.rs](../../ordo-control/src/lib.rs)); every other caller is
`#[cfg(test)]`. So 62 `[Brain] Invoking` lines = **62 stateless HTTP requests**, not one
in-process loop. The genuinely guarded turn loop (`DEFAULT_MAX_TOOL_ITERATIONS = 6`,
`MAX_DUPLICATE_TOOL_CALLS_PER_TURN = 1` in
[ordo-assistant/src/service.rs](../../ordo-assistant/src/service.rs)) is a **different,
unused** path that prints no `[Brain]` line. The `ordo-jobs` "Provider Availability Check"
(also `cloud.credentials.list`) was ruled out — test-only registration, 1800 s interval.

The external client is **inferred** to be the diagnostic-mode UI agent (its tool menu
collapses onto the one credential read; web/remote tools are forbidden; 0 MCP servers;
self-heal/llama unconfigured). Not byte-proven: the log has no timestamps or HTTP access
records.

### Q2 — what produced `−1` (PROVEN negative + strongest external cause)

Exhaustive audit found **no Rust path in the workspace** that can emit `0xFFFFFFFF`: zero
`process::exit(-1)`, zero `abort`, zero panic hooks, zero `panic = "abort"`, no
`SetConsoleCtrlHandler`. Debug build is `panic = unwind`. Combined with the measured
toolchain exit codes and the clean Event Log, the only explanation consistent with
`0xFFFFFFFF` + no fault record + uncheckpointed WAL is an **external OS-level termination
with code −1**.

The **leading concrete cause** is a non-intercepted Windows console control event. The
launcher runs the runtime as `Start-Process cargo run -- serve -WindowStyle Minimized`
([Launch-Ordo-Portable.ps1](../../Launch-Ordo-Portable.ps1)), and `run_serve` awaited only
`tokio::signal::ctrl_c()`, which on Windows catches **only** `CTRL_C` / `CTRL_BREAK` — not
`CTRL_CLOSE` / `CTRL_LOGOFF` / `CTRL_SHUTDOWN`. Closing the minimized console therefore
hard-killed the runtime with no graceful shutdown. Manual `taskkill` and an antivirus kill
are alternative external causes that look identical on disk; separating them definitively
needs a WER dump (see follow-ups).

---

## 5. Fixes (landed)

### Fix A (P0) — graceful shutdown on every signal class + deterministic WAL checkpoint

- [ordo-cli/src/main.rs](../../ordo-cli/src/main.rs) — `run_serve` now waits on
  `wait_for_shutdown_signal()`, which catches all five Windows console events
  (`ctrl_c`/`ctrl_break`/`ctrl_close`/`ctrl_logoff`/`ctrl_shutdown`) and, on Unix,
  `SIGINT`/`SIGTERM`. On any of them it logs the signal, calls `runtime.shutdown()`, then
  folds the WAL via `checkpoint_wal_with_retry`.
- [ordo-store/src/lib.rs](../../ordo-store/src/lib.rs) — new `pub fn checkpoint_wal(path)`
  + `WalCheckpoint`: opens a fresh short-lived connection and runs
  `PRAGMA wal_checkpoint(TRUNCATE)`, returning `(busy, log_frames, checkpointed_frames)`.
- `checkpoint_wal_with_retry` retries briefly because `shutdown()` only *aborts* the
  component tasks; the detached `StorageTask` OS threads that own the SQLite connections
  close a beat later, so an immediate checkpoint races them and returns `busy`. The retry
  lands an uncontended `TRUNCATE` once those threads exit. (Found in independent review.)
- [ordo-cli/Cargo.toml](../../ordo-cli/Cargo.toml) — added the `ordo-store` path dep.

**Windows caveat (documented in code):** for `CTRL_CLOSE`/`CTRL_LOGOFF`/`CTRL_SHUTDOWN`
the OS grants only ~5 s after notifying the handler before force-terminating, so the
graceful path is **best-effort** for those. `Ctrl+C` / `Ctrl+Break` are not force-killed
and always complete. The guaranteed fix for the console-close vector is Fix C (launcher).

### Fix B (P0) — runaway guard on `Brain::invoke_tool`

- [ordo-brain/src/lib.rs](../../ordo-brain/src/lib.rs) — `admit_tool_call` (called at the
  top of `invoke_tool`) consults a `Mutex<ToolCallGuard>`:
  - **Hard sliding-window rate cap** per capability (`TOOL_CALL_RATE_MAX = 120` /
    `TOOL_CALL_RATE_WINDOW = 10s`) → rejects fast runaways with
    `BrainError::ToolCallRateLimited` **before** any bus traffic. Generous on purpose:
    legitimate UI-driven bursts are unaffected.
  - **Warn-only consecutive-identical detection** (every `TOOL_CALL_WARN_STRIDE = 30`
    identical-in-a-row calls) → makes a slow runaway **visible in the logs in real time**
    instead of having to be reconstructed after a crash. Warn-only because blocking
    identical calls would break legitimate idle polling.
  - **Never caches results** — caching would be a correctness bug for non-idempotent tools
    (e.g. `assistant.new_session`). The lock is held only for the synchronous bookkeeping
    and never across an `.await` (consistent with Fix Pattern 9).

> Note: the *slow* fixation that drove this specific incident (~1 call / 6 s) is below the
> rate cap by design — that loop was harmless (read-only, all succeeded). The cap protects
> against a *fast* runaway; the warn surfaces the slow one. The real fix for a fixated
> client lives in the client (the UI agent / diagnostic-mode planner), which is outside this
> Rust workspace — see follow-ups.

---

## 6. Verification

- `cargo build -p ordo-cli --bin ordo` with `RUSTFLAGS=-D warnings` → **clean** (the
  launcher's exact gate; the shipping binary contains the new code).
- Unit tests, all passing:
  - `ordo-store::wal_checkpoint_tests::checkpoint_wal_runs_clean_and_preserves_data` —
    real WAL DB, `busy == 0`, WAL folded, 200 rows intact.
  - `ordo-brain::guard_tests` — rate cap trips at the 121st call, window pruning lets
    traffic resume, consecutive counting + reset, and rate-limited calls report
    `consecutive == 0` (Issue 2 regression test).
- **Independent review** (a second model that did not write the code) found two real
  issues — the checkpoint/shutdown race and the misleading consecutive warning — **both
  fixed** above and re-verified.
- **Not** exercised live in the sandbox: actually closing a console window and observing the
  graceful line. Reliable injection of a Windows console-close/break from a non-interactive
  shell risks killing the controlling shell, and port 4141 was held by another Ordo
  checkout. See the manual test below.

### Manual end-to-end test (30 s)

1. `./Launch-Ordo-Portable.ps1` and wait for `"[runtime] ready. waiting for shutdown
   signal (Ctrl+C / close window)..."`.
2. **Ctrl+C** in the runtime console → expect `"[runtime] received Ctrl+C; shutting down
   components..."`, then `"[runtime] WAL checkpoint complete (busy=0 …)"`, then
   `"[runtime] shutdown complete"`, and a clean exit (no `0xffffffff`).
3. Confirm `data\ordo.db-wal` shrank (folded into `ordo.db`).
4. Repeat, but **close the console window** instead of Ctrl+C — the same lines should
   appear if shutdown completes within the OS grace window.

---

## 7. Follow-ups

- **P1 — launcher hardening — DONE 2026-06-07** ([Launch-Ordo-Portable.ps1](../../Launch-Ordo-Portable.ps1)):
  the runtime is no longer launched as a transient minimized console. The launcher now (1)
  builds the runtime as a separate foreground step (`cargo build --bin ordo`), then (2)
  launches the built `target\debug\ordo.exe serve` in its **own hidden console**
  (`Start-Process -WindowStyle Hidden`), so there is no visible/closeable window tied to the
  runtime and closing the launcher's shell cannot orphan-kill it. It also enables a WER
  LocalDump for `ordo.exe` (`HKCU\…\Windows Error Reporting\LocalDumps\ordo.exe`,
  `DumpType=2`, `DumpFolder=<root>\crash-dumps`, `DumpCount=10`) so the next termination is
  provable. **Verified**: the built exe launched hidden/detached survives the spawning
  shell's exit and reaches `/health` (tested on an isolated port + temp DB); `cargo build
  --bin ordo` is clean; `-Check` parses; the WER keys read back correctly.
- **P2 — client-side fixation** (open): the diagnostic-mode UI agent fixating on
  `cloud.credentials.list` is the deeper Q1 cause and lives in the UI app
  (`bin\windows\Ordo.exe`), outside this workspace. Add a turn/no-op guard there (or widen
  the diagnostic-mode tool menu so the model can make progress).
- **P2 — store hardening** (open): collapse the multiple SQLite connections on one DB file
  ([ordo-store/src/lib.rs](../../ordo-store/src/lib.rs) flags this) to remove the latent
  WAL-writer contention that ballooned the WAL to 3.6 MB.
- **Optional**: for a *guaranteed* console-close checkpoint regardless of the OS 5 s timer,
  add a raw `SetConsoleCtrlHandler` that runs `checkpoint_wal` synchronously inside the
  handler. With P1 done (no closeable console + own hidden console), this is now low value.

### Reading a crash dump (when one appears)

With the WER LocalDump enabled, a genuine **native crash** of `ordo.exe` (e.g. access
violation `0xC0000005`, abort `0xC0000409`, stack overflow `0xC00000FD`) writes a full
`.dmp` into `<root>\crash-dumps\`. An **external kill** that supplies exit `-1` (console
close that outran the grace timer, `taskkill`, antivirus) writes **no** dump — so *the
presence or absence of a dump alone tells you which class of failure occurred*.

To read a dump (`ordo.exe.<pid>.dmp`):

1. Open it in WinDbg (`windbg -z <file>.dmp`) or Visual Studio (File ▸ Open ▸ the `.dmp`,
   then "Debug with Native Only").
2. In WinDbg: `!analyze -v` — the `EXCEPTION_RECORD` / `ExceptionCode` is the NTSTATUS
   (e.g. `c0000005`), and the faulting stack points at the crashing frame (look for
   `ordo`/`sqlite3`/`keyring` frames).
3. Cross-reference Event Viewer ▸ Windows Logs ▸ Application for the matching `Application
   Error` (1000) / `Windows Error Reporting` (1001) entry and timestamp.
4. If `crash-dumps\` stays empty after a termination, treat it as an external kill (revisit
   AV exclusions for `target\debug\ordo.exe`, and confirm nothing `taskkill`ed it).

---

## 8. Operator recovery (after any hard kill)

1. With the runtime stopped, fold the WAL: a clean start replays it automatically, or run
   `sqlite3 data\ordo.db "PRAGMA wal_checkpoint(TRUNCATE);"`. **Do not delete
   `ordo.db-wal`** — that discards committed-but-unfolded data.
2. Kill any orphan: `Get-Process ordo | Stop-Process -Force`.
3. Relaunch; confirm `"[runtime] ready…"` and that `ordo.db-wal` shrinks after startup.
