#!/usr/bin/env bash
# Ordo prebuilt installer for Pop!_OS / Ubuntu / Debian.
#
# This is the user-friendly path: it downloads a prebuilt Ordo .deb from GitHub
# Releases and installs it. No compiler, no Rust/Node, and no 30-60 minute Servo
# + SpiderMonkey build on your machine.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Lucerna-Labs/ordo/main/scripts/install-linux-prebuilt.sh | bash
#
# Pin a specific release:
#   ORDO_VERSION=v0.1.0 bash install-linux-prebuilt.sh
#
# The script calls sudo only for the final package install step.
set -Eeuo pipefail

REPO="${ORDO_REPO:-Lucerna-Labs/ordo}"
TAG="${ORDO_VERSION:-latest}"

err() { echo "Error: $*" >&2; exit 1; }

if ! command -v dpkg >/dev/null 2>&1; then
  err "This prebuilt installer is for Debian-family distros (Pop!_OS, Ubuntu,
Debian). On other distros, build from source instead:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install-linux-from-github.sh | bash"
fi

if command -v curl >/dev/null 2>&1; then
  fetch()    { curl -fsSL "$1"; }
  download() { curl -fL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch()    { wget -qO- "$1"; }
  download() { wget -O "$2" "$1"; }
else
  err "Need curl or wget to download the package."
fi

as_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    err "Need root (or sudo) to install the package."
  fi
}

arch="$(dpkg --print-architecture)"   # amd64 / arm64

if [[ "$TAG" == "latest" ]]; then
  api="https://api.github.com/repos/$REPO/releases/latest"
else
  api="https://api.github.com/repos/$REPO/releases/tags/$TAG"
fi

echo "Looking up Ordo release ($TAG) for $arch from $REPO..."
release_json="$(fetch "$api")" || err "Could not query the GitHub releases API."

asset_urls() {
  printf '%s\n' "$release_json" \
    | grep -o '"browser_download_url": *"[^"]*"' \
    | sed 's/.*"browser_download_url": *"//; s/"$//'
}

# Prefer an arch-matched .deb, fall back to any .deb in the release.
url="$(asset_urls | grep -E "ordo_.*_${arch}\.deb$" | head -1 || true)"
[[ -n "$url" ]] || url="$(asset_urls | grep -E '\.deb$' | head -1 || true)"

if [[ -z "$url" ]]; then
  err "No prebuilt .deb was found in the '$TAG' release.
A prebuilt package may not be published yet. You can build from source instead:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install-linux-from-github.sh | bash"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
deb="$tmp/$(basename "$url")"

echo "Downloading: $url"
download "$url" "$deb" || err "Download failed."

echo "Installing $(basename "$deb") (you may be prompted for your password)..."
# dpkg -i then 'apt-get install -f' resolves runtime dependencies and avoids the
# 'unsupported file' error some apt versions throw on a local '.deb' path.
if ! as_root dpkg -i "$deb"; then
  echo "Resolving runtime dependencies..."
  as_root env DEBIAN_FRONTEND=noninteractive apt-get install -f -y
fi

echo
echo "Ordo installed. Launch it from your application menu (search for \"Ordo\"),"
echo "or run: /opt/ordo/bin/ordo-launcher"
