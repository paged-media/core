#!/usr/bin/env bash
# Verify core's font copies against the shared corpus's canonical list.
#
# core is PUBLIC and must build standalone, so it cannot depend on the
# private paged-media/corpus repo — it keeps its own copies of the same
# OFL faces. That duplication is deliberate, and it drifted the first day
# it existed: NotoSansArabic-VF.ttf was added to core alone, so the two
# trees disagreed and nothing said so.
#
# The fix is a manifest, not a dependency. The corpus publishes
# config/fonts.sha256; core commits it verbatim as
# corpus/fonts/CANONICAL.sha256 and checks its own bytes against it. No
# private access needed, and a mismatch fails HERE rather than surfacing
# as a silently substituted font in a fidelity diff — which is exactly
# how this class of problem has bitten before.
#
# When the corpus adds or updates a face: re-run its
# harness/gen-font-hashes.py, copy the file here, and copy the face too
# if core's tests need it.
set -euo pipefail
cd "$(dirname "$0")/.."

MANIFEST="corpus/fonts/CANONICAL.sha256"
[ -f "$MANIFEST" ] || { echo "error: $MANIFEST missing"; exit 2; }

missing=0
mismatch=0
checked=0
while read -r want name; do
    case "$want" in ''|'#'*) continue ;; esac
    f="corpus/fonts/$name"
    # core deliberately carries a SUBSET — absence is fine, wrong bytes
    # are not.
    [ -f "$f" ] || { missing=$((missing + 1)); continue; }
    got=$(shasum -a 256 "$f" | cut -d' ' -f1)
    checked=$((checked + 1))
    if [ "$got" != "$want" ]; then
        echo "::error::$name differs from the canonical corpus copy"
        echo "  canonical $want"
        echo "  here      $got"
        mismatch=$((mismatch + 1))
    fi
done < "$MANIFEST"

echo "fonts: $checked verified against the canonical manifest, $missing not carried here (subset by design)"
[ "$mismatch" -eq 0 ] || { echo "::error::font corpus has drifted from paged-media/corpus"; exit 1; }
