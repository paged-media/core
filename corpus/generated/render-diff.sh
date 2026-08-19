#!/usr/bin/env bash
# corpus/generated/render-diff.sh — the fidelity ENGINE.
#
# Render one IDML through paged-inspect and ΔE-diff every page against
# its InDesign-exported reference PDF.
#
# This script used to live in the PRIVATE paged-media/corpus repo, and
# `diff.sh` (the gate) skipped itself whenever it was absent. A clean
# public checkout is exactly that case, so core's own CI printed
#
#     ==> fidelity engine .../corpus/samples/diff.sh not present
#     ==> skipping the generated-fidelity gate in this environment
#
# on BOTH runners and reported success — a step literally named
# "generated-corpus fidelity gate (hard)" that had never gated anything
# in the repo it guards. Nothing in here is private: it wires
# paged-inspect, pdftoppm and paged-diff. Only the ENVATO ASSETS are
# licensed, and those live on in the corpus repo. So the engine moves
# here, next to the license-clear fixtures it runs against.
#
# Outputs:
#
#   /tmp/paged-diff/cand-NNN.png    candidate (our render)
#   /tmp/paged-diff/ref-NNN.png     reference (rasterised PDF)
#   /tmp/paged-diff/heat-NNN.png    per-page heatmap (only on misses)
#   /tmp/paged-diff/report.json     machine-readable per-page summary
#
# Usage: ./corpus/generated/render-diff.sh [<idml-name>]
# Defaults to "sample". Resolves <name>.{idml,pdf} against
# corpus/generated/ first, then corpus/samples/ (present only when the
# private corpus is checked out alongside).

set -euo pipefail

NAME="${1:-sample}"
# Anchor on the repo root by walking UP for Cargo.toml rather than
# counting `../..` — the count is what silently broke when the monorepo
# was split (every path resolved to a directory that did not exist).
ROOT="$(cd "$(dirname "$0")" && pwd)"
while [ ! -f "$ROOT/Cargo.toml" ] && [ "$ROOT" != "/" ]; do ROOT="$(dirname "$ROOT")"; done
[ -f "$ROOT/Cargo.toml" ] || { echo "error: no Cargo.toml above $0 — run me from inside the core checkout" >&2; exit 2; }
SAMPLE_DIR="$ROOT/corpus/samples"
GENERATED_DIR="$ROOT/corpus/generated"
# Resolve the IDML/PDF pair against either the curated samples
# directory or the generated mega-files directory. Generated samples
# (emitted by `cargo run -p paged-gen -- emit`) take precedence so a
# generator-produced fixture can shadow a hand-curated one with the
# same name during development.
if [ -f "$GENERATED_DIR/$NAME.idml" ]; then
    SAMPLE_DIR="$GENERATED_DIR"
fi
IDML="$SAMPLE_DIR/$NAME.idml"
PDF="$SAMPLE_DIR/$NAME.pdf"
OUT="${IDML_DIFF_OUT:-/tmp/paged-diff}"
DPI="${IDML_DIFF_DPI:-144}"
FONTS="$ROOT/corpus/fonts"

[ -f "$IDML" ] || { echo "missing $IDML"; exit 1; }
# A reference PDF is optional — generated samples emitted by
# `cargo run -p paged-gen` have no InDesign-exported reference yet, so
# we still render the IDML and skip the per-page ΔE diff downstream.
HAVE_PDF=1
if [ ! -f "$PDF" ]; then
    echo "==> no reference PDF at $PDF — rendering IDML only, skipping ref rasterisation"
    HAVE_PDF=0
fi
if [ "$HAVE_PDF" -eq 1 ]; then
    command -v pdftoppm >/dev/null || { echo "install poppler (pdftoppm)"; exit 1; }
fi

rm -rf "$OUT" && mkdir -p "$OUT"

echo "==> render IDML through paged-inspect → $OUT"
# Per-sample optional Links/ folder (e.g. corpus/samples/<name>-Links/)
# resolved into the renderer; harmless if it doesn't exist.
LINKS_FLAG=""
if [ -d "$SAMPLE_DIR/$NAME-Links" ]; then
    LINKS_FLAG="--links-dir $SAMPLE_DIR/$NAME-Links"
fi

# Per-sample font mapping. The default registrations below cover
# sample.idml's chairman + body content (serif Minion Pro mapped to
# Cormorant Garamond, sans-serif Open Sans for headers). Other samples
# (Sample-3 uses sans-serif Myriad Pro everywhere; InDesign substitutes
# Minion Pro with Myriad-like glyphs at PDF export) can override the
# defaults by dropping a `$NAME.fonts.sh` next to the IDML — that file
# sets the FONT_FLAGS array verbatim before we hand it to inspect.
DEFAULT_FONT="$FONTS/SourceSerif4.ttf"
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
if [ -f "$SAMPLE_DIR/$NAME.fonts.sh" ]; then
    # shellcheck disable=SC1090
    . "$SAMPLE_DIR/$NAME.fonts.sh"
