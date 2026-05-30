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

# spikes/composer-calibration/sweep-corpus.sh
#
# Multi-entry calibration sweep. Walks every JSON in
# corpus/calibration/, runs the composer at a grid of
# (tolerance, stretch, shrink) and reports per-entry +
# corpus-wide line-break parity. The plan's risk register
# sets ≥ 95% line-break parity as the Spike B pass criterion.
#
# Usage:
#   ./spikes/composer-calibration/sweep-corpus.sh
#   ./spikes/composer-calibration/sweep-corpus.sh --best   # only show best combo + per-entry breakdown
#
# Requires: cargo, jq, python3.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CORPUS_DIR="$ROOT/corpus/calibration"
[ -d "$CORPUS_DIR" ] || { echo "missing $CORPUS_DIR"; exit 1; }
command -v jq >/dev/null || { echo "install jq"; exit 1; }
command -v python3 >/dev/null || { echo "install python3"; exit 1; }

BIN="$ROOT/target/release/composer-calibration"
[ -x "$BIN" ] || (cd "$ROOT" && cargo build -q --release -p spike-composer-calibration)

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

ENTRIES=( "$CORPUS_DIR"/*.json )

# Score one entry against given penalties. Echoes "matches total".
score_entry() {
    local entry="$1" tol="$2" stretch="$3" shrink="$4"
    jq --arg t "$tol" --arg s "$stretch" --arg k "$shrink" \
        '{font: .spec.font, point_size: .spec.point_size,
          column_width_pt: .spec.column_width_pt, text: .spec.text,
          penalties: {tolerance: ($t | tonumber),
                      stretch_ratio: ($s | tonumber),
                      shrink_ratio: ($k | tonumber)}}' \
        "$entry" > "$WORK/spec.json"
    (cd "$ROOT" && "$BIN" "$WORK/spec.json" --json) > "$WORK/out.json" 2>/dev/null
    jq -r '.expected_lines[]' "$entry" > "$WORK/expected.txt"
    jq -r --slurpfile s "$entry" '
        . as $out
        | $s[0].spec.text as $text
        | $out.lines[]
        | $text[.start:.end]' "$WORK/out.json" > "$WORK/got.txt"
    paste "$WORK/expected.txt" "$WORK/got.txt" \
        | awk -F$'\t' 'BEGIN { matches = 0; total = 0 }
          {
              a = $1; b = $2;
              gsub(/[[:space:]]+/, " ", a); gsub(/[[:space:]]+/, " ", b);
              sub(/^ /, "", a); sub(/ $/, "", a);
              sub(/^ /, "", b); sub(/ $/, "", b);
              total += 1;
              if (a == b) matches += 1;
          }
          END { printf "%d %d\n", matches, total }'
}

MODE="${1:-grid}"

if [ "$MODE" = "--best" ] || [ "$MODE" = "--report" ]; then
    # Just report parity at current defaults.
    # Pull defaults from the source so this stays in sync. Match
    # `tolerance: 8.0,` etc. inside `pub fn new(...) -> Self`.
    DEFAULTS_BLOCK=$(awk '
        /pub fn new\(column_width_pt: f32\) -> Self/ { in_fn=1 }
        in_fn { print }
        in_fn && /^    \}$/ { exit }
    ' "$ROOT/crates/paged-text/src/compose.rs")
    TOL=$(printf '%s\n' "$DEFAULTS_BLOCK" | grep -E '^\s*tolerance:' | grep -oE '[0-9]+\.[0-9]+|[0-9]+' | head -1)
    STR=$(printf '%s\n' "$DEFAULTS_BLOCK" | grep -E '^\s*stretch_ratio:' | grep -oE '[0-9]+\.[0-9]+|[0-9]+' | head -1)
    SHR=$(printf '%s\n' "$DEFAULTS_BLOCK" | grep -E '^\s*shrink_ratio:' | grep -oE '[0-9]+\.[0-9]+|[0-9]+' | head -1)
    echo "Reporting parity at composer defaults: tol=$TOL stretch=$STR shrink=$SHR"
    echo
    printf "%-30s %s\n" "entry" "match"
    printf "%s\n" "----------------------------------------"
    TOTAL_M=0
    TOTAL_T=0
    for entry in "${ENTRIES[@]}"; do
        name=$(basename "$entry" .json)
        read M T <<< "$(score_entry "$entry" "$TOL" "$STR" "$SHR")"
        printf "%-30s %d/%d\n" "$name" "$M" "$T"
        TOTAL_M=$((TOTAL_M + M))
        TOTAL_T=$((TOTAL_T + T))
    done
    PCT=$(awk -v m=$TOTAL_M -v t=$TOTAL_T 'BEGIN { if (t==0) print 0; else printf "%.1f", 100.0*m/t }')
    echo
    echo "corpus total: $TOTAL_M/$TOTAL_T = $PCT%"
    exit 0
fi

# Grid sweep.
best_score=0
best_combo=""
header_printed=0

for tol in 4 6 8 10 16; do
    for stretch in 0.20 0.25 0.33 0.40 0.50 1.00; do
        for shrink in 0.10 0.15 0.20 0.30 0.50; do
            TOTAL_M=0
            TOTAL_T=0
            PER_ENTRY=""
            for entry in "${ENTRIES[@]}"; do
                read M T <<< "$(score_entry "$entry" "$tol" "$stretch" "$shrink")"
                TOTAL_M=$((TOTAL_M + M))
                TOTAL_T=$((TOTAL_T + T))
                PER_ENTRY="$PER_ENTRY $M/$T"
            done
            PCT=$(awk -v m=$TOTAL_M -v t=$TOTAL_T 'BEGIN { if (t==0) print 0; else printf "%.1f", 100.0*m/t }')
            if [ $header_printed -eq 0 ]; then
                printf "%5s %7s %7s   %5s%%\n" "tol" "stretch" "shrink" "match"
                printf "%s\n" "----------------------------------------"
                header_printed=1
            fi
            printf "%5s %7s %7s   %5s\n" "$tol" "$stretch" "$shrink" "$PCT"
            PCT_INT=$(awk -v p="$PCT" 'BEGIN { printf "%d", p*10 }')
            BEST_INT=$(awk -v p="$best_score" 'BEGIN { printf "%d", p*10 }')
            if [ "$PCT_INT" -gt "$BEST_INT" ]; then
                best_score=$PCT
                best_combo="tolerance=$tol stretch=$stretch shrink=$shrink"
            fi
        done
    done
done

echo
echo "best: $best_score%   $best_combo"
