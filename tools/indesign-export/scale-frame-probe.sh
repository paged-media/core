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
# does to its members; see scale-frame-probe.jsx. Writes the answers as
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
LOCK=/tmp/paged-indesign.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 15; done
trap 'rm -f "$SHIM"; rmdir "$LOCK" 2>/dev/null || true' EXIT
cat > "$SHIM" <<JSX
var PROBE_OUT = "$OUT";
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