fi

# Synthetic generator fixtures under corpus/generated/ were exported
# without InDesign's missing-image placeholder visible (the fixtures
# test geometry/effects, not broken-link visuals). Suppress the
# renderer's placeholder for those; real-world packs keep it on so
# template scaffolding (broken-link "Your Image Here" frames) match
# their reference PDFs.
PLACEHOLDER_FLAG=""
if [ "$SAMPLE_DIR" = "$GENERATED_DIR" ]; then
    PLACEHOLDER_FLAG="--no-missing-image-placeholder"
fi

(cd "$ROOT" && cargo run -q --release -p paged-renderer --bin paged-inspect -- \
    "$IDML" \
    --render "$OUT/cand.png" \
    --default-font "$DEFAULT_FONT" \
    "${FONT_FLAGS[@]}" \
    $LINKS_FLAG \
    $PLACEHOLDER_FLAG \
    --dpi "$DPI" >/dev/null)

if [ "$HAVE_PDF" -eq 1 ]; then
    # Match pdftoppm's CMYK profile to whatever our renderer uses
    # (FOGRA39 by default — see crates/paged-renderer/src/bin/inspect.rs's
    # resolve_cmyk_profile_by_name + crates/paged-color/src/lib.rs).
    # Without this, pdftoppm's poppler-baked default is U.S. Web
    # Coated SWOP, which produces ~(35,31,32) sRGB for K=100; our
    # renderer with Adobe FOGRA39 produces ~(29,29,27); the
    # ~4 ΔE delta is entirely the CMYK profile mismatch and adds
    # to every solid-CMYK fill across the corpus. Forcing both
    # paths to FOGRA39 makes them apples-to-apples.
    # BOTH SIDES MUST USE THE SAME PROFILE, and the profile is not a
    # free choice: paged-inspect resolves the document's declared CMYK
    # profile and falls back to CoatedFOGRA39 for InDesign's "$ID/"
    # sentinel (crates/paged-renderer/src/bin/inspect.rs
    # resolve_cmyk_profile_by_name, whose own comment names this
    # harness). pdftoppm therefore has to be FOGRA39 too. Poppler's
    # baked-in default is U.S. Web Coated SWOP, which renders K=100 at
    # ~(35,31,32) sRGB against our ~(29,29,27) — a ~4 dE offset added to
    # every solid CMYK fill on every page.
    #
    # That signature is unmistakable once seen: a *uniform* p99 of ~4.16
    # across every page of every fixture. Do not chase it in the
    # renderer; check this first.
    PDFTOPPM_CMYK_FLAGS=()
    FOGRA39="/Library/Application Support/Adobe/Color/Profiles/Recommended/CoatedFOGRA39.icc"
    CMYK_ICC="${PAGED_CMYK_PROFILE:-}"
    if [ -n "$CMYK_ICC" ]; then
        :                                   # explicit override wins
    elif [ -f "$FOGRA39" ]; then
        CMYK_ICC="$FOGRA39"
    fi
    if [ -n "$CMYK_ICC" ] && [ -f "$CMYK_ICC" ]; then
        PDFTOPPM_CMYK_FLAGS=(-defaultcmykprofile "$CMYK_ICC")
        case "$CMYK_ICC" in
            *CoatedFOGRA39.icc) ;;          # matches the renderer default
            *) echo "==> WARNING: rasterising with $CMYK_ICC, not CoatedFOGRA39." >&2
               echo "==> The renderer defaults to FOGRA39, so every solid CMYK fill will read high." >&2 ;;
        esac
    else
        echo "==> WARNING: no CoatedFOGRA39.icc — pdftoppm falls back to poppler's SWOP default" >&2
        echo "==> while the renderer falls back to naive CMYK math. Two different colour" >&2
        echo "==> spaces: expect a uniform ~4 dE p99 on every page of every fixture." >&2
        echo "==> Set PAGED_CMYK_PROFILE to a FOGRA39 profile to compare like with like." >&2
        # Leave a marker so the GATE can tell "regressed" from "could not
        # reproduce the reference colour space". Those are different
        # answers and must not share an exit code.
        : > "$OUT/.no-cmyk-profile"
    fi
    echo "==> rasterise $PDF via pdftoppm at $DPI dpi"
    # `${arr[@]}` on an EMPTY array trips `set -u` on bash 3.2, which is
    # what macOS ships — so the no-profile path died with "unbound
    # variable" on macOS while working on ubuntu's bash 5. The `+` form
    # expands to nothing when unset instead of erroring.
    pdftoppm ${PDFTOPPM_CMYK_FLAGS[@]+"${PDFTOPPM_CMYK_FLAGS[@]}"} \
        -r "$DPI" -png "$PDF" "$OUT/ref" >/dev/null
    # pdftoppm uses the smallest sufficient zero-padding (2 digits for
    # 48 pages). paged-inspect always pads to 3. Normalise both to 3 so
    # the per-page loop below can pair them by integer page number.
    for f in "$OUT"/ref-*.png; do
        base=${f##*/}
        raw=${base#ref-}; raw=${raw%.png}
        # Strip leading zeros without breaking the value.
        n=$((10#$raw))
        new=$(printf "$OUT/ref-%03d.png" "$n")
        [ "$f" = "$new" ] || mv "$f" "$new"
    done
else
    echo "==> skipping reference rasterisation (no PDF)"
fi

REPORT="$OUT/report.json"
total_pages=0
pass_pages=0
shopt -s nullglob

if [ "$HAVE_PDF" -eq 1 ]; then
    echo "==> per-page ΔE diff"
    DIFF="$ROOT/target/release/paged-diff"
    [ -x "$DIFF" ] || (cd "$ROOT" && cargo build -q --release -p paged-fidelity --bin paged-diff)

    echo "[" > "$REPORT"
    first=1

    for cand in "$OUT"/cand-*.png; do
        page="${cand##*-}"; page="${page%.png}"
        ref="$OUT/ref-$page.png"
        if [ ! -f "$ref" ]; then
            echo "  page $page: no reference PNG (PDF page count mismatch?)"
            continue
        fi
        total_pages=$((total_pages + 1))
        line=$("$DIFF" "$ref" "$cand" --json --heatmap "$OUT/heat-$page.png" || true)
        pass=$(echo "$line" | grep -oE '"passes":(true|false)' | sed 's/.*://')
        mean=$(echo "$line" | grep -oE '"mean_de":[0-9.]+' | sed 's/.*://')
        p99=$(echo "$line" | grep -oE '"p99_de":[0-9.]+' | sed 's/.*://')
        ssim=$(echo "$line" | grep -oE '"ssim":[0-9.]+' | sed 's/.*://')
        [ "$pass" = "true" ] && pass_pages=$((pass_pages + 1))
        printf "  page %s  meanΔE=%6.3f  p99ΔE=%6.3f  ssim=%5.3f  %s\n" \
            "$page" "$mean" "$p99" "$ssim" "$pass"
        if [ $first -eq 0 ]; then echo "," >> "$REPORT"; fi
        first=0
        printf '  {"page":%s,%s}' "$((10#$page))" "${line:1:${#line}-2}" >> "$REPORT"
    done
    echo "" >> "$REPORT"
    echo "]" >> "$REPORT"

    echo
    echo "summary: $pass_pages/$total_pages pages pass §13.2 thresholds"
    echo "report: $REPORT"
else
    # No reference PDF — emit an empty report and just count
    # candidate renders so the harness output stays uniform.
    echo "[]" > "$REPORT"
    for cand in "$OUT"/cand-*.png; do
        total_pages=$((total_pages + 1))
        page="${cand##*-}"; page="${page%.png}"
        printf "  page %s  (no reference, skipped diff)\n" "$page"
    done
    echo
    echo "summary: $total_pages candidate page(s) rendered, no PDF reference"
    echo "report: $REPORT"
fi

# Manifest sidecar — the web viewer reads this to populate the sample
# picker. Upserts an entry for $NAME without disturbing entries for
# other samples that have been diffed. Pure jq would be cleaner but
# we don't want to require jq; awk + python3 fallback covers macOS
# and most Linux dev boxes.
MANIFEST="$SAMPLE_DIR/manifest.json"
python3 - "$MANIFEST" "$NAME" "$IDML" "$PDF" "$total_pages" "$pass_pages" "$OUT" "$REPORT" <<'PY'
import json
import os
import sys
from pathlib import Path

(_, manifest, name, idml, pdf, total, passed, out, report) = sys.argv
data = {"samples": []}
mp = Path(manifest)
if mp.exists():
    try:
        data = json.loads(mp.read_text())
    except json.JSONDecodeError:
        data = {"samples": []}
data.setdefault("samples", [])
data["samples"] = [s for s in data["samples"] if s.get("name") != name]
data["samples"].append(
    {
        "name": name,
        "idml": os.path.basename(idml),
        "pdf": os.path.basename(pdf),
        "pages": int(total),
        "passing": int(passed),
        "diff_dir": out,
        "report": report,
    }
)
data["samples"].sort(key=lambda s: s["name"])
mp.write_text(json.dumps(data, indent=2) + "\n")
print(f"manifest: {manifest}")
PY
