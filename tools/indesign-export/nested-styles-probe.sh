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
# Ask InDesign how it spells and sets nested styles; see
# nested-styles-probe.jsx. Writes nested-styles-probe.{json,idml,pdf} into
# the directory given (default: a fresh temporary one). Takes the InDesign
# GUI for a few seconds.
#
#   bash tools/indesign-export/nested-styles-probe.sh [out-dir]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-nested-probe)}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/nested-styles-probe.jsx"
SHIM="$(mktemp -t paged-nested-probe-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PROBE_OUT = "$OUT";
\$.evalFile(File("$JSX"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
echo "==> $OUT/nested-styles-probe.json"
