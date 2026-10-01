#!/usr/bin/env bash
# Ask InDesign how Smart Text Reflow grows (or shrinks) a document; see
# reflow-probe.jsx. Writes <stem>.reflow.json + <stem>.reflow.pdf beside the
# IDML. Takes the InDesign GUI for a few seconds.
#
#   bash tools/indesign-export/reflow-probe.sh corpus/generated/reflow.idml
#   PAGED_REFLOW_LIMIT=true PAGED_REFLOW_EDIT=shrink bash tools/indesign-export/reflow-probe.sh <idml>
set -euo pipefail
IDML="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
STEM="${IDML%.idml}"
SUFFIX="${PAGED_REFLOW_SUFFIX:-reflow}"
APP="${INDESIGN_APP:-Adobe InDesign 2025}"
JSX="$(cd "$(dirname "$0")" && pwd)/reflow-probe.jsx"
SHIM="$(mktemp -t paged-reflow-shim).jsx"
trap 'rm -f "$SHIM"' EXIT

# Smart Text Reflow runs on InDesign's IDLE time, so the edit and the
# measurement are two scripts with a pause between them.
run_phase () {
    cat > "$SHIM" <<JSX
var PAGED_REFLOW_IDML = "$IDML";
var PAGED_REFLOW_JSON = "$STEM.$SUFFIX.json";
var PAGED_REFLOW_PDF = "$STEM.$SUFFIX.pdf";
var PAGED_REFLOW_LIMIT = "${PAGED_REFLOW_LIMIT:-false}";
var PAGED_REFLOW_EDIT = "${PAGED_REFLOW_EDIT:-grow}";
var PAGED_REFLOW_VARIANT = "${PAGED_REFLOW_VARIANT:-}";
var PAGED_REFLOW_PHASE = "$1";
\$.evalFile(File("$JSX"));
JSX
    osascript <<OSA
with timeout of 300 seconds
    tell application "$APP"
        activate
        do script POSIX file "$SHIM" language javascript
    end tell
end timeout
OSA
}

run_phase prepare
sleep "${PAGED_REFLOW_IDLE_S:-4}"
run_phase report
echo "==> $STEM.$SUFFIX.json"
