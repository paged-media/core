#!/usr/bin/env bash
# Ask InDesign where it sets inline and above-line anchored objects; see
# inline-objects-probe.jsx. Writes <stem>.probe.json beside the IDML (or to
# the second argument). Takes the InDesign GUI for a few seconds.
#
#   bash tools/indesign-export/inline-objects-probe.sh corpus/generated/inline-objects.idml
set -euo pipefail
IDML="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
OUT="${2:-${IDML%.idml}.probe.json}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/inline-objects-probe.jsx"
SHIM="$(mktemp -t paged-inline-probe-shim).jsx"
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
