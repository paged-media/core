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

# Ask InDesign how an object style's text-wrap, anchored-object and
# frame-fitting options reach the items that apply it; see
# object-style-options-probe.jsx. Writes object-style-options-probe.json (or
# the first argument): InDesign's answers through its DOM, plus what its own
# IDML export writes on each item and style. The second argument, if given,
# is an IDML to open and read back (corpus/generated/object-style-options.idml).
# Takes the InDesign GUI for a few seconds; take the shared lock first:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/object-style-options-probe.sh \
#     "" corpus/generated/object-style-options.idml
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-}"
OUT="${OUT:-$HERE/object-style-options-probe.json}"
case "$OUT" in /*) ;; *) OUT="$(pwd)/$OUT" ;; esac
IDML="${2:-}"
if [ -n "$IDML" ]; then
    case "$IDML" in /*) ;; *) IDML="$(pwd)/$IDML" ;; esac
fi
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
WORK="$(mktemp -d -t paged-object-style-options)"
SHIM="$WORK/shim.jsx"
RESAVE="$WORK/authored.idml"
DOM="$WORK/dom.json"
trap 'rm -rf "$WORK"' EXIT
cat > "$SHIM" <<JSX
var PROBE_OUT = "$DOM";
var PROBE_RESAVE = "$RESAVE";
var PROBE_IDML = "$IDML";
\$.evalFile(File("$HERE/object-style-options-probe.jsx"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 300 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
# What InDesign's own IDML writes: each named item's wrap, anchored-object
# and fitting elements, and each object style's.
python3 -I - "$DOM" "$RESAVE" "$OUT" <<'PY'
import json, sys, zipfile
import xml.etree.ElementTree as ET

dom_path, idml_path, out_path = sys.argv[1:4]
with open(dom_path, encoding="utf-8") as f:
    dom = json.load(f)
TAGS = ("TextWrapPreference", "AnchoredObjectSetting", "FrameFittingOption")


def spell(el):
    s = "<" + el.tag + "".join(f' {k}="{v}"' for k, v in sorted(el.attrib.items()))
    kids = [spell(c) for c in el if c.tag != "Properties"] + [
        spell(g) for c in el if c.tag == "Properties" for g in c
    ]
    return s + (">" + "".join(kids) + "</" + el.tag + ">" if kids else "/>")


def options(el):
    return {c.tag: spell(c) for c in el if c.tag in TAGS}


items, styles = {}, {}
with zipfile.ZipFile(idml_path) as z:
    for name in z.namelist():
        if not name.endswith(".xml"):
            continue
        root = ET.fromstring(z.read(name))
        for el in root.iter():
            if el.tag == "ObjectStyle":
                flags = {k: v for k, v in el.attrib.items() if k in (
                    "EnableTextWrapAndOthers", "EnableAnchoredObjectOptions",
                    "EnableFrameFittingOptions")}
                styles[el.get("Name")] = {"enable": flags, **options(el)}
            elif el.get("Name") and el.get("Name") != "$ID/":
                if el.tag in ("Rectangle", "TextFrame", "Oval", "Polygon"):
                    items[el.get("Name")] = {
                        "AppliedObjectStyle": el.get("AppliedObjectStyle"), **options(el)}
dom["exported_items"] = items
dom["exported_styles"] = {k: v for k, v in styles.items()
                          if k in ("W BBox", "W Child", "A Right", "F Fill", "$ID/[None]")}
with open(out_path, "w", encoding="utf-8") as f:
    json.dump(dom, f, indent=2, ensure_ascii=False)
    f.write("\n")
PY
echo "==> $OUT"
