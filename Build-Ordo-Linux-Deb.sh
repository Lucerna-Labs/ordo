#!/usr/bin/env bash
set -Eeuo pipefail

VERSION="${ORDO_PACKAGE_VERSION:-0.1.0}"
ARCH="${ORDO_PACKAGE_ARCH:-amd64}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$ROOT/dist"
STAGE="$DIST/ordo_${VERSION}_${ARCH}"
PACKAGE="$DIST/ordo_${VERSION}_${ARCH}.deb"

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

need_command cargo
need_command npm
need_command dpkg-deb

rm -rf "$STAGE" "$PACKAGE"
mkdir -p "$STAGE/DEBIAN"
mkdir -p "$STAGE/opt/ordo/bin"
mkdir -p "$STAGE/opt/ordo/ordo-studio"
mkdir -p "$STAGE/opt/ordo/docs"
mkdir -p "$STAGE/usr/share/applications"

echo "Building Ordo Studio..."
npm --prefix "$ROOT/ordo-studio" ci
npm --prefix "$ROOT/ordo-studio" run build

echo "Building Ordo runtime..."
cargo build --release -p ordo-cli

echo "Building Ordo Servo shell..."
cargo build --manifest-path "$ROOT/ordo-servo-shell/Cargo.toml" --features servo-engine --release

install -m 0755 "$ROOT/target/release/ordo" "$STAGE/opt/ordo/bin/ordo"
install -m 0755 "$ROOT/ordo-servo-shell/target/release/ordo-servo-shell" "$STAGE/opt/ordo/bin/ordo-servo-shell"
install -m 0755 "$ROOT/packaging/linux/ordo-launcher" "$STAGE/opt/ordo/bin/ordo-launcher"
install -m 0644 "$ROOT/packaging/linux/ordo.desktop" "$STAGE/usr/share/applications/ordo.desktop"

cp -a "$ROOT/ordo-studio/dist" "$STAGE/opt/ordo/ordo-studio/dist"
for doc in official-memory.md build-history.md fixbook.md; do
  if [[ -f "$ROOT/docs/$doc" ]]; then
    install -m 0644 "$ROOT/docs/$doc" "$STAGE/opt/ordo/docs/$doc"
  fi
done

cat >"$STAGE/DEBIAN/control" <<EOF
Package: ordo
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: Lucerna Labs
Depends: curl, libssl3, libx11-6, libxcb1, libxkbcommon0, libwayland-client0, libegl1, libgles2
Description: Ordo local-first AI runtime and Servo operator studio
 Ordo is a local-first AI runtime and operator studio rendered through an
 embedded Servo shell.
EOF

cat >"$STAGE/DEBIAN/postinst" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database /usr/share/applications || true
fi
exit 0
EOF
chmod 0755 "$STAGE/DEBIAN/postinst"

dpkg-deb --build "$STAGE" "$PACKAGE"
echo "Built $PACKAGE"
