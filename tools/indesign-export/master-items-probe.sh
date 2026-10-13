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

# Ask InDesign whether a page's own ShowMasterItems counts; see
# master-items-probe.jsx. InDesign's own export writes the attribute on
# <Spread> only; this derives two variants where the spread and the page
# disagree, has InDesign print both, and reports whether the master's
# rectangle printed. InDesign 20.0.1 follows the spread and ignores the
# page. Takes the InDesign GUI for a few seconds; it takes the shared lock.
#
#   bash tools/indesign-export/master-items-probe.sh [OUT_DIR]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-master-probe)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/master-items-probe.jsx"
SHIM="$(mktemp -t paged-master-probe-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
run() {
    printf 'var PROBE_OUT = "%s";\nvar PROBE_MODE = "%s";\n$.evalFile(File("%s"));\n' "$OUT" "$1" "$JSX" > "$SHIM"
    osascript -e 'with timeout of 900 seconds' -e "tell application \"$APP\"" \
        -e "«event K2  dosc» (POSIX file \"$SHIM\") given «class doLg»:«constant ****JSLg»" \
        -e 'end tell' -e 'end timeout' >/dev/null
}
run build
echo "==> InDesign writes: $(unzip -p "$OUT/master-items.idml" 'Spreads/*.xml' | grep -oE '<(Spread|Page) [^>]*' | grep -oE '^<[A-Za-z]+|ShowMasterItems="[^"]*"' | tr '\n' ' ')"
python3 -I - "$OUT" <<'PY'
import re, sys, zipfile
out = sys.argv[1]
for name, spread, page in [('spread-true-page-false', b'true', b'false'), ('spread-false-page-true', b'false', b'true')]:
    src = zipfile.ZipFile(f'{out}/master-items.idml')
    with zipfile.ZipFile(f'{out}/{name}.idml', 'w') as z:
        for n in src.namelist():
            data = src.read(n)
            if n.startswith('Spreads/'):
                data = re.sub(rb'(<Spread [^>]*?)ShowMasterItems="\w+"', rb'\1ShowMasterItems="' + spread + b'"', data)
                data = data.replace(b'<Page ', b'<Page ShowMasterItems="' + page + b'" ', 1)
            z.writestr(n, data, compress_type=zipfile.ZIP_STORED if n == 'mimetype' else zipfile.ZIP_DEFLATED)
PY
run open
rmdir "$LOCK" 2>/dev/null || true
for v in spread-true-page-false spread-false-page-true; do
    pdftoppm -r 36 -gray "$OUT/$v.pdf" "$OUT/$v" >/dev/null
    python3 -I -c "
import sys
d = open(sys.argv[1], 'rb').read(); t = d.split(b'\n', 3); px = t[3]
dark = sum(1 for b in px if b < 64)
print(f'{sys.argv[2]:24} master rectangle {\"printed\" if dark else \"hidden\"} ({dark} dark pixels)')" "$OUT/$v-1.pgm" "$v"
done
echo "==> $OUT"
