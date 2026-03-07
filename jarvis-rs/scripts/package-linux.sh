#!/bin/bash
# =============================================================================
# package-linux.sh — Create Linux release artifacts for Jarvis
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

APP_NAME="jarvis"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
VERSION="${JARVIS_VERSION:-$(python3 - <<'PY'
from pathlib import Path
import tomllib

data = tomllib.loads(Path('Cargo.toml').read_text())
print(data.get('workspace', {}).get('package', {}).get('version', '0.1.0'))
PY
)}"

echo "Packaging ${APP_NAME} v${VERSION} for Linux (${ARCH})..."

cargo build --release

PACKAGE_ROOT="target/release/jarvis-linux-${ARCH}"
DEB_DIR="target/release/deb-staging"
TARBALL_PATH="target/release/jarvis-linux-${ARCH}.tar.gz"
DEB_PATH="target/release/${APP_NAME}_${VERSION}_${ARCH}.deb"

rm -rf "$PACKAGE_ROOT" "$DEB_DIR" "$TARBALL_PATH" "$DEB_PATH"
mkdir -p "$PACKAGE_ROOT/assets" "$DEB_DIR/DEBIAN" "$DEB_DIR/usr/bin" \
    "$DEB_DIR/usr/lib/jarvis/assets" "$DEB_DIR/usr/share/applications"

cp "target/release/${APP_NAME}" "$PACKAGE_ROOT/"
if [[ -d "assets/panels" ]]; then
    cp -R "assets/panels" "$PACKAGE_ROOT/assets/"
fi

tar czf "$TARBALL_PATH" -C "target/release" "$(basename "$PACKAGE_ROOT")"

cp "target/release/${APP_NAME}" "$DEB_DIR/usr/lib/jarvis/${APP_NAME}"
if [[ -d "assets/panels" ]]; then
    cp -R "assets/panels" "$DEB_DIR/usr/lib/jarvis/assets/"
fi

cat > "$DEB_DIR/usr/bin/${APP_NAME}" <<'WRAPPER'
#!/bin/sh
exec /usr/lib/jarvis/jarvis "$@"
WRAPPER
chmod +x "$DEB_DIR/usr/bin/${APP_NAME}"

cat > "$DEB_DIR/DEBIAN/control" <<CTRL
Package: ${APP_NAME}
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Depends: libx11-6, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libgtk-3-0, libsoup-3.0-0
Maintainer: Dylan Burton <dylan@example.com>
Description: Jarvis desktop app
 Cross-platform Jarvis desktop app with local panels, AI tooling,
 chat, games, and GPU-accelerated rendering.
CTRL

cat > "$DEB_DIR/usr/share/applications/${APP_NAME}.desktop" <<DESKTOP
[Desktop Entry]
Name=Jarvis
Comment=Jarvis desktop app
Exec=jarvis
Terminal=false
Type=Application
Categories=Utility;
Keywords=jarvis;assistant;terminal;chat;
DESKTOP

dpkg-deb --build "$DEB_DIR" "$DEB_PATH"
rm -rf "$DEB_DIR"

echo "Built tarball: $TARBALL_PATH"
echo "Built deb: $DEB_PATH"
