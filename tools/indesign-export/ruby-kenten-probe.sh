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

# Ask InDesign how big, and where, it sets ruby and emphasis marks; see
# ruby-kenten-probe.jsx. Writes ruby-kenten.{idml,pdf,json} into OUT_DIR
# (default: a fresh temporary directory) and prints every text run of the
# PDF (font, size, origin) beside the characters' insertion points. Takes
# the InDesign GUI for a few seconds; it takes the shared lock.
#
#   bash tools/indesign-export/ruby-kenten-probe.sh [OUT_DIR]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-ruby-kenten-probe)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SHIM="$(mktemp -t paged-ruby-kenten-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
printf 'var PROBE_OUT = "%s";\n$.evalFile(File("%s"));\n' "$OUT" "$HERE/ruby-kenten-probe.jsx" > "$SHIM"
osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
rmdir "$LOCK" 2>/dev/null || true
python3 -c 'import json,sys; [print("error:", e) for e in json.load(open(sys.argv[1]))["errors"]]' "$OUT/ruby-kenten.json"
echo "==> the PDF's text runs (font, size, origin; y down from the page top)"
python3 "$HERE/pdf-text.py" "$OUT/ruby-kenten.pdf"
echo "==> $OUT"
