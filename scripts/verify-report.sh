#!/usr/bin/env bash
# scripts/verify-report.sh — render the verify scoreboard + set exit code.
#
# Reads the tab-separated rows written by verify-lane.sh (RESULT\tLANE\tNOTE)
# from $VERIFY_SCOREBOARD, prints a fixed-width table, and exits:
#   1  if ANY lane is FAIL   (the gate is red)
#   0  otherwise             (all PASS, SKIPs allowed)
#
# SKIP never fails the gate — a SKIP means "not runnable here" (e.g. the
# fidelity gate without its corpus/poppler deps), which is reported
# honestly but is not a regression.

set -uo pipefail

board="${VERIFY_SCOREBOARD:?VERIFY_SCOREBOARD must be set}"

echo
echo "========================= make verify ========================="
printf '%-7s  %-14s  %s\n' "RESULT" "LANE" "NOTE"
printf '%-7s  %-14s  %s\n' "------" "----" "----"

fails=0
if [ -s "$board" ]; then
  while IFS=$'\t' read -r result lane note; do
    [ -n "$result" ] || continue
    printf '%-7s  %-14s  %s\n' "$result" "$lane" "$note"
    [ "$result" = "FAIL" ] && fails=$((fails + 1))
  done < "$board"
else
  echo "(no lanes ran)"
fi
echo "==============================================================="

if [ "$fails" -gt 0 ]; then
  echo "verify: FAIL — $fails lane(s) red."
  exit 1
fi
echo "verify: green — no FAIL lanes (SKIPs are honest, not failures)."
exit 0
