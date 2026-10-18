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

# Ask InDesign what a live caption prints
# see caption-probe.jsx.
# Writes caption-probe.json, and InDesign's IDML and PDF of the document to
# the second and third arguments (default temporary files). Pass absolute
# paths. Takes the InDesign GUI for a few seconds; take the shared lock:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/caption-probe.sh
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/caption-probe.json}"
IDML="${2:-$(mktemp -t paged-caption).idml}"
PDF="${3:-$(mktemp -t paged-caption).pdf}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
SHIM="$(mktemp -t paged-caption-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PROBE_OUT = "$OUT";
var PROBE_IDML = "$IDML";
var PROBE_PDF = "$PDF";
var PROBE_PDF_RELINKED = "${PROBE_PDF_RELINKED:-}";
var PROBE_IMAGE_A = "${PROBE_IMAGE_A:?absolute path}";
var PROBE_IMAGE_B = "${PROBE_IMAGE_B:?absolute path}";
\$.evalFile(File("$HERE/caption-probe.jsx"));
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
