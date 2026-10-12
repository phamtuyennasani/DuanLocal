#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP_DIR="$ROOT/crates/app"
RESOURCE_DIR="$APP_DIR/resources"
TARGET=${DEVSTACK_TARGET:-}

if [ "$(uname -s)" != "Darwin" ]; then
  echo "This script must run on macOS." >&2
  exit 1
fi

if [ -z "$TARGET" ]; then
  TARGET=$(rustc -vV | awk '/^host: / {print $2}')
fi
case "$TARGET" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *)
    echo "Unsupported macOS target: $TARGET" >&2
    echo "Set DEVSTACK_TARGET=aarch64-apple-darwin or x86_64-apple-darwin." >&2
    exit 1
    ;;
esac

if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "Rust target $TARGET is not installed." >&2
  echo "Install it with: rustup target add $TARGET" >&2
  exit 1
fi

if ! cargo tauri --version >/dev/null 2>&1 && ! command -v cargo-tauri >/dev/null 2>&1; then
  echo "Tauri CLI is required. Install it once with:" >&2
  echo "  cargo install tauri-cli --version '^2'" >&2
  exit 1
fi

mkdir -p "$RESOURCE_DIR"

echo "→ Building devctl for $TARGET"
cargo build --release -p devctl --target "$TARGET"
cp "$ROOT/target/$TARGET/release/devctl" "$RESOURCE_DIR/devctl"
chmod 755 "$RESOURCE_DIR/devctl"

echo "→ Building DevStack .app for $TARGET"
(cd "$APP_DIR" && cargo tauri build --target "$TARGET" --bundles app --config tauri.macos.conf.json)

APP_BUNDLE="$ROOT/target/$TARGET/release/bundle/macos/DevStack.app"
DMG_DIR="$ROOT/target/$TARGET/release/bundle/dmg"
VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "devstack-app") | .version')
DMG_PATH="$DMG_DIR/DevStack_${VERSION}_${TARGET%-apple-darwin}.dmg"
if [ ! -d "$APP_BUNDLE" ]; then
  echo "Missing app bundle: $APP_BUNDLE" >&2
  exit 1
fi

APP_BINARY=$(find "$APP_BUNDLE/Contents/MacOS" -maxdepth 1 -type f -perm -111 -print -quit)
if [ -z "$APP_BINARY" ]; then
  echo "Missing executable in: $APP_BUNDLE/Contents/MacOS" >&2
  exit 1
fi
if [ ! -x "$APP_BUNDLE/Contents/Resources/devctl" ]; then
  echo "Missing bundled devctl: $APP_BUNDLE/Contents/Resources/devctl" >&2
  exit 1
fi

mkdir -p "$DMG_DIR"
rm -f "$DMG_PATH"
STAGING=$(mktemp -d "${TMPDIR:-/tmp}/devstack-dmg.XXXXXX")
trap 'rm -rf "$STAGING"' EXIT
cp -R "$APP_BUNDLE" "$STAGING/DevStack.app"
ln -s /Applications "$STAGING/Applications"

echo "→ Creating headless DMG"
hdiutil create -volname DevStack -srcfolder "$STAGING" -ov -format UDZO "$DMG_PATH" >/dev/null

if [ ! -s "$DMG_PATH" ]; then
  echo "DMG was not created: $DMG_PATH" >&2
  exit 1
fi

echo
echo "Packaging complete. App:"
printf '  %s\n' "$APP_BUNDLE"
echo "DMG:"
printf '  %s\n' "$DMG_PATH"

echo
echo "Architecture:"
file "$APP_BINARY"
file "$APP_BUNDLE/Contents/Resources/devctl"
