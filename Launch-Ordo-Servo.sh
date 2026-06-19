#!/usr/bin/env bash
set -Eeuo pipefail

CHECK=0
SKIP_BUILD=0
WIDTH=1560
HEIGHT=980

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      CHECK=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --width)
      WIDTH="${2:?--width requires a value}"
      shift 2
      ;;
    --height)
      HEIGHT="${2:?--height requires a value}"
      shift 2
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

ORDO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STUDIO_DIR="$ORDO_ROOT/ordo-studio"
RUNTIME_USER_FILES="$ORDO_ROOT/ordo-runtime/user-files"
MODES_PATH="$RUNTIME_USER_FILES/modes"
CONTROL_URL="http://127.0.0.1:4141"
STUDIO_URL="$CONTROL_URL/"
RUNTIME_OUT="$ORDO_ROOT/runtime-servo.out.log"
RUNTIME_ERR="$ORDO_ROOT/runtime-servo.err.log"
SERVO_OUT="$ORDO_ROOT/servo-shell.out.log"
SERVO_ERR="$ORDO_ROOT/servo-shell.err.log"
SERVO_SHELL_DIR="$ORDO_ROOT/ordo-servo-shell"
SERVO_SHELL_BIN="$SERVO_SHELL_DIR/target/debug/ordo-servo-shell"

runtime_pid=""

cleanup() {
  local status=$?
  if [[ -n "${runtime_pid:-}" ]] && kill -0 "$runtime_pid" 2>/dev/null; then
    echo "Servo shell closed; stopping Ordo runtime PID $runtime_pid..."
    kill "$runtime_pid" 2>/dev/null || true
    wait "$runtime_pid" 2>/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

wait_for_health() {
  local deadline=$((SECONDS + 120))
  until curl -fsS "$CONTROL_URL/health" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      echo "Runtime health check failed." >&2
      if [[ -f "$RUNTIME_ERR" ]]; then
        echo "--- runtime-servo.err.log ---" >&2
        tail -120 "$RUNTIME_ERR" >&2 || true
      fi
      exit 1
    fi
    sleep 2
  done
}

echo
echo "Ordo Servo Linux launcher"
echo "Workspace: $ORDO_ROOT"
echo "Runtime user files: $RUNTIME_USER_FILES"
echo "Modes: $MODES_PATH"
echo

if (( CHECK )); then
  echo "Launcher check passed. No processes were started."
  exit 0
fi

need_command cargo
need_command curl

mkdir -p "$MODES_PATH"

export ORDO_USER_FILES_PATH="$RUNTIME_USER_FILES"
export ORDO_MODES_PATH="$MODES_PATH"
export ORDO_RUNTIME_PROFILE="standard"
export ORDO_CONTROL_URL="$CONTROL_URL"
export ORDO_ENABLE_AVATAR="1"

STUDIO_INDEX="$STUDIO_DIR/dist/index.html"
NEEDS_STUDIO_BUILD=0
if (( ! SKIP_BUILD )) && [[ ! -f "$STUDIO_INDEX" ]]; then
  NEEDS_STUDIO_BUILD=1
fi

if (( NEEDS_STUDIO_BUILD )); then
  need_command npm
fi

if (( ! SKIP_BUILD )); then
  if [[ -d "$STUDIO_DIR/node_modules" || "$NEEDS_STUDIO_BUILD" -eq 1 ]]; then
    pushd "$STUDIO_DIR" >/dev/null
    if [[ ! -d node_modules ]]; then
      echo "Installing Ordo Studio frontend dependencies..."
      npm ci
    fi
    echo "Building Ordo Studio bundle for Servo..."
    npm run build
    popd >/dev/null
  else
    echo "Using existing Ordo Studio bundle at $STUDIO_INDEX."
  fi
fi

if [[ ! -f "$STUDIO_INDEX" ]]; then
  echo "Studio bundle is missing: $STUDIO_INDEX" >&2
  echo "Run without --skip-build, or build ordo-studio first." >&2
  exit 1
fi

if [[ ! -x "$SERVO_SHELL_BIN" ]]; then
  echo "Building embedded Ordo Servo shell for Linux..."
  cargo build --manifest-path "$SERVO_SHELL_DIR/Cargo.toml" --features servo-engine
fi

if [[ ! -x "$SERVO_SHELL_BIN" ]]; then
  echo "Embedded Servo shell did not build to $SERVO_SHELL_BIN" >&2
  exit 1
fi

if curl -fsS "$CONTROL_URL/health" >/dev/null 2>&1; then
  echo "Ordo already appears to be running at $CONTROL_URL." >&2
  echo "Close the existing Ordo runtime first, then launch again." >&2
  exit 1
fi

rm -f "$RUNTIME_OUT" "$RUNTIME_ERR" "$SERVO_OUT" "$SERVO_ERR"

echo "Starting Ordo runtime from current workspace..."
(
  cd "$ORDO_ROOT"
  cargo run -- serve
) >"$RUNTIME_OUT" 2>"$RUNTIME_ERR" &
runtime_pid=$!

echo "Runtime PID: $runtime_pid"
echo "Waiting for runtime health at $CONTROL_URL/health..."
wait_for_health
echo "Runtime is healthy."

echo "Checking Ordo-served Studio bundle at $STUDIO_URL..."
if ! curl -fsS "$STUDIO_URL" | grep -q "<title>Ordo</title>"; then
  echo "Ordo runtime did not serve the Studio bundle at $STUDIO_URL" >&2
  exit 1
fi

echo "Opening Ordo Studio through embedded Servo shell..."
(
  cd "$ORDO_ROOT"
  HTTP_PROXY="http://127.0.0.1:9" \
  HTTPS_PROXY="http://127.0.0.1:9" \
  ALL_PROXY="socks5://127.0.0.1:9" \
  http_proxy="http://127.0.0.1:9" \
  https_proxy="http://127.0.0.1:9" \
  all_proxy="socks5://127.0.0.1:9" \
  NO_PROXY="127.0.0.1,localhost,::1,[::1]" \
  no_proxy="127.0.0.1,localhost,::1,[::1]" \
  "$SERVO_SHELL_BIN" \
    --target "$STUDIO_URL" \
    --width "$WIDTH" \
    --height "$HEIGHT"
) >"$SERVO_OUT" 2>"$SERVO_ERR"
