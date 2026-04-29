#!/usr/bin/env bash
# tools/indesign-export/run-export.sh
#
# Runs the export-pdfs.jsx driver against the local InDesign install.
# macOS-only; on Windows invoke InDesign with the JSX directly.
#
# The driver itself reads its INPUT_DIR + PRESET_NAME from constants
# at the top of export-pdfs.jsx — edit those if you point it at a
# different output location.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
JSX="$ROOT/tools/indesign-export/export-pdfs.jsx"
APP="${INDESIGN_APP:-Adobe InDesign 2024}"

if [ ! -f "$JSX" ]; then
    echo "missing $JSX"
    exit 1
fi

osascript <<EOF
tell application "$APP"
    activate
    do script POSIX file "$JSX" language javascript
end tell
EOF

echo "==> InDesign export pass complete (see corpus/generated/*.pdf)"
