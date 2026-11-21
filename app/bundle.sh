#!/bin/sh
# Build Dusty.app.
#   ./bundle.sh                 release build for this Mac's architecture
#   ./bundle.sh debug           debug build
#   ./bundle.sh universal       release build for Apple silicon + Intel (lipo)
# Signing is optional: set DUSTY_SIGN_IDENTITY (e.g. "Developer ID Application: Name (TEAMID)")
# to sign with the hardened runtime; otherwise the app is ad-hoc signed for local use.
# Automatic updates: run scripts/fetch-sparkle.sh once so Sparkle.framework gets embedded.
# Without it the app still builds, just with updates off (DUSTY_REQUIRE_SPARKLE=1 makes that an error).
set -e
cd "$(dirname "$0")"
MODE="${1:-release}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUILD="${DUSTY_BUILD_NUMBER:-1}"

case "$MODE" in
  debug)
    cargo build
    BIN="target/debug/dusty-app" ;;
  release)
    cargo build --release
    BIN="target/release/dusty-app" ;;
  universal)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 || true
    cargo build --release --target aarch64-apple-darwin
    cargo build --release --target x86_64-apple-darwin
    mkdir -p target/universal
    lipo -create -output target/universal/dusty \
      target/aarch64-apple-darwin/release/dusty-app target/x86_64-apple-darwin/release/dusty-app
    BIN="target/universal/dusty" ;;
  *) echo "usage: $0 [release|debug|universal]" >&2; exit 2 ;;
esac

APP="target/Dusty.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Dusty"
cp assets/Dusty.icns "$APP/Contents/Resources/Dusty.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Dusty</string>
  <key>CFBundleDisplayName</key><string>Dusty</string>
  <key>CFBundleIdentifier</key><string>io.github.deuspheara.dusty</string>
  <key>CFBundleExecutable</key><string>Dusty</string>
  <key>CFBundleIconFile</key><string>Dusty</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>SUFeedURL</key><string>https://deuspheara.github.io/dusty/appcast.xml</string>
  <key>SUPublicEDKey</key><string>1AyPHjqtQ3vMWduIHwzPS8q/Ek3N9eCuDjldAy53cgI=</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>
</dict></plist>
PLIST

# Sparkle (automatic updates). Dusty isn't sandboxed, so Sparkle's XPC services aren't needed.
SPARKLE=""
if [ -d vendor/Sparkle.framework ]; then
  mkdir -p "$APP/Contents/Frameworks"
  ditto vendor/Sparkle.framework "$APP/Contents/Frameworks/Sparkle.framework"
  SPARKLE="$APP/Contents/Frameworks/Sparkle.framework"
  rm -rf "$SPARKLE/Versions/B/XPCServices" "$SPARKLE/XPCServices"
elif [ "${DUSTY_REQUIRE_SPARKLE:-0}" = "1" ]; then
  echo "Sparkle.framework missing: run scripts/fetch-sparkle.sh" >&2
  exit 1
else
  echo "note: no app/vendor/Sparkle.framework, building without automatic updates" >&2
fi

# sign inside-out: Sparkle's helpers, the framework, then the app
if [ -n "$DUSTY_SIGN_IDENTITY" ]; then
  sign() { codesign --force --options runtime --timestamp --sign "$DUSTY_SIGN_IDENTITY" "$@"; }
else
  sign() { codesign --force --sign - "$@"; }
fi
if [ -n "$SPARKLE" ]; then
  sign "$SPARKLE/Versions/B/Autoupdate"
  sign "$SPARKLE/Versions/B/Updater.app"
  sign "$SPARKLE"
fi
sign "$APP"
echo "Built $APP ($VERSION, $MODE)"
