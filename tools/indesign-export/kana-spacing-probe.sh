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

# Ask InDesign how it spaces kana under each kerning method; see
# kana-spacing-probe.jsx. Writes kana-spacing.{idml,pdf,json} into OUT_DIR
# (default: a fresh temporary directory) and prints, per case, where each
# character starts (pt from the page's left) as InDesign's PDF draws it
# (pdf-glyphs.py). Takes the InDesign GUI for a few seconds; it takes the
# shared lock.
#
#   bash tools/indesign-export/kana-spacing-probe.sh [OUT_DIR]
set -euo pipefail
OUT="${1:-$(mktemp -d -t paged-kana-spacing-probe)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SHIM="$(mktemp -t paged-kana-spacing-shim).jsx"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
printf 'var PROBE_OUT = "%s";\n$.evalFile(File("%s"));\n' "$OUT" "$HERE/kana-spacing-probe.jsx" > "$SHIM"
osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
rmdir "$LOCK" 2>/dev/null || true
python3 - "$OUT" "$HERE/pdf-glyphs.py" <<'PY'
import json, subprocess, sys
out, tool = sys.argv[1], sys.argv[2]
probe = json.load(open(f"{out}/kana-spacing.json"))
for e in probe["errors"]:
    if e:
        print("error:", e)
frames = []
for c in probe["cases"]:
    name, _, at = c.partition("@")
    page, _, top = at.partition(":")
    frames.append((int(page) + 1, float(top), name))
glyphs = [json.loads(l) for l in subprocess.run(
    ["python3", tool, f"{out}/kana-spacing.pdf"], capture_output=True, text=True).stdout.splitlines()]
summary = {}
for page, top, name in frames:
    mine = [g for g in glyphs if g["page"] == page and top - 15 <= g["y"] <= top + 60]
    base = [g for g in mine if g["size"] >= 29.9]
    summary[name] = {
        "base": [[g["char"], g["x"], round(g["x"] + g["w"] / 1000 * g["size"] * g["xscale"], 3)] for g in base],
    }
    print(f"{name:20} base " + " ".join(f"{c}{x:.2f}" for c, x, _ in summary[name]["base"]))
json.dump(summary, open(f"{out}/kana-spacing-glyphs.json", "w"), ensure_ascii=False, indent=1)
PY
echo "==> $OUT"
