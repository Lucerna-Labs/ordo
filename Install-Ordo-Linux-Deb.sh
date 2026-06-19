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

detect_deb_arch() {
  if command -v dpkg >/dev/null 2>&1; then
    dpkg --print-architecture
    return
  fi

  case "$(uname -m)" in
    x86_64|amd64) printf 'amd64\n' ;;
    aarch64|arm64) printf 'arm64\n' ;;
    *) uname -m ;;
  esac
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARCH="${ORDO_PACKAGE_ARCH:-$(detect_deb_arch)}"
PACKAGE="$ROOT/dist/ordo_${VERSION}_${ARCH}.deb"

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

need_command dpkg

as_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
    return
  fi

  if ! command -v sudo >/dev/null 2>&1; then
    echo "Missing sudo. Run this installer as root, or install sudo." >&2
    exit 1
  fi

  sudo "$@"
}

if [[ ! -x "$ROOT/Build-Ordo-Linux-Deb.sh" ]]; then
  echo "Missing or non-executable builder: $ROOT/Build-Ordo-Linux-Deb.sh" >&2
  exit 1
fi

if (( CHECK )); then
  "$ROOT/Build-Ordo-Linux-Deb.sh" --check 2>/dev/null || true
  echo "Debian installer check passed."
  echo "  package: $PACKAGE"
  exit 0
fi

"$ROOT/Build-Ordo-Linux-Deb.sh"

if [[ ! -f "$PACKAGE" ]]; then
  echo "Expected package was not created: $PACKAGE" >&2
  exit 1
fi

echo "Installing Ordo package with dpkg:"
echo "  $PACKAGE"

if as_root dpkg -i "$PACKAGE"; then
  echo "Installed Ordo. Open Ordo from the app menu."
  exit 0
fi

if ! command -v apt-get >/dev/null 2>&1; then
  echo "dpkg reported missing dependencies, but apt-get is not available." >&2
  echo "Install the missing dependencies shown above, then rerun this script." >&2
  exit 1
fi

echo "Repairing package dependencies with apt-get..."
as_root apt-get update
as_root apt-get install -f -y

echo "Retrying Ordo package install..."
as_root dpkg -i "$PACKAGE"
echo "Installed Ordo. Open Ordo from the app menu."
