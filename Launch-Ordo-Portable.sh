#!/usr/bin/env bash
# Ordo portable launcher (Linux/macOS) — the bash counterpart of
# Launch-Ordo-Portable.ps1. Builds the headless runtime from the current
# workspace (with the WASM + native code-exec runners), runs it detached, and
# waits for it to be healthy. Unlike the Windows script there is no prebuilt
# desktop app binary to launch here — open the Studio with Launch-Ordo-Studio.sh,
# or point a browser at http://127.0.0.1:4141/avatar.html for the avatar.
#
#   ./Launch-Ordo-Portable.sh           # build + run the runtime
#   ./Launch-Ordo-Portable.sh --check   # validate prerequisites, start nothing
set -euo pipefail

ORDO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTROL_URL="http://127.0.0.1:4141"
RUNTIME_USER_FILES="$ORDO_ROOT/ordo-runtime/user-files"
MODES_PATH="$RUNTIME_USER_FILES/modes"
RUNTIME_OUT="$ORDO_ROOT/runtime-portable.out.log"
RUNTIME_ERR="$ORDO_ROOT/runtime-portable.err.log"
RUNTIME_BIN="$ORDO_ROOT/target/debug/ordo"

echo
echo "Ordo portable launcher"
echo "Workspace: $ORDO_ROOT"
echo

[ -f "$ORDO_ROOT/Cargo.toml" ] || { echo "Cargo.toml not found at $ORDO_ROOT" >&2; exit 1; }
for tool in cargo curl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool was not found on PATH" >&2; exit 1; }
done

if [ "${1:-}" = "--check" ]; then
  echo "Portable launcher check passed. No processes were started."
  exit 0
fi

mkdir -p "$RUNTIME_USER_FILES" "$MODES_PATH"

export ORDO_USER_FILES_PATH="$RUNTIME_USER_FILES"
export ORDO_MODES_PATH="$MODES_PATH"
export ORDO_RUNTIME_PROFILE="standard"
export ORDO_CONTROL_URL="$CONTROL_URL"
export ORDO_ENABLE_AVATAR="1"
# NOTE: we deliberately do NOT set RUSTFLAGS="-D warnings" here. This launcher
# builds with the user's own (unpinned) rustc, so a newer compiler's new lint
# would turn a warning into a hard build failure at launch. Warning strictness
# is enforced in CI under the pinned toolchain, not when an end user runs this.
# Code execution (code.* / workspace.*). The native runner is compiled in via
# the `native-exec` feature below and ARMED here; set to "false" to keep only
# the WASM runner + workspace read/write tools.
export ORDO_CODE_ALLOW_NATIVE="true"
export ORDO_CODE_WORKSPACE_PATH="$RUNTIME_USER_FILES/workspace"
# Real local embeddings via Ollama (replaces the weak hashing fallback). Clear
# ORDO_EMBEDDING_OLLAMA_MODEL to revert.
export ORDO_EMBEDDING_OLLAMA_MODEL="nomic-embed-text"
export ORDO_EMBEDDING_DIMENSIONS="768"

# Kill any stale Ordo runtime listening on 4141 (only ours, by binary path).
if command -v lsof >/dev/null 2>&1; then
  for pid in $(lsof -ti :4141 -sTCP:LISTEN 2>/dev/null); do
    target="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
    case "$target" in "$ORDO_ROOT"/*) kill -9 "$pid" 2>/dev/null || true; echo "Stopped stale Ordo runtime PID $pid";; esac
  done
fi

rm -f "$RUNTIME_OUT" "$RUNTIME_ERR"

echo "Building Ordo runtime (the first build can take a while)..."
( cd "$ORDO_ROOT" && cargo build --bin ordo --features sandbox-wasm,native-exec )
[ -x "$RUNTIME_BIN" ] || { echo "Runtime binary not found at $RUNTIME_BIN after build." >&2; exit 1; }

echo "Starting Ordo runtime (detached)..."
( cd "$ORDO_ROOT" && nohup "$RUNTIME_BIN" serve >"$RUNTIME_OUT" 2>"$RUNTIME_ERR" & echo $! >"$ORDO_ROOT/.ordo-runtime.pid" )
RUNTIME_PID="$(cat "$ORDO_ROOT/.ordo-runtime.pid" 2>/dev/null || true)"
echo "Runtime PID: ${RUNTIME_PID:-unknown}"

echo "Waiting for runtime health at $CONTROL_URL/health..."
healthy=0
for _ in $(seq 1 60); do
  if curl -fsS "$CONTROL_URL/health" -o /dev/null 2>/dev/null; then healthy=1; break; fi
  sleep 2
done

if [ "$healthy" -ne 1 ]; then
  echo "ERROR: Ordo runtime did not become healthy." >&2
  tail -n 120 "$RUNTIME_ERR" 2>/dev/null >&2 || true
  exit 1
fi

echo "Runtime is healthy at $CONTROL_URL."
echo "Open the desktop Studio with ./Launch-Ordo-Studio.sh, or the avatar at"
echo "  $CONTROL_URL/avatar.html"
