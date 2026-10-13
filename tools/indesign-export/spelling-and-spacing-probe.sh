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

# Ask InDesign how it spells drop-cap detail, ruby, small caps and type on
# a path, and how it lays out letter spacing, tracking at a line's end,
# superscript, paragraph rules and type on a path; see
# spelling-and-spacing-probe.jsx. InDesign builds the sample itself and
# exports it to OUT_DIR (default: a temp dir) as IDML, PDF and JSON; this
# script then prints the spellings the IDML carries and measures, from
# InDesign's PDF, where the rules and the type on a path landed.
#
# Takes the InDesign GUI for a few seconds; it takes the shared lock:
#
#   bash tools/indesign-export/spelling-and-spacing-probe.sh [OUT_DIR]
#
# The answers of InDesign 20.0.1 are pinned in
# crates/paged-renderer/tests/text_glyph_level.rs (letter spacing,
# tracking at a line's end), pipeline/text_path.rs (type on a path) and
# plugin-publish's importer tests (the spellings).
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-spelling-probe)}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/spelling-and-spacing-probe.jsx"
SHIM="$(mktemp -t paged-spelling-probe-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
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
rmdir "$LOCK" 2>/dev/null || true

IDML="$OUT/spelling-and-spacing.idml"
PDF="$OUT/spelling-and-spacing.pdf"
[ -f "$IDML" ] || { echo "InDesign wrote no IDML; see $OUT/spelling-and-spacing.json" >&2; exit 1; }

echo "==> spellings InDesign wrote ($IDML)"
unzip -p "$IDML" 'Stories/*.xml' 'Spreads/*.xml' \
    | grep -oE '(DropcapDetail|DropCapDetail|RubyFlag|RubyType|Capitalization|PathAlignment|TextAlignment|PathTypeAlignment)="[^"]*"' \
    | sort | uniq -c

echo "==> rules and type on a path in InDesign's PDF"
pdftoppm -r 1152 -gray "$PDF" "$OUT/page" >/dev/null
python3 -I - "$OUT" <<'PY'
import json, glob, sys
out = sys.argv[1]
probe = json.load(open(f'{out}/spelling-and-spacing.json'))
S = 16.0  # px per pt at 1152 dpi

def pgm(path):
    data = open(path, 'rb').read()
    toks, i = [], 0
    while len(toks) < 4:
        while data[i:i+1].isspace(): i += 1
        j = i
        while not data[j:j+1].isspace(): j += 1
        toks.append(data[i:j]); i = j
    return int(toks[1]), int(toks[2]), data[i + 1:]

pages = sorted(glob.glob(f'{out}/page-*.pgm'))
w, h, px = pgm(pages[0])
# Paragraph rules: rows inked across most of the 480 pt column.
for name in ('ruleabove', 'rulebelow'):
    fr = probe['frames'][name]
    base = fr['lines'][0]['baseline']
    top, bot = int((base - 30) * S), int((base + 30) * S)
    wide = [r for r in range(top, bot) if sum(1 for b in px[r*w + int(60*S):r*w + int(535*S):16] if b < 128) > 0.6 * (475 * S / 16)]
    if wide:
        print(f'{name}: rule {wide[0]/S:.3f} → {(wide[-1]+1)/S:.3f} pt, baseline {base:.3f}: '
              f'top {wide[0]/S - base:+.3f}, bottom {(wide[-1]+1)/S - base:+.3f}')
# Type on a path, page 2: the ink of each row against its line.
w, h, px = pgm(pages[1])
for row in probe['paths']:
    y = row['line']
    rows = [r for r in range(int((y - 60) * S), int((y + 40) * S)) if min(px[r*w:(r+1)*w]) < 128]
    if rows:
        print(f"{row['textAlignment']:>9} on {row['pathAlignment']:<6} line {y}: "
              f"baseline (ink bottom) {(rows[-1]+1)/S - y:+.3f} pt from the line")
PY
echo "==> $OUT"
