#!/usr/bin/env bash
# corpus/generated/export-diff.sh
#
# PDF-export SELF-CONSISTENCY loop (Concept 3, M9): the exported PDF,
# rasterised by poppler, must match our own CPU renderer on the same
# scene. This is not the InDesign fidelity gate (diff.sh) — it pins
# the EXPORTER against the renderer so a regression in either path
# (text-as-text placement, colour encoding, transparency groups,
# image flip) shows up as pixels.
#
# Pipeline per fixture:
#   1. cargo run -p paged-export-pdf --features cli --bin paged-export
#      → <name>.pdf (PDF 1.7, no profile — pure geometry/colour pin).
#   2. pdftoppm -r 144 → pdf-NNN.png.
#   3. paged-inspect --render (CPU, 144 dpi) → native-NNN.png.
#   4. paged-diff per page; fail when mean ΔE > 1.5 or SSIM < 0.93
#      (text AA differs between poppler and tiny-skia; geometry or
#      colour bugs blow past these immediately).
#
# Usage:
#   ./corpus/generated/export-diff.sh                # all fixtures
#   ./corpus/generated/export-diff.sh geometry       # one fixture
#   IDML_EXPORT_DIFF_GATE=advisory ./export-diff.sh  # never fail
#
# Requires pdftoppm (poppler). Fixture IDMLs must exist (run
# diff.sh / paged-gen first).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GENERATED_DIR="$ROOT/corpus/generated"
OUT="${IDML_EXPORT_DIFF_OUT:-/tmp/paged-export-diff}"
GATE="${IDML_EXPORT_DIFF_GATE:-hard}"
MAX_MEAN_DE=1.5
MIN_SSIM=0.93
FALLBACK_FONT="$ROOT/corpus/fonts/Inter.ttf"

# v1 exporter gap, documented: gradient shadings interpolate sRGB
# stops in DeviceRGB while the renderer interpolates in CMYK space
# (pdftoppm-calibrated) — mid-tones drift ~ΔE 1.8 on gradient-heavy
# pages. Native CMYK/Lab shadings are the v2 fix; until then the
# gradients fixture gets its own ceiling.
max_mean_for() {
  case "$1" in
    gradients) echo 2.2 ;;
    *) echo "$MAX_MEAN_DE" ;;
  esac
}

# Use ONE CMYK profile on BOTH sides: the exporter tags CMYK content
# ICCBased(N4) so poppler converts through the same profile the
# native render uses — otherwise poppler's naive DeviceCMYK black
# (#000) diverges from the renderer's profile black (~#1a1a1a).
find_profile() {
  if [ -n "${PAGED_CMYK_PROFILE:-}" ] && [ -f "$PAGED_CMYK_PROFILE" ]; then
    echo "$PAGED_CMYK_PROFILE"; return
  fi
  for f in "$ROOT"/corpus/profiles/*.icc; do
    [ -f "$f" ] && { echo "$f"; return; }
  done
  local adobe="/Library/Application Support/Adobe/Color/Profiles/Recommended/CoatedFOGRA39.icc"
  [ -f "$adobe" ] && { echo "$adobe"; return; }
  echo ""
}
PROFILE="$(find_profile)"
[ -n "$PROFILE" ] || echo "note: no CMYK profile found — K-blacks will diverge (poppler naive vs renderer)"

command -v pdftoppm >/dev/null || { echo "pdftoppm not found (brew install poppler)"; exit 1; }

fixtures=()
if [ "$#" -gt 0 ]; then
  for n in "$@"; do fixtures+=("$GENERATED_DIR/$n.idml"); done
else
  for f in "$GENERATED_DIR"/*.idml; do fixtures+=("$f"); done
fi

cd "$ROOT"
cargo build -q --release -p paged-export-pdf --features cli --bin paged-export
cargo build -q --release --bin paged-inspect
cargo build -q --release -p paged-fidelity --bin paged-diff
EXPORT="$ROOT/target/release/paged-export"
INSPECT="$ROOT/target/release/paged-inspect"
DIFF="$ROOT/target/release/paged-diff"

fail=0
for idml in "${fixtures[@]}"; do
  name="$(basename "$idml" .idml)"
  [ -f "$idml" ] || { echo "missing fixture: $idml (run diff.sh first)"; exit 1; }
  dir="$OUT/$name"
  rm -rf "$dir"; mkdir -p "$dir"

  export_args=("$idml" "$dir/$name.pdf" --font "$FALLBACK_FONT")
  inspect_args=(--render "$dir/native.png" --default-font "$FALLBACK_FONT")
  if [ -n "$PROFILE" ]; then
    export_args+=(--profile "$PROFILE")
    inspect_args+=(--cmyk-profile "$PROFILE")
  fi
  "$EXPORT" "${export_args[@]}" >/dev/null 2>&1
  pdftoppm -png -r 144 "$dir/$name.pdf" "$dir/pdf"
  "$INSPECT" "${inspect_args[@]}" "$idml" >/dev/null 2>&1
  max_mean="$(max_mean_for "$name")"

  page=1
  for native in "$dir"/native*.png; do
    pdfpage=$(printf "%s/pdf-%d.png" "$dir" "$page")
    [ -f "$pdfpage" ] || pdfpage=$(printf "%s/pdf-%02d.png" "$dir" "$page")
    [ -f "$pdfpage" ] || { echo "$name p$page: missing pdf raster"; fail=1; page=$((page+1)); continue; }
    # paged-diff exits non-zero when its own default threshold trips
    # — we gate on OUR thresholds below, so tolerate the status.
    json="$("$DIFF" --json "$native" "$pdfpage" || true)"
    mean=$(echo "$json" | python3 -c "import json,sys; print(json.load(sys.stdin)['mean_de'])")
    ssim=$(echo "$json" | python3 -c "import json,sys; print(json.load(sys.stdin)['ssim'])")
    ok=$(python3 -c "print(int($mean <= $max_mean and $ssim >= $MIN_SSIM))")
    status="ok"
    if [ "$ok" != "1" ]; then status="FAIL"; fail=1; fi
    echo "$name p$page: mean_de=$mean ssim=$ssim [$status]"
    page=$((page+1))
  done
done

if [ "$GATE" = "advisory" ]; then exit 0; fi
exit $fail
