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

# Ask InDesign how many table rows a frame holds as its height nears the rows' sum; see table-fit-probe.jsx.
# With PROBE_IN=<absolute .idml> in the environment it opens that document
# and reports where each row of its tables stands instead of building one.
# Writes table-fit-probe.json, and InDesign's IDML and PDF of the document to
# the second and third arguments (default temporary files). Pass absolute
# paths. Takes the InDesign GUI for a few seconds; take the shared lock:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/table-fit-probe.sh
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/table-fit-probe.json}"
IDML="${2:-$(mktemp -t paged-table-fit).idml}"
PDF="${3:-$(mktemp -t paged-table-fit).pdf}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
SHIM="$(mktemp -t paged-table-fit-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PROBE_IN = "${PROBE_IN:-}";
var PROBE_OUT = "$OUT";
var PROBE_IDML = "$IDML";
var PROBE_PDF = "$PDF";
\$.evalFile(File("$HERE/table-fit-probe.jsx"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 300 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
echo "==> $OUT (InDesign's IDML: $IDML, PDF: $PDF)"
