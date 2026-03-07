#!/bin/bash
# =============================================================================
# package-macos.sh — Create a signed/notarized macOS app bundle for Jarvis
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

APP_NAME="Jarvis"
EXECUTABLE_NAME="Jarvis"
BINARY_NAME="jarvis"
BUNDLE_ID="com.dylanburton.jarvis"
MIN_SYSTEM_VERSION="12.0"
ENTITLEMENTS_PATH="packaging/macos/entitlements.mac.plist"

BUILD_MODE="release"
SIGN_IDENTITY="${CODE_SIGN_IDENTITY:-}"
NOTARIZE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            BUILD_MODE="debug"
            shift
            ;;
        --release)
            BUILD_MODE="release"
            shift
            ;;
        --sign)
            shift
            SIGN_IDENTITY="${1:-$SIGN_IDENTITY}"
            shift
            ;;
        --notarize)
            NOTARIZE=true
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

VERSION="${JARVIS_VERSION:-$(python3 - <<'PY'
from pathlib import Path
import tomllib

data = tomllib.loads(Path('Cargo.toml').read_text())
print(data.get('workspace', {}).get('package', {}).get('version', '0.1.0'))
PY
)}"

if [[ "$BUILD_MODE" == "release" ]]; then
    cargo build --release
    BINARY_DIR="target/release"
else
    cargo build
    BINARY_DIR="target/debug"
fi

APP_DIR="${BINARY_DIR}/${APP_NAME}.app"
DMG_PATH="${BINARY_DIR}/jarvis-macos-$(uname -m).dmg"
DMG_STAGING="${BINARY_DIR}/dmg-staging"

echo "Packaging ${APP_NAME} v${VERSION} for macOS (${BUILD_MODE})..."

rm -rf "$APP_DIR" "$DMG_STAGING" "$DMG_PATH"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources/assets"

cp "${BINARY_DIR}/${BINARY_NAME}" "$APP_DIR/Contents/MacOS/${EXECUTABLE_NAME}"
chmod +x "$APP_DIR/Contents/MacOS/${EXECUTABLE_NAME}"

if [[ -d "assets/panels" ]]; then
    cp -R "assets/panels" "$APP_DIR/Contents/Resources/assets/"
fi

if [[ -f "assets/jarvis-icon.png" ]]; then
    cp "assets/jarvis-icon.png" "$APP_DIR/Contents/Resources/"
fi

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>${EXECUTABLE_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>${MIN_SYSTEM_VERSION}</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

if [[ -n "$SIGN_IDENTITY" ]]; then
    codesign --force --deep --options runtime \
        --entitlements "$ENTITLEMENTS_PATH" \
        --sign "$SIGN_IDENTITY" \
        "$APP_DIR"
else
    codesign --force --deep --sign - "$APP_DIR"
fi

mkdir -p "$DMG_STAGING"
cp -R "$APP_DIR" "$DMG_STAGING/"
ln -s /Applications "$DMG_STAGING/Applications"

hdiutil create -volname "$APP_NAME" -srcfolder "$DMG_STAGING" -ov -format UDZO "$DMG_PATH"

if [[ -n "$SIGN_IDENTITY" ]]; then
    codesign --force --sign "$SIGN_IDENTITY" "$DMG_PATH"
fi

if [[ "$NOTARIZE" == true ]]; then
    : "${APPLE_ID:?APPLE_ID is required for notarization}"
    : "${APPLE_ID_PASSWORD:?APPLE_ID_PASSWORD is required for notarization}"
    : "${APPLE_TEAM_ID:?APPLE_TEAM_ID is required for notarization}"

    xcrun notarytool submit "$DMG_PATH" \
        --apple-id "$APPLE_ID" \
        --password "$APPLE_ID_PASSWORD" \
        --team-id "$APPLE_TEAM_ID" \
        --wait

    xcrun stapler staple "$APP_DIR"
    xcrun stapler staple "$DMG_PATH"
fi

rm -rf "$DMG_STAGING"

echo "Built app: $APP_DIR"
echo "Built dmg: $DMG_PATH"
