#!/usr/bin/env bash
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# This file is part of paged (https://paged.media) and is additionally
# available under the Paged Media Enterprise License (PMEL). Full
# copyright and license information is available in LICENSE.md which is
# distributed with this source code.
#
#  @copyright  Copyright (c) And The Next GmbH
#  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
#

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
APP="${INDESIGN_APP:-Adobe InDesign 2025}"

# The driver reads both of these from the environment. `PAGED_CORPUS_DIR`
# defaults to this repo's corpus/generated; `PAGED_EXPORT_ONLY=<stem>`
# re-exports a single fixture, which is what you want when one
# reference is wrong — rewriting every PDF rebaselines every threshold
# at once.
CORPUS_DIR="${PAGED_CORPUS_DIR:-$ROOT/corpus/generated}"
ONLY="${PAGED_EXPORT_ONLY:-}"

if [ ! -f "$JSX" ]; then
    echo "missing $JSX"
    exit 1
fi

# InDesign is a separate, already-running process, so it cannot see this
# shell's environment. Hand the two settings over as globals in a shim
# that then evaluates the driver.
SHIM="$(mktemp -t paged-export-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PAGED_CORPUS_DIR = "$CORPUS_DIR";
var PAGED_EXPORT_ONLY = "$ONLY";
\$.evalFile(File("$JSX"));
JSX

# A cold InDesign launch plus a document open runs well past
# AppleScript's default 120 s reply timeout (measured: a 60 s open
# killed a run in 2026-09).
osascript <<EOF
with timeout of 900 seconds
    tell application "$APP"
        activate
        do script POSIX file "$SHIM" language javascript
    end tell
end timeout
EOF

echo "==> InDesign export pass complete (see corpus/generated/*.pdf)"
