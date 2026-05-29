#!/usr/bin/env bash
# Fails when the vendored packages/client/src/wasm/idml_canvas_wasm.d.ts
# changes between the merge base and HEAD without a matching bump
# to PROTOCOL_VERSION in crates/idml-canvas/src/channel.rs.
#
# Triggered by .github/workflows/protocol-version.yml after the
# wasm rebuild + `git diff --exit-code` step confirms the vendored
# .d.ts on disk matches what the current Rust source produces.
# This script then asks: did the .d.ts shape change vs the base
# branch? If yes, did PROTOCOL_VERSION change too?
#
# Run locally:
#   BASE_REF=main scripts/check-protocol-version.sh

set -euo pipefail

BASE_REF="${BASE_REF:-origin/main}"
DTS_PATH="packages/client/src/wasm/idml_canvas_wasm.d.ts"
CHANNEL_PATH="crates/idml-canvas/src/channel.rs"

# Skip the check when the .d.ts didn't exist on the base ref — this
# is the one-time bootstrap (vendoring the previously-ignored file).
# Subsequent PRs that change the vendored .d.ts will hit the
# structural-change branch below.
if ! git cat-file -e "$BASE_REF:$DTS_PATH" 2>/dev/null; then
  echo "protocol-version: $DTS_PATH didn't exist on $BASE_REF (bootstrap); skipping"
  exit 0
fi

# Did the .d.ts change in a way that matters? We look only at lines
# that touch type declarations / exports — comments, blank lines,
# and whitespace re-flows don't require a version bump.
TYPE_DIFF=$(git diff "$BASE_REF" -- "$DTS_PATH" \
  | grep -E '^[+-](export|interface|type|class)' || true)

if [ -z "$TYPE_DIFF" ]; then
  echo "protocol-version: .d.ts unchanged structurally; no bump required"
  exit 0
fi

echo "protocol-version: .d.ts changed structurally — checking PROTOCOL_VERSION…"
echo "$TYPE_DIFF" | head -10
echo "…"

# Extract the constant's value at the base ref and at HEAD.
extract_version() {
  local ref="$1"
  git show "$ref:$CHANNEL_PATH" 2>/dev/null \
    | grep -E 'pub const PROTOCOL_VERSION' \
    | grep -oE 'ProtocolVersion\([0-9]+\)' \
    | grep -oE '[0-9]+' \
    | head -1
}

BASE_VERSION=$(extract_version "$BASE_REF" || echo "")
HEAD_VERSION=$(extract_version "HEAD" || echo "")

if [ -z "$BASE_VERSION" ] || [ -z "$HEAD_VERSION" ]; then
  echo "::error::Could not extract PROTOCOL_VERSION from $CHANNEL_PATH"
  echo "  base ($BASE_REF): '$BASE_VERSION'"
  echo "  head: '$HEAD_VERSION'"
  exit 1
fi

if [ "$BASE_VERSION" = "$HEAD_VERSION" ]; then
  echo "::error::Generated .d.ts changed but PROTOCOL_VERSION is still $HEAD_VERSION."
  echo "  Bump PROTOCOL_VERSION in $CHANNEL_PATH whenever a tsify-derived"
  echo "  type's shape changes — the wire contract is versioned, and consumers"
  echo "  rely on the increment to detect schema drift."
  exit 1
fi

echo "protocol-version: PROTOCOL_VERSION bumped $BASE_VERSION → $HEAD_VERSION ✓"
