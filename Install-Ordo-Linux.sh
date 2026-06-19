#!/usr/bin/env bash
set -Eeuo pipefail

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

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

load_cargo_env() {
  local cargo_root="${CARGO_HOME:-$HOME/.cargo}"
  if ! command -v cargo >/dev/null 2>&1 && [[ -f "$cargo_root/env" ]]; then
    # shellcheck disable=SC1091
    source "$cargo_root/env"
  elif ! command -v cargo >/dev/null 2>&1 && [[ -d "$cargo_root/bin" ]]; then
    export PATH="$cargo_root/bin:$PATH"
  fi
}

if [[ "${ORDO_SKIP_LINUX_DEPS:-0}" != "1" && -x "$ROOT/scripts/install-linux-build-deps.sh" ]]; then
  "$ROOT/scripts/install-linux-build-deps.sh"
  load_cargo_env
fi

load_cargo_env

if command -v dpkg >/dev/null 2>&1; then
  if (( CHECK )); then
    "$ROOT/Install-Ordo-Linux-Deb.sh" --check
  else
    "$ROOT/Install-Ordo-Linux-Deb.sh"
  fi
  exit $?
fi

if (( CHECK )); then
  "$ROOT/Build-Ordo-Linux-AppImage.sh" --check
  echo "Generic Linux installer check passed."
  exit 0
fi

echo "This distro does not appear to use Debian packages."
echo "Building an AppImage instead..."
"$ROOT/Build-Ordo-Linux-AppImage.sh"
echo
echo "Launch Ordo with:"
echo "  ./dist/Ordo-${ORDO_PACKAGE_VERSION:-0.1.0}-$(uname -m).AppImage"
