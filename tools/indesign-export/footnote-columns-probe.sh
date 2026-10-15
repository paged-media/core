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

# Ask InDesign how it reads a frame's footnote layout; see
# footnote-columns-probe.jsx.
# Opens the IDML given first; writes footnote-columns-probe.json (and
# InDesign's own IDML of the document to the third argument, default a
# temporary file). Pass absolute paths: InDesign resolves relative ones
# against its own working directory. Takes the InDesign GUI
# for a few seconds; take the shared lock first:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/footnote-columns-probe.sh "$PWD/corpus/generated/footnote-columns.idml"
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IDML="${1:?the IDML to open}"
OUT="${2:-$HERE/footnote-columns-probe.json}"
RESAVE="${3:-$(mktemp -t paged-footnote-columns).idml}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
SHIM="$(mktemp -t paged-footnote-columns-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PROBE_IDML = "$IDML";
var PROBE_OUT = "$OUT";
var PROBE_RESAVE = "$RESAVE";
\$.evalFile(File("$HERE/footnote-columns-probe.jsx"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 300 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
echo "==> $OUT (InDesign's IDML: $RESAVE)"
