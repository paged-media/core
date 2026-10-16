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

# Ask InDesign how it sets vertical text; see vertical-text-probe.jsx.
# InDesign authors the frames horizontally, this script turns every story
# vertical in the IDML (StoryOrientation="Vertical"), and InDesign opens
# that, reports its lines and exports the PDF. Writes
# vertical-text{,-author}.idml, vertical-text.pdf, the phases' JSON and
# vertical-text-glyphs.json (every glyph the PDF draws, per case, from
# pdf-glyphs.py) into OUT_DIR (default: a fresh temporary directory) and
# prints each case's columns: their glyphs, where each glyph's outline is
# drawn from, its writing mode and angle. Takes the InDesign GUI for a few
# seconds; it takes the shared lock.
#
#   bash tools/indesign-export/vertical-text-probe.sh [OUT_DIR]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-vertical-text-probe)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SHIM="$(mktemp -t paged-vertical-text-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
run_phase() {
    printf 'var PROBE_OUT = "%s";\nvar PROBE_PHASE = "%s";\n$.evalFile(File("%s"));\n' \
        "$OUT" "$1" "$HERE/vertical-text-probe.jsx" > "$SHIM"
    osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
}
run_phase author
python3 - "$OUT" <<'TURN'
import re, sys, zipfile
out = sys.argv[1]
src = zipfile.ZipFile(f"{out}/vertical-text-author.idml")
# Each frame's label (its case) by the story it holds.
label_of = {}
for n in src.namelist():
    if n.startswith("Spreads/"):
        s = src.read(n).decode("utf-8")
        for frame in re.finditer(r'<TextFrame [^>]*ParentStory="([^"]+)".*?</TextFrame>', s, re.S):
            m = re.search(r'Key="Label" Value="([^"]*)"', frame.group(0))
            label_of[frame.group(1)] = m.group(1) if m else ""
with zipfile.ZipFile(f"{out}/vertical-text.idml", "w") as z:
    z.writestr(zipfile.ZipInfo("mimetype"), src.read("mimetype"), compress_type=zipfile.ZIP_STORED)
    for n in src.namelist():
        if n == "mimetype":
            continue
        b = src.read(n)
        if n.startswith("Stories/"):
            story = re.search(rb'<Story Self="([^"]+)"', b).group(1).decode()
            label = label_of.get(story, "")
            if not label.startswith("horizontal"):
                b = b.replace(b'StoryOrientation="Horizontal"', b'StoryOrientation="Vertical"')
        z.writestr(n, b, compress_type=zipfile.ZIP_DEFLATED)
TURN
run_phase read
rmdir "$LOCK" 2>/dev/null || true
python3 - "$OUT" "$HERE/pdf-glyphs.py" <<'SHOW'
import json, subprocess, sys
out, tool = sys.argv[1], sys.argv[2]
author = json.load(open(f"{out}/vertical-text-author.json"))
probe = json.load(open(f"{out}/vertical-text-read.json"))
for e in author["errors"] + probe["errors"]:
    print("error:", e)
glyphs = [json.loads(l) for l in subprocess.run(
    ["python3", tool, f"{out}/vertical-text.pdf"], capture_output=True, text=True).stdout.splitlines()]
summary = {}
for case in probe["cases"]:
    mine = [g for g in glyphs if g["page"] == case["page"]]
    summary[case["name"]] = {"bounds": case["bounds"], "leading": case.get("leading"),
                             "lines": case.get("lines", []), "glyphs": mine}
    print(f"== {case['name']}  bounds {case['bounds']}  leading {case.get('leading')}")
    cols = {}
    for g in mine:
        cols.setdefault(round(g["x"], 1), []).append(g)
    for x in sorted(cols, reverse=True):
        print(f"   x {x:8.2f}: " + " ".join(
            f"{g['char']}{g['y']:.1f}{'' if g['mode'] == 'v' else 'h'}"
            f"{'' if abs(g['angle']) < 0.1 else '@' + str(round(g['angle']))}"
            f"{'' if abs(g['size'] - 20) < 0.01 else '/' + format(g['size'], 'g')}" for g in cols[x]))
json.dump(summary, open(f"{out}/vertical-text-glyphs.json", "w"), ensure_ascii=False, indent=1)
SHOW
echo "==> $OUT"
