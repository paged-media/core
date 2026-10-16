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

# Ask InDesign which of an object style's corner attributes it reads; see
# object-style-corners-probe.jsx. InDesign authors a style with four
# rounded 20 pt corners on one rectangle and exports it; this script
# rewrites the style's corners into variants (the all-corner pair only, the
# per-corner attributes only, both disagreeing, neither, per-corner options
# without radii), strips the rectangle's own corner attributes so it takes
# the style's, and has InDesign read every variant back. Writes
# object-style-corners-{author,read}.json and the variants into OUT_DIR
# (default: a fresh temporary directory). Takes the InDesign GUI for a few
# seconds; it takes the shared lock.
#
#   bash tools/indesign-export/object-style-corners-probe.sh [OUT_DIR]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-object-style-corners-probe)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/object-style-corners-probe.jsx"
SHIM="$(mktemp -t paged-object-style-corners-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
run_phase() {
    printf 'var PROBE_OUT = "%s";\nvar PROBE_PHASE = "%s";\n$.evalFile(File("%s"));\n' "$OUT" "$1" "$JSX" > "$SHIM"
    osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
}
run_phase author
python3 - "$OUT" <<'PY'
import os, re, sys, zipfile
out = sys.argv[1]
src = zipfile.ZipFile(os.path.join(out, "object-style-corners.idml"))
parts = {n: src.read(n) for n in src.namelist()}
styles = parts["Resources/Styles.xml"].decode("utf-8")
m = re.search(r'<ObjectStyle Self="ObjectStyle/Corners"[^>]*>', styles)
tag = m.group(0)
print("exported style:", " ".join(re.findall(r'\b\w*Corner\w*="[^"]*"', tag)))
for name in parts:
    if name.startswith("Spreads/"):
        for r in re.findall(r'<Rectangle [^>]*>', parts[name].decode("utf-8")):
            print("exported rect: ", " ".join(re.findall(r'\b\w*Corner\w*="[^"]*"', r)))
CORNER = re.compile(r' (?:TopLeft|TopRight|BottomLeft|BottomRight)?Corner(?:Option|Radius)="[^"]*"')
SLOTS = ("TopLeft", "TopRight", "BottomLeft", "BottomRight")
def per_corner(option, radius):
    s = "".join(f' {c}CornerOption="{option}"' for c in SLOTS)
    if radius is not None:
        s += "".join(f' {c}CornerRadius="{radius}"' for c in SLOTS)
    return s
VARIANTS = {
    "1-as-exported": None,
    "2-pair-only": ' CornerOption="RoundedCorner" CornerRadius="20"',
    "3-per-corner-only": per_corner("RoundedCorner", 20),
    "4-disagree": ' CornerOption="InsetCorner" CornerRadius="30"' + per_corner("RoundedCorner", 20),
    "5-neither": "",
    "6-per-corner-option-only": ' CornerRadius="30"' + per_corner("RoundedCorner", None),
}
os.makedirs(os.path.join(out, "variants"), exist_ok=True)
for name, attrs in VARIANTS.items():
    files = dict(parts)
    if attrs is not None:
        bare = CORNER.sub("", tag)
        files["Resources/Styles.xml"] = styles.replace(tag, bare[:-1] + attrs + ">").encode("utf-8")
        for n in parts:
            if n.startswith("Spreads/"):
                files[n] = re.sub(r'<Rectangle [^>]*>', lambda r: CORNER.sub("", r.group(0)),
                                  parts[n].decode("utf-8")).encode("utf-8")
    with zipfile.ZipFile(os.path.join(out, "variants", name + ".idml"), "w") as z:
        z.writestr(zipfile.ZipInfo("mimetype"), files["mimetype"], compress_type=zipfile.ZIP_STORED)
        for n, b in files.items():
            if n != "mimetype":
                z.writestr(n, b, compress_type=zipfile.ZIP_DEFLATED)
PY
run_phase read
rmdir "$LOCK" 2>/dev/null || true
python3 - "$OUT/object-style-corners-read.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for e in d["errors"]:
    print("error:", e)
def fmt(c):
    return "  ".join(f"{k} {v[0]} {v[1] if len(v) > 1 else ''}" for k, v in c.items())
for v in d["variants"]:
    if "error" in v:
        print(f"{v['name']:26} error {v['error']}")
        continue
    print(f"{v['name']:26} style: {fmt(v['style'])}")
    print(f"{'':26} rect ({v['rectStyle']}): {fmt(v['rect'])}")
PY
echo "==> $OUT"
