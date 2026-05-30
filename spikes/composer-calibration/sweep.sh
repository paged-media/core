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

# spikes/composer-calibration/sweep.sh
#
# Spike B sweep harness: runs the composer at a grid of penalty
# knobs against a calibration corpus entry and reports which combo
# best matches the InDesign-broken reference lines.
#
# Each corpus entry (corpus/calibration/<name>.json) holds:
#   spec.{font, point_size, column_width_pt, text, penalties}
#   expected_lines[]   — the InDesign-rendered lines in order.
#
# Score per (tolerance, stretch, shrink) combo: % of composer
# lines that match the corresponding expected line after
# whitespace normalisation. The plan's risk register sets ≥ 95%
# line-break parity as the Spike B pass criterion.
#
# Usage:
#   ./spikes/composer-calibration/sweep.sh corpus/calibration/chairman-pullquote.json
#
# Requires: cargo, jq.

set -euo pipefail
CORPUS="${1:-corpus/calibration/chairman-pullquote.json}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
[ -f "$ROOT/$CORPUS" ] || { echo "missing $CORPUS"; exit 1; }
command -v jq >/dev/null || { echo "install jq"; exit 1; }

BIN="$ROOT/target/release/composer-calibration"
[ -x "$BIN" ] || (cd "$ROOT" && cargo build -q --release -p spike-composer-calibration)

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Reference lines, one per file line.
jq -r '.expected_lines[]' "$ROOT/$CORPUS" > "$WORK/expected.txt"
EXPECTED_N=$(wc -l < "$WORK/expected.txt" | tr -d ' ')

best_score=0
best_combo=""
header_printed=0

for tol in 4 6 8 10 16; do
    for stretch in 0.20 0.25 0.33 0.40 0.50 1.00; do
        for shrink in 0.10 0.15 0.20 0.30 0.50; do
            jq --arg t "$tol" --arg s "$stretch" --arg k "$shrink" \
                '{font: .spec.font, point_size: .spec.point_size,
                  column_width_pt: .spec.column_width_pt, text: .spec.text,
                  penalties: {tolerance: ($t | tonumber),
                              stretch_ratio: ($s | tonumber),
                              shrink_ratio: ($k | tonumber)}}' \
                "$ROOT/$CORPUS" > "$WORK/spec.json"
            (cd "$ROOT" && "$BIN" "$WORK/spec.json" --json) > "$WORK/out.json" 2>/dev/null
            LINE_COUNT=$(jq '.line_count' "$WORK/out.json")
            # Reconstruct each composed line's full text by slicing
            # the original paragraph at the composer's byte ranges.
            # The spike's `preview` field is bounded at 72 chars and
            # would silently fail every long line; this sidesteps it.
            jq -r --slurpfile s "$ROOT/$CORPUS" '
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
                  END { printf "%d %d\n", matches, total }' \
                > "$WORK/score.txt"
            read MATCHES TOTAL < "$WORK/score.txt"
            PCT=$(awk -v m="$MATCHES" -v t="$EXPECTED_N" 'BEGIN { if (t==0) print 0; else printf "%.1f", 100.0*m/t }')
            if [ $header_printed -eq 0 ]; then
                printf "%5s %7s %7s   %4s   %5s%%\n" "tol" "stretch" "shrink" "lines" "match"
                printf "%s\n" "------------------------------------------------"
                header_printed=1
            fi
            printf "%5s %7s %7s   %4d   %5s\n" "$tol" "$stretch" "$shrink" "$LINE_COUNT" "$PCT"
            # Compare percentages as integers (×10) so awk doesn't bite.
            PCT_INT=$(awk -v p="$PCT" 'BEGIN { printf "%d", p*10 }')
            BEST_INT=$(awk -v p="$best_score" 'BEGIN { printf "%d", p*10 }')
            if [ "$PCT_INT" -gt "$BEST_INT" ]; then
                best_score=$PCT
                best_combo="tolerance=$tol  stretch=$stretch  shrink=$shrink  lines=$LINE_COUNT"
            fi
        done
    done
done

echo
echo "best: $best_score%  (expected $EXPECTED_N lines)  $best_combo"
