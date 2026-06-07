#!/usr/bin/env bash
# scripts/rust-channel.sh — print the pinned Rust channel from rust-toolchain.toml.
#
# Single source of truth for the toolchain version (audit B21): every CI
# workflow reads THIS instead of hardcoding `@stable`, so the pin in
# rust-toolchain.toml (`channel = "1.94.1"`) is honoured uniformly and a
# bump is a one-line change in one file.
#
# Emits just the channel string (e.g. `1.94.1`) on stdout. In a workflow:
#   echo "channel=$(scripts/rust-channel.sh)" >> "$GITHUB_OUTPUT"
# then feed ${{ steps.<id>.outputs.channel }} to dtolnay/rust-toolchain's
# `toolchain:` input.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FILE="$ROOT/rust-toolchain.toml"

[ -f "$FILE" ] || { echo "rust-toolchain.toml not found at $FILE" >&2; exit 1; }

# Parse the `channel = "X"` line under [toolchain]. Plain grep/sed — no
# toml parser dependency in CI. Tolerates single or double quotes and
# surrounding whitespace.
channel="$(
  grep -E '^[[:space:]]*channel[[:space:]]*=' "$FILE" \
    | head -n1 \
    | sed -E 's/^[^=]*=[[:space:]]*["'\'']?([^"'\'' ]+)["'\'']?.*/\1/'
)"

[ -n "$channel" ] || { echo "could not parse channel from $FILE" >&2; exit 1; }
printf '%s\n' "$channel"
