#!/bin/sh
# Render the app icon in every color from assets/brand/dusty-icon.svg (purple master) with headless Chrome:
#   assets/brand/dusty-icon[-mint|-cream].png   1024 px, for docs
#   app/assets/icons/{purple,mint,cream}.png    512 px, embedded in the app for the Icon menu
#   app/assets/Dusty.icns                       the bundle icon (purple)
set -e
cd "$(dirname "$0")/.."
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p app/assets/icons
cp assets/brand/dusty-front.png "$TMP/"

render() { # name top bottom speck sparkle
  sed -e "s/#9A82FF/$2/" -e "s/#6544E6/$3/" \
      -e "s/<g fill=\"#FFFFFF\" opacity=\"0.5\">/<g fill=\"$4\" opacity=\"0.5\">/" \
      -e "s/<path fill=\"#FFFFFF\" d=\"M230/<path fill=\"$5\" d=\"M230/" \
      assets/brand/dusty-icon.svg > "$TMP/$1.svg"
  "$CHROME" --headless=new --hide-scrollbars --allow-file-access-from-files --default-background-color=00000000 \
    --window-size=1024,1024 --screenshot="$TMP/$1.png" "file://$TMP/$1.svg" 2>/dev/null
  sips -z 512 512 "$TMP/$1.png" --out "app/assets/icons/$1.png" >/dev/null
}
render purple "#9A82FF" "#6544E6" "#FFFFFF" "#FFFFFF"
render mint   "#8FE6C4" "#3FBF8E" "#FFFFFF" "#FFFFFF"
render cream  "#FFF6E8" "#F6DFC4" "#C9B39A" "#B3A5D3"
cp "$TMP/purple.png" assets/brand/dusty-icon.png
cp "$TMP/mint.png" assets/brand/dusty-icon-mint.png
cp "$TMP/cream.png" assets/brand/dusty-icon-cream.png

mkdir "$TMP/Dusty.iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s "$TMP/purple.png" --out "$TMP/Dusty.iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$TMP/purple.png" --out "$TMP/Dusty.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$TMP/Dusty.iconset" -o app/assets/Dusty.icns
echo "Icons written."
