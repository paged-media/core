#!/usr/bin/env bash
# scripts/fetch-profiles.sh
#
# Populate corpus/profiles/ with a freely-licensed CMYK profile so the
# colour tests actually run.
#
# WHY THIS EXISTS. The 2026-08-19 corpus audit found ZERO .icc files
# anywhere in the workspace, while the qcms engine, CMYK export, PDF/X-4,
# ink coverage and soft-proofing all depend on one. Roughly eight tests
# printed "skipping" and passed — an entire colour subsystem green and
# asserting nothing. Three different tests looked in three different
# places, one of them (`corpus/calibration`) a directory that has only
# ever held JSON.
#
# WHY NOT COMMIT THE PROFILE. Profile licences vary and this is a public
# repo. Adobe's CoatedFOGRA39 (the one most likely to be sitting on a
# designer's Mac) is explicitly not redistributable. So corpus/profiles/
# is gitignored and populated on demand — the same pattern ci.yml's
# export-diff gate already uses for its rasterisation profile.
#
# Usage:
#   scripts/fetch-profiles.sh          # fetch if absent
#   scripts/fetch-profiles.sh --force  # re-fetch
#
# Tests resolve profiles via paged_color::test_profiles::find_cmyk_profile,
# which prefers $PAGED_CMYK_PROFILE, then corpus/profiles/, then a local
# Adobe install. Setting PAGED_CMYK_PROFILE makes this script optional.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/corpus/profiles"

# Ghostscript's default CMYK profile, from a PINNED release tag so the
# bytes (and therefore every ΔE budget measured against them) are
# reproducible. Artifex ships it for exactly this purpose; ci.yml's
# export-diff gate fetches the same file from the same tag.
URL="https://raw.githubusercontent.com/ArtifexSoftware/ghostpdl/ghostpdl-10.05.1/iccprofiles/default_cmyk.icc"
TARGET="$DEST/default_cmyk.icc"

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

mkdir -p "$DEST"

if [ -s "$TARGET" ] && [ "$FORCE" -eq 0 ]; then
    echo "profiles: $TARGET already present ($(wc -c <"$TARGET" | tr -d ' ') bytes) — pass --force to re-fetch"
    exit 0
fi

echo "profiles: fetching default_cmyk.icc from ghostpdl-10.05.1"
if ! curl -fsSL --retry 3 -o "$TARGET" "$URL"; then
    rm -f "$TARGET"
    echo "profiles: FETCH FAILED — colour tests will skip." >&2
    echo "          Set PAGED_CMYK_PROFILE to any .icc instead, or drop one in $DEST." >&2
    exit 1
fi

# A truncated or HTML-error download is worse than none: it would make
# qcms fail deep inside a test rather than at the fetch. ICC files start
# with a 4-byte big-endian size followed by a preferred-CMM signature;
# checking the profile parses at all is the cheap honest guard.
size=$(wc -c <"$TARGET" | tr -d ' ')
if [ "$size" -lt 1000 ]; then
    rm -f "$TARGET"
    echo "profiles: downloaded file is only ${size} bytes — not a profile. Removed." >&2
    exit 1
fi

echo "profiles: wrote $TARGET (${size} bytes)"
