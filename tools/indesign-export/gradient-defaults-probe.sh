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

# Ask InDesign which conditional text it shows; see conditions-probe.jsx.
# Ask InDesign where it starts a linear gradient that states no
# GradientFillStart, on frames turned every 15 degrees (ADR 147).
#
# Builds a variant of gradient-geometry.idml whose 18 rectangles are 80 x 60
# pt, turned 0, 15, ... 255 degrees, each with GradientFillAngle="0" and no
# start or length; opens it in InDesign and has InDesign write its own IDML
# (through conditions-probe.sh's PROBE_EXPORT_IDML), then reads back the
# GradientFillStart / GradientFillLength InDesign wrote for each. Writes
# gradient-defaults-probe.json beside this script (or to the first argument).
# Takes the InDesign GUI for a few seconds; take the shared lock first:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/gradient-defaults-probe.sh
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${1:-$HERE/gradient-defaults-probe.json}"
WORK="$(mktemp -d -t paged-gradient-defaults)"
trap 'rm -rf "$WORK"' EXIT
SRC="$ROOT/corpus/generated/gradient-geometry.idml"
[ -f "$SRC" ] || { echo "emit corpus/generated/gradient-geometry.idml first (paged-gen emit-all)" >&2; exit 2; }
mkdir "$WORK/src" && (cd "$WORK/src" && unzip -q "$SRC")
python3 -I - "$WORK/src" <<'PY'
import glob, math, re, sys
root = sys.argv[1]
spread = max(glob.glob(root + "/Spreads/*.xml"), key=lambda p: open(p).read().count("<Rectangle "))
s = open(spread).read()
degs = list(range(0, 270, 15))
out, last = [], 0
for deg, m in zip(degs, re.finditer(r"<Rectangle [^>]*>", s)):
    t = math.radians(deg)
    c, si = math.cos(t), math.sin(t)
    tag = m.group(0)
    tx, ty = re.search(r'ItemTransform="([^"]*)"', tag).group(1).split()[4:6]
    tag = re.sub(r'ItemTransform="[^"]*"', 'ItemTransform="%.6f %.6f %.6f %.6f %s %s"' % (c, -si, si, c, tx, ty), tag)
    tag = re.sub(r'GradientFillAngle="[^"]*"', 'GradientFillAngle="0"', tag)
    out += [s[last:m.start()], tag]
    last = m.end()
s = "".join(out) + s[last:]
for a, b in (("0 80", "0 60"), ("80 80", "80 60"), ("120 60", "80 60"), ("120 0", "80 0")):
    s = s.replace('Anchor="%s" LeftDirection="%s" RightDirection="%s"' % (a, a, a),
                  'Anchor="%s" LeftDirection="%s" RightDirection="%s"' % (b, b, b))
open(spread, "w").write(s)
PY
(cd "$WORK/src" && zip -q -X -0 "$WORK/variant.idml" mimetype && zip -q -X -r "$WORK/variant.idml" . -x mimetype)
PROBE_EXPORT_IDML="$WORK/indesign.idml" bash "$HERE/conditions-probe.sh" "$WORK/variant.idml" "$WORK/conditions.json" >/dev/null
mkdir "$WORK/back" && (cd "$WORK/back" && unzip -q "$WORK/indesign.idml")
python3 -I - "$WORK/back" "$OUT" <<'PY'
import glob, json, math, re, sys
root, out = sys.argv[1], sys.argv[2]
spread = max(glob.glob(root + "/Spreads/*.xml"), key=lambda p: open(p).read().count("<Rectangle "))
rows = []
for deg, tag in zip(range(0, 270, 15), re.findall(r"<Rectangle [^>]*>", open(spread).read())):
    attr = lambda k: re.search(k + r'="([^"]*)"', tag).group(1)
    sx, sy = (round(float(v), 2) for v in attr("GradientFillStart").split())
    rows.append({"rotation": deg, "box": [80, 60], "start": [sx, sy],
                 "length": round(float(attr("GradientFillLength")), 2),
                 "angle": float(attr("GradientFillAngle"))})
json.dump({"asked_of": "InDesign 20.0.1", "frames": rows}, open(out, "w"), indent=1)
PY
echo "==> $OUT"
