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

# Ask InDesign what Update Table of Contents writes into an existing TOC
# story; see toc-update-probe.jsx. Opens the IDML given first (default
# corpus/generated/toc.idml); writes toc-update-probe.json, and InDesign's
# IDML and PDF after the update to the third and fourth arguments
# (default temporary files). Pass absolute paths. Takes the InDesign GUI
# for a few seconds; take the shared lock:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/toc-update-probe.sh
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IDML="${1:-$(cd "$HERE/../.." && pwd)/corpus/generated/toc.idml}"
OUT="${2:-$HERE/toc-update-probe.json}"
RESAVE="${3:-$(mktemp -t paged-toc-update).idml}"
PDF="${4:-$(mktemp -t paged-toc-update).pdf}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
SHIM="$(mktemp -t paged-toc-update-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PROBE_IDML = "$IDML";
var PROBE_OUT = "$OUT";
var PROBE_RESAVE = "$RESAVE";
var PROBE_PDF = "$PDF";
\$.evalFile(File("$HERE/toc-update-probe.jsx"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 300 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
echo "==> $OUT (InDesign's IDML: $RESAVE, PDF: $PDF)"
