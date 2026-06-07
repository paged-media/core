#!/usr/bin/env bash
# scripts/verify-lane.sh — run ONE verify lane and print its scoreboard row.
#
# Usage: verify-lane.sh <lane-name> -- <command...>
#        verify-lane.sh <lane-name> --skip "<reason>"
#
# Runs the command, captures pass/fail, and appends a single
# PASS/FAIL/SKIP row to the scoreboard file named by $VERIFY_SCOREBOARD
# (a temp file the Makefile creates and prints at the end). The lane's
# own output streams live to the terminal so a failing lane is
# debuggable; the row is the one-line summary.
#
# Exit status mirrors the lane: 0 on PASS/SKIP, nonzero on FAIL — so the
# Makefile recipe (which `-` ignores per-lane failure to keep the table
# whole) can still tell `make verify` to exit nonzero overall via the
# scoreboard scan in `report`.
#
# Plain Make + bash, no new tooling: this is the only shared helper.

set -uo pipefail

lane="$1"; shift
board="${VERIFY_SCOREBOARD:?VERIFY_SCOREBOARD must be set by the Makefile}"

row() { printf '%s\t%s\t%s\n' "$1" "$lane" "${2:-}" >> "$board"; }

if [ "${1:-}" = "--skip" ]; then
  reason="${2:-no reason given}"
  printf '\n=== lane: %s ===\nSKIP: %s\n' "$lane" "$reason"
  row "SKIP" "$reason"
  exit 0
fi

if [ "${1:-}" = "--" ]; then shift; fi

printf '\n=== lane: %s ===\n+ %s\n' "$lane" "$*"
if "$@"; then
  row "PASS"
  printf 'PASS: %s\n' "$lane"
  exit 0
else
  rc=$?
  row "FAIL" "exit $rc"
  printf 'FAIL: %s (exit %s)\n' "$lane" "$rc"
  exit "$rc"
fi
