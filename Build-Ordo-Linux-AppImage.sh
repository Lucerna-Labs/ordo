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
PORTABLE_DIR="$DIST/ordo-linux-${VERSION}-${ARCH}"
APPDIR="$DIST/Ordo.AppDir"
OUTPUT="$DIST/Ordo-${VERSION}-${ARCH}.AppImage"
TOOLS_DIR="$DIST/appimage-tools"

appimage_arch() {
  case "$ARCH" in
    x86_64) printf 'x86_64\n' ;;
    aarch64) printf 'aarch64\n' ;;
    *)
      echo "Unsupported AppImage architecture: $ARCH" >&2
      exit 1
      ;;
  esac
}

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

download_tool() {
  local name="$1"
  local url="$2"
  local dest="$3"

  mkdir -p "$(dirname "$dest")"
  if [[ -x "$dest" ]]; then
    printf '%s\n' "$dest"
    return
  fi

  echo "Downloading $name..."
  if command -v curl >/dev/null 2>&1; then
    curl -L --fail -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$dest" "$url"
  else
    echo "Missing curl or wget; install one or set ORDO_LINUXDEPLOY/ORDO_APPIMAGETOOL." >&2
    exit 1
  fi
  chmod +x "$dest"
  printf '%s\n' "$dest"
}

find_or_download_linuxdeploy() {
  if [[ -n "${ORDO_LINUXDEPLOY:-}" ]]; then
    printf '%s\n' "$ORDO_LINUXDEPLOY"
    return
  fi
  if command -v linuxdeploy >/dev/null 2>&1; then
    command -v linuxdeploy
    return
  fi

  local arch tool_url tool_path
  arch="$(appimage_arch)"
  tool_url="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${arch}.AppImage"
  tool_path="$TOOLS_DIR/linuxdeploy-${arch}.AppImage"
  download_tool "linuxdeploy" "$tool_url" "$tool_path"
}

find_or_download_appimagetool() {
  if [[ -n "${ORDO_APPIMAGETOOL:-}" ]]; then
    printf '%s\n' "$ORDO_APPIMAGETOOL"
    return
  fi
  if command -v appimagetool >/dev/null 2>&1; then
    command -v appimagetool
    return
  fi

  local arch tool_url tool_path
  arch="$(appimage_arch)"
  tool_url="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-${arch}.AppImage"
  tool_path="$TOOLS_DIR/appimagetool-${arch}.AppImage"
  download_tool "appimagetool" "$tool_url" "$tool_path"
}

need_command cp
need_command install
need_command mkdir
need_command rm

for required_file in \
  "$ROOT/Build-Ordo-Linux-Portable.sh" \
  "$ROOT/packaging/linux/AppRun" \
  "$ROOT/packaging/linux/ordo-appimage.desktop" \
  "$ROOT/packaging/linux/ordo.svg"; do
  if [[ ! -f "$required_file" ]]; then
    echo "Missing required packaging file: $required_file" >&2
    exit 1
  fi
done

if (( CHECK )); then
  "$ROOT/Build-Ordo-Linux-Portable.sh" --check
  if [[ -z "${ORDO_LINUXDEPLOY:-}" ]] && ! command -v linuxdeploy >/dev/null 2>&1; then
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
      echo "AppImage check failed: install curl/wget or set ORDO_LINUXDEPLOY." >&2
      exit 1
    fi
  fi
  if [[ -z "${ORDO_APPIMAGETOOL:-}" ]] && ! command -v appimagetool >/dev/null 2>&1; then
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
      echo "AppImage check failed: install curl/wget or set ORDO_APPIMAGETOOL." >&2
      exit 1
    fi
  fi
  echo "AppImage package check passed."
  echo "  root: $ROOT"
  echo "  appdir: $APPDIR"
  echo "  output: $OUTPUT"
  exit 0
fi

"$ROOT/Build-Ordo-Linux-Portable.sh"

rm -rf "$APPDIR" "$OUTPUT"
mkdir -p "$APPDIR/opt/ordo"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/scalable/apps"

cp -a "$PORTABLE_DIR/." "$APPDIR/opt/ordo/"
install -m 0755 "$ROOT/packaging/linux/AppRun" "$APPDIR/AppRun"
install -m 0644 "$ROOT/packaging/linux/ordo-appimage.desktop" "$APPDIR/usr/share/applications/ordo.desktop"
install -m 0644 "$ROOT/packaging/linux/ordo.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/ordo.svg"
install -m 0644 "$ROOT/packaging/linux/ordo-appimage.desktop" "$APPDIR/ordo.desktop"
install -m 0644 "$ROOT/packaging/linux/ordo.svg" "$APPDIR/ordo.svg"
install -m 0644 "$ROOT/packaging/linux/ordo.svg" "$APPDIR/.DirIcon"

LINUXDEPLOY="$(find_or_download_linuxdeploy)"
APPIMAGETOOL="$(find_or_download_appimagetool)"

echo "Bundling AppDir dependencies with linuxdeploy..."
if ! APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY" \
  --appdir "$APPDIR" \
  --desktop-file "$APPDIR/usr/share/applications/ordo.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/scalable/apps/ordo.svg" \
  --executable "$APPDIR/opt/ordo/bin/ordo" \
  --executable "$APPDIR/opt/ordo/bin/ordo-servo-shell"; then
  echo "linuxdeploy dependency scan failed; continuing with the raw AppDir." >&2
fi

echo "Building AppImage..."
ARCH="$(appimage_arch)" APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" "$APPDIR" "$OUTPUT"
chmod +x "$OUTPUT"

echo "Built Linux AppImage:"
echo "  $OUTPUT"
