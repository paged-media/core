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

# Ask InDesign which frame a handle scale works in: how a selection's
# transformation bounding box is oriented (one rotated item, several
# sharing a rotation, several with different rotations), how a group of
# the same items is oriented, and what scaling a group along its own axes
# does to its members, and the same for a placed image selected on its
# own (the content of a rotated frame); see scale-frame-probe.jsx. Writes the answers as
# JSON to the first argument (default
# tools/indesign-export/scale-frame-probe.json). Opens a scratch document
# WITH a window (the menu action the probe uses needs one), so it takes
# the screen for a few seconds, and closes it unsaved. It takes the shared
# lock.
#
#   bash tools/indesign-export/scale-frame-probe.sh [out.json]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/scale-frame-probe.json}"
case "$OUT" in /*) ;; *) OUT="$(pwd)/$OUT" ;; esac
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$HERE/scale-frame-probe.jsx"
SHIM="$(mktemp -t paged-scale-frame-probe-shim).jsx"
IMG_DIR="$(mktemp -d -t paged-scale-frame-probe-img)"
IMG="$IMG_DIR/picture-160x60.png"
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rm -rf "$IMG_DIR"; rmdir "$LOCK" 2>/dev/null || true' EXIT
# A 160 x 60 px picture with no resolution chunk: InDesign places it at
# 72 ppi, 160 x 60 pt. Left half dark, right half light, so its
# orientation shows.
python3 -I - "$IMG" <<'PY'
import struct, sys, zlib
w, h = 160, 60
row = b"\x00" + b"".join(bytes([40 if x < w // 2 else 220] * 3) for x in range(w))
rows = row * h
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
PY
cat > "$SHIM" <<JSX
var PROBE_OUT = "$OUT";
var PROBE_IMAGE = "$IMG";
\$.evalFile(File("$JSX"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 300 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
echo "==> $OUT"
