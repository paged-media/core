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

# Ask InDesign which composers print ruby and kenten; see
# cjk-composer-probe.jsx. Prints, per frame (composer), the ruby text and
# the emphasis marks InDesign's PDF carries above it. Takes the InDesign
# GUI for a few seconds; it takes the shared lock.
#
#   bash tools/indesign-export/cjk-composer-probe.sh [OUT_DIR]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-cjk-probe)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/cjk-composer-probe.jsx"
SHIM="$(mktemp -t paged-cjk-probe-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
printf 'var PROBE_OUT = "%s";\n$.evalFile(File("%s"));\n' "$OUT" "$JSX" > "$SHIM"
osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
rmdir "$LOCK" 2>/dev/null || true
[ -f "$OUT/cjk-composer.pdf" ] || { cat "$OUT/cjk-composer.error.txt" 2>/dev/null; exit 1; }
echo "==> composers InDesign wrote: $(unzip -p "$OUT/cjk-composer.idml" 'Stories/*.xml' | grep -oE 'Composer="[^"]*"' | tr '\n' ' ')"
echo "==> what each frame prints (y of the line, then its words)"
pdftotext -bbox "$OUT/cjk-composer.pdf" - | grep '<word' \
    | sed -E 's/.*yMin="([0-9.]+)".*>(.*)<\/word>/\1 \2/' | sort -n
echo "    ruby かんじ and the KentenGeneric marks appear only above the frames"
echo "    set with HL Composer J and HL Single J (InDesign 20.0.1)."
echo "==> $OUT"
