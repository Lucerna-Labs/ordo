#!/usr/bin/env bash
set -Eeuo pipefail

ORDO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAUNCHER="$ORDO_ROOT/Launch-Ordo-Servo.sh"
APP_DIR="$HOME/.local/share/applications"
DESKTOP_DIR="${XDG_DESKTOP_DIR:-$HOME/Desktop}"
APP_FILE="$APP_DIR/ordo.desktop"
DESKTOP_FILE="$DESKTOP_DIR/Ordo.desktop"

if [[ ! -f "$LAUNCHER" ]]; then
  echo "ERROR: Launch-Ordo-Servo.sh not found at $LAUNCHER" >&2
  exit 1
fi

chmod +x "$LAUNCHER"
mkdir -p "$APP_DIR"
mkdir -p "$DESKTOP_DIR"

cat >"$APP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=Ordo
Comment=Launch Ordo through the embedded Servo shell
Exec=$LAUNCHER
Path=$ORDO_ROOT
Terminal=false
Categories=Development;Utility;
StartupNotify=true
EOF

cp "$APP_FILE" "$DESKTOP_FILE"
chmod +x "$APP_FILE" "$DESKTOP_FILE"

if command -v gio >/dev/null 2>&1; then
  gio set "$DESKTOP_FILE" metadata::trusted true >/dev/null 2>&1 || true
fi

echo "Created Linux app launcher:"
echo "  $APP_FILE"
echo "Created desktop shortcut:"
echo "  $DESKTOP_FILE"
