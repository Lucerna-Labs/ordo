#!/usr/bin/env bash
set -Eeuo pipefail

VERSION="${ORDO_PACKAGE_VERSION:-0.1.0}"
CHECK=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      CHECK=1
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf 'x86_64\n' ;;
    aarch64|arm64) printf 'aarch64\n' ;;
    *) uname -m ;;
  esac
}

ARCH="${ORDO_PACKAGE_ARCH:-$(detect_arch)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$ROOT/dist"
STAGE="$DIST/ordo-linux-${VERSION}-${ARCH}"
ARCHIVE="$DIST/ordo-linux-${VERSION}-${ARCH}.tar.gz"

load_cargo_env() {
  local cargo_root="${CARGO_HOME:-$HOME/.cargo}"
  if ! command -v cargo >/dev/null 2>&1 && [[ -f "$cargo_root/env" ]]; then
    # shellcheck disable=SC1091
    source "$cargo_root/env"
  fi
  if ! command -v cargo >/dev/null 2>&1 && [[ -d "$cargo_root/bin" ]]; then
    export PATH="$cargo_root/bin:$PATH"
  fi
}

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

load_cargo_env
need_command cargo
need_command npm
need_command tar

for required_file in \
  "$ROOT/packaging/linux/ordo-launcher" \
  "$ROOT/packaging/linux/launch-ordo-portable" \
  "$ROOT/packaging/linux/install-portable-desktop-shortcut" \
  "$ROOT/packaging/linux/ordo.svg"; do
  if [[ ! -f "$required_file" ]]; then
    echo "Missing required packaging file: $required_file" >&2
    exit 1
  fi
done

if (( CHECK )); then
  echo "Portable Linux package check passed."
  echo "  root: $ROOT"
  echo "  stage: $STAGE"
  echo "  archive: $ARCHIVE"
  exit 0
fi

rm -rf "$STAGE" "$ARCHIVE"
mkdir -p "$STAGE/bin"
mkdir -p "$STAGE/ordo-studio"
mkdir -p "$STAGE/docs"

echo "Building Ordo Studio..."
npm --prefix "$ROOT/ordo-studio" ci
npm --prefix "$ROOT/ordo-studio" run build

echo "Building Ordo runtime..."
cargo build --release -p ordo-cli

echo "Building Ordo Servo shell..."
# shellcheck source=scripts/servo-build-env.sh
source "$ROOT/scripts/servo-build-env.sh"
cargo build --manifest-path "$ROOT/ordo-servo-shell/Cargo.toml" --features servo-engine --release

install -m 0755 "$ROOT/target/release/ordo" "$STAGE/bin/ordo"
install -m 0755 "$ROOT/ordo-servo-shell/target/release/ordo-servo-shell" "$STAGE/bin/ordo-servo-shell"
install -m 0755 "$ROOT/packaging/linux/ordo-launcher" "$STAGE/bin/ordo-launcher"
install -m 0755 "$ROOT/packaging/linux/launch-ordo-portable" "$STAGE/launch-ordo.sh"
install -m 0755 "$ROOT/packaging/linux/install-portable-desktop-shortcut" "$STAGE/install-desktop-shortcut.sh"
install -m 0644 "$ROOT/packaging/linux/ordo.svg" "$STAGE/ordo.svg"

cp -a "$ROOT/ordo-studio/dist" "$STAGE/ordo-studio/dist"
for doc in README.md README-BETA.md; do
  if [[ -f "$ROOT/$doc" ]]; then
    install -m 0644 "$ROOT/$doc" "$STAGE/$doc"
  fi
done
for doc in official-memory.md build-history.md fixbook.md; do
  if [[ -f "$ROOT/docs/$doc" ]]; then
    install -m 0644 "$ROOT/docs/$doc" "$STAGE/docs/$doc"
  fi
done

tar -C "$DIST" -czf "$ARCHIVE" "$(basename "$STAGE")"

echo "Built portable Linux package:"
echo "  $ARCHIVE"
echo
echo "Test it with:"
echo "  tar -xzf $ARCHIVE -C $DIST"
echo "  $STAGE/launch-ordo.sh"
