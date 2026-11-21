#!/bin/sh
# Rebuild Dusty's in-app icons (app/assets/icons/ui/*.svg) from gen.py, plus an HTML contact sheet.
set -e
cd "$(dirname "$0")/../.."
python3 scripts/ui-icons/gen.py app/assets/icons/ui
python3 scripts/ui-icons/preview.py app/assets/icons/ui "${1:-${TMPDIR:-/tmp}/dusty-icons.html}"
echo "preview: ${1:-${TMPDIR:-/tmp}/dusty-icons.html}"
