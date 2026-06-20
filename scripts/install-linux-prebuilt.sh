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
  # No -f on the API query: a missing release returns HTTP 404 with a JSON body,
  # and we want to read that body to tell "no release yet" apart from a real
  # network error, instead of dying with a misleading "API error".
  fetch()    { curl -sSL "$1"; }
  download() { curl -fL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch()    { wget -qO- "$1"; }
  download() { wget -O "$2" "$1"; }
else
  err "Need curl or wget to download the package."
fi

no_prebuilt() {
  err "No prebuilt Ordo .deb is available yet$1.
This usually just means a release has not been published to $REPO yet.
Build from source instead (compiles Servo + SpiderMonkey, ~30-60 min):
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install-linux-from-github.sh | bash"
}

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

# GitHub's unauthenticated API is edge-cached and rate-limited: /releases/latest
# sometimes returns 404/"Not Found" or an empty body for up to a minute even when
# a release exists. That false negative is exactly what sends people to the slow
# source build by mistake. So: retry, and if "latest" still won't resolve, fall
# back to the /releases list (newest-first, less aggressively cached).
usable_json() {
  [[ -n "$1" ]] && ! printf '%s' "$1" | grep -qE '"message" *: *"(Not Found|API rate limit)'
}

fetch_json_retry() {
  local url="$1" body="" attempt
  for attempt in 1 2 3 4; do
    body="$(fetch "$url" 2>/dev/null || true)"
    if usable_json "$body"; then
      printf '%s' "$body"
      return 0
    fi
    sleep 2
  done
  printf '%s' "$body"
  return 1
}

asset_urls() {
  printf '%s\n' "$1" \
    | grep -o '"browser_download_url": *"[^"]*"' \
    | sed 's/.*"browser_download_url": *"//; s/"$//'
}

# Prefer an arch-matched .deb, else any .deb. On a /releases list (newest-first)
# the first match is the newest release's package.
pick_deb() {
  local urls arch_deb
  urls="$(asset_urls "$1")"
  arch_deb="$(printf '%s\n' "$urls" | grep -E "ordo_.*_${arch}\.deb$" | head -1 || true)"
  if [[ -n "$arch_deb" ]]; then
    printf '%s' "$arch_deb"
    return 0
  fi
  printf '%s\n' "$urls" | grep -E '\.deb$' | head -1 || true
}

echo "Looking up Ordo release ($TAG) for $arch from $REPO..."
url=""
if [[ "$TAG" == "latest" ]]; then
  release_json="$(fetch_json_retry "https://api.github.com/repos/$REPO/releases/latest" || true)"
  url="$(pick_deb "$release_json")"
  if [[ -z "$url" ]]; then
    echo "  'latest' was unavailable from the API; checking the full release list..."
    list_json="$(fetch_json_retry "https://api.github.com/repos/$REPO/releases?per_page=30" || true)"
    if ! usable_json "$list_json"; then
      no_prebuilt " (could not reach GitHub — also check your internet connection)"
    fi
    url="$(pick_deb "$list_json")"
  fi
else
  release_json="$(fetch_json_retry "https://api.github.com/repos/$REPO/releases/tags/$TAG" || true)"
  if ! usable_json "$release_json"; then
    no_prebuilt " for '$TAG' (could not reach GitHub, or no such release tag)"
  fi
  url="$(pick_deb "$release_json")"
fi

[[ -n "$url" ]] || no_prebuilt " in the '$TAG' release"

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
