#!/bin/sh
# Download the pinned Sparkle release into app/vendor/ (gitignored).
#   app/vendor/Sparkle.framework   embedded by app/bundle.sh
#   app/vendor/sparkle-bin/        sign_update, generate_keys, … used by the release workflow
set -e
cd "$(dirname "$0")/../app"
VERSION="2.10.1"
SHA256="1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27"

if [ -f vendor/.sparkle-version ] && [ "$(cat vendor/.sparkle-version)" = "$VERSION" ]; then
  echo "Sparkle $VERSION already in app/vendor"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL -o "$TMP/sparkle.tar.xz" \
  "https://github.com/sparkle-project/Sparkle/releases/download/$VERSION/Sparkle-$VERSION.tar.xz"
echo "$SHA256  $TMP/sparkle.tar.xz" | shasum -a 256 -c - >/dev/null \
  || { echo "Sparkle download checksum mismatch" >&2; exit 1; }
mkdir -p "$TMP/x" && tar -xf "$TMP/sparkle.tar.xz" -C "$TMP/x"

rm -rf vendor/Sparkle.framework vendor/sparkle-bin
mkdir -p vendor
cp -R "$TMP/x/Sparkle.framework" vendor/
cp -R "$TMP/x/bin" vendor/sparkle-bin
echo "$VERSION" > vendor/.sparkle-version
echo "Sparkle $VERSION ready in app/vendor"
