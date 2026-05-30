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
