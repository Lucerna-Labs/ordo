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
