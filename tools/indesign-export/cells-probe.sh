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

# Ask InDesign where it sets the text of every table cell; see
# cells-probe.jsx. Writes <stem>.cells.json beside the IDML (or to the
# second argument). Takes the InDesign GUI for a few seconds; take the
# shared lock first:
#
#   until mkdir /tmp/paged-indesign.lock 2>/dev/null; do sleep 15; done
#   bash tools/indesign-export/cells-probe.sh corpus/generated/table-cell-baseline.idml
#   rmdir /tmp/paged-indesign.lock
set -euo pipefail
IDML="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
OUT="${2:-${IDML%.idml}.cells.json}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/cells-probe.jsx"
SHIM="$(mktemp -t paged-cells-probe-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
cat > "$SHIM" <<JSX
var PROBE_IDML = "$IDML";
var PROBE_OUT = "$OUT";
\$.evalFile(File("$JSX"));
JSX
# The raw `do script` event code: see run-export.sh for why.
osascript <<OSA
with timeout of 900 seconds
    tell application "$APP"
        «event K2  dosc» (POSIX file "$SHIM") given «class doLg»:«constant ****JSLg»
    end tell
end timeout
OSA
echo "==> $OUT"
