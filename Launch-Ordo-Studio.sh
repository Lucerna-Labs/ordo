#!/usr/bin/env bash
# Ordo Studio launcher (Linux/macOS) — the bash counterpart of
# Launch-Ordo-Studio.ps1. Starts the headless runtime from the current
# workspace, waits for it to be healthy, then runs the Tauri Studio dev shell.
#
#   ./Launch-Ordo-Studio.sh           # start the stack
#   ./Launch-Ordo-Studio.sh --check   # validate prerequisites, start nothing
#
# On Linux the desktop shell renders with webkit2gtk (not WebView2). The avatar
# pop-out (the Bot button) opens the avatar page in your default browser via
# xdg-open, where the microphone works normally; the in-tab avatar mic needs a
# webkit permission handler that is not wired yet — use the pop-out for voice.
set -euo pipefail

ORDO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STUDIO_DIR="$ORDO_ROOT/ordo-studio"
RUNTIME_USER_FILES="$ORDO_ROOT/ordo-runtime/user-files"
MODES_PATH="$RUNTIME_USER_FILES/modes"
CONTROL_URL="http://127.0.0.1:4141"
RUNTIME_OUT="$ORDO_ROOT/runtime-dev.out.log"
RUNTIME_ERR="$ORDO_ROOT/runtime-dev.err.log"

echo
echo "Ordo Studio launcher"
echo "Workspace: $ORDO_ROOT"
echo "Runtime user files: $RUNTIME_USER_FILES"
echo

[ -f "$STUDIO_DIR/package.json" ] || { echo "ordo-studio/package.json not found at $STUDIO_DIR" >&2; exit 1; }

for tool in npm cargo curl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool was not found on PATH" >&2; exit 1; }
done

mkdir -p "$MODES_PATH"

if [ "${1:-}" = "--check" ]; then
  echo "Launcher check passed. No processes were started."
  exit 0
fi

# Pass through optional search-provider keys if exported in the environment.
export ORDO_USER_FILES_PATH="$RUNTIME_USER_FILES"
export ORDO_MODES_PATH="$MODES_PATH"
export ORDO_RUNTIME_PROFILE="standard"
export ORDO_CONTROL_URL="$CONTROL_URL"
# Run the avatar performance driver (~30Hz) so the avatar pop-out lip-syncs out
# of the box. Set to "0" to disable. See docs/avatar.md.
export ORDO_ENABLE_AVATAR="1"
export RUSTFLAGS="-D warnings"

if [ ! -d "$STUDIO_DIR/node_modules" ]; then
  echo "Installing Ordo Studio frontend dependencies..."
  ( cd "$STUDIO_DIR" && npm ci )
fi

echo "Clearing stale Ordo dev listeners on ports 4141 and 1420..."
for port in 4141 1420; do
  if command -v lsof >/dev/null 2>&1; then
    lsof -ti :"$port" -sTCP:LISTEN 2>/dev/null | xargs -r kill -9 2>/dev/null || true
  elif command -v fuser >/dev/null 2>&1; then
    fuser -k "$port"/tcp 2>/dev/null || true
  fi
done
sleep 0.8

rm -f "$RUNTIME_OUT" "$RUNTIME_ERR"

echo "Starting Ordo runtime from current workspace..."
( cd "$ORDO_ROOT" && cargo run -- serve >"$RUNTIME_OUT" 2>"$RUNTIME_ERR" ) &
RUNTIME_PID=$!
echo "Runtime PID: $RUNTIME_PID"

echo "Waiting for runtime health at $CONTROL_URL/health..."
healthy=0
for _ in $(seq 1 60); do
  if curl -fsS "$CONTROL_URL/health" -o /dev/null 2>/dev/null; then healthy=1; break; fi
  sleep 2
done

if [ "$healthy" -ne 1 ]; then
  echo "ERROR: Ordo runtime did not become healthy." >&2
  echo "--- runtime-dev.err.log (tail) ---" >&2
  tail -n 120 "$RUNTIME_ERR" 2>/dev/null >&2 || true
  kill "$RUNTIME_PID" 2>/dev/null || true
  exit 1
fi

echo "Runtime is healthy. Starting Ordo Studio..."
trap 'kill "$RUNTIME_PID" 2>/dev/null || true' EXIT
( cd "$STUDIO_DIR" && npm run tauri:dev )
