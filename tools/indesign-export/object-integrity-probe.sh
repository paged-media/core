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

# Ask InDesign what a script may do to a locked page item, what deleting
# a container does to its pasted-in content, and what deleting a group's
# members leaves behind; see object-integrity-probe.jsx. Writes the
# answers as JSON to the first argument (default
# tools/indesign-export/object-integrity-probe.json). Opens a windowless
# scratch document and closes it unsaved.
#
#   bash tools/indesign-export/object-integrity-probe.sh [out.json]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/object-integrity-probe.json}"
case "$OUT" in /*) ;; *) OUT="$(pwd)/$OUT" ;; esac
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$HERE/object-integrity-probe.jsx"
SHIM="$(mktemp -t paged-object-probe-shim).jsx"
trap 'rm -f "$SHIM"' EXIT
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
