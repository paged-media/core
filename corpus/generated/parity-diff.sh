#!/usr/bin/env bash
# corpus/generated/parity-diff.sh
#
# CPU ↔ Vello parity gate over `corpus/generated/*.idml`.
#
# The fidelity gate (diff.sh) measures the CPU rasterizer against
# InDesign. The editor and the viewer draw with Vello, which nothing
# measured: shadows were skipped, dashes drew solid and every blend mode
# read the paper as a backdrop, and every fixture still passed. This
# script renders every page through BOTH backends from the same display
# list and diffs them, so the GPU lane is held to the CPU lane, which is
# held to InDesign.
#
# Per fixture (under $IDML_PARITY_OUT/<name>):
#   cpu-NNN.png / vello-NNN.png   the two renders
#   heat-NNN.png                  heatmap, only on budget violations
#   pages.json                    per-page { page, mean_de, p99_de, ssim }
# and $IDML_PARITY_OUT/parity.json with every fixture's worst page.
#
# Budgets live in corpus/generated/parity-thresholds.json: a default
# worst-page mean ΔE, and per-fixture entries, each with the measured
# cause when it is above the default. The same never-loosen rule as
# fidelity-thresholds.json applies: fix the lane, then tighten.
#
# The CPU side renders WITHOUT --snap-glyph-origins. That flag mimics
# pdftoppm's glyph phase for the InDesign comparison; Vello has no such
# mode, so leaving it on would measure the snap, not the rasterizers.
#
# Needs paged-inspect built with the `gpu` feature and a wgpu adapter
# (a GPU, or Mesa lavapipe in CI — see .github/workflows/gpu.yml).
#
# Usage:
#   cargo build --release -p paged-renderer --features gpu --bin paged-inspect
#   cargo build --release -p paged-fidelity --bin paged-diff
#   ./corpus/generated/parity-diff.sh                 # every fixture
#   ./corpus/generated/parity-diff.sh effects text    # named fixtures
#   IDML_PARITY_GATE=advisory ./corpus/generated/parity-diff.sh   # never fail
#
# Exit status: 0 within budget (or advisory), 1 a fixture over budget,
# 2 a broken setup (no binary, no adapter).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GENERATED_DIR="$ROOT/corpus/generated"
FONTS="$ROOT/corpus/fonts"
THRESHOLDS="$GENERATED_DIR/parity-thresholds.json"
OUT_ROOT="${IDML_PARITY_OUT:-/tmp/idml-parity}"
GATE_MODE="${IDML_PARITY_GATE:-strict}"   # strict | advisory
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
INSPECT="$TARGET/release/paged-inspect"
DIFF="$TARGET/release/paged-diff"
DPI="${IDML_PARITY_DPI:-144}"

for bin in "$INSPECT" "$DIFF"; do
    [ -x "$bin" ] || { echo "error: $bin is missing; build it first (see Usage)." >&2; exit 2; }
done
[ -f "$THRESHOLDS" ] || { echo "error: missing $THRESHOLDS" >&2; exit 2; }

if [ "$#" -gt 0 ]; then
    FIXTURES=("$@")
else
    FIXTURES=()
    while IFS= read -r line; do
        [ -n "$line" ] && FIXTURES+=("$line")
    done < <(python3 -c "
import json
print('\n'.join(f['name'] for f in json.load(open('$GENERATED_DIR/fidelity-thresholds.json'))['fixtures']))
")
fi

rm -rf "$OUT_ROOT"
mkdir -p "$OUT_ROOT"

for NAME in "${FIXTURES[@]}"; do
    IDML="$GENERATED_DIR/$NAME.idml"
    if [ ! -f "$IDML" ]; then
        echo "==> $NAME: no $IDML (run paged-gen emit-all first); skipped" >&2
        continue
    fi
    OUT="$OUT_ROOT/$NAME"
    mkdir -p "$OUT"
    DEFAULT_FONT="$FONTS/SourceSerif4.ttf"
    FONT_FLAGS=(
        --font-family "Open Sans=$FONTS/OpenSans.ttf"
        --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
        --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
    )
    if [ -f "$GENERATED_DIR/$NAME.fonts.sh" ]; then
        # shellcheck disable=SC1090
        . "$GENERATED_DIR/$NAME.fonts.sh"
    fi
    for backend in cpu vello; do
        if ! "$INSPECT" "$IDML" --render "$OUT/$backend.png" --backend "$backend" \
            --default-font "$DEFAULT_FONT" "${FONT_FLAGS[@]}" \
            --no-missing-image-placeholder --dpi "$DPI" >/dev/null 2>"$OUT/$backend.err"; then
            # paged-inspect exits 3 when Vello drew nothing at all.
            echo "error: $NAME: the $backend render failed:" >&2
            tail -8 "$OUT/$backend.err" >&2
            exit 2
        fi
        # A build without the `gpu` feature renders "vello" on the CPU,
        # and a failed adapter request returns empty pages: either way
        # the comparison would measure nothing.
        if grep -q "needs the \`gpu\` feature\|GPU init failed" "$OUT/$backend.err"; then
            echo "error: $NAME: no Vello render ($(head -1 "$OUT/$backend.err"))" >&2
            exit 2
        fi
        # Single-page documents render to the bare name (see render-diff.sh).
        [ -f "$OUT/$backend.png" ] && mv "$OUT/$backend.png" "$OUT/$backend-001.png"
    done
    : > "$OUT/pages.tsv"
    for cpu in "$OUT"/cpu-*.png; do
        page="${cpu##*/cpu-}"
        page="${page%.png}"
        vello="$OUT/vello-$page.png"
        [ -f "$vello" ] || { echo "error: $NAME: Vello rendered no page $page" >&2; exit 2; }
        "$DIFF" --json "$cpu" "$vello" > "$OUT/diff-$page.json" || true
        echo "$page $OUT/diff-$page.json" >> "$OUT/pages.tsv"
    done
done

set +e
python3 - "$OUT_ROOT" "$THRESHOLDS" "$GATE_MODE" "$DIFF" "${FIXTURES[@]}" <<'PY'
import json, os, subprocess, sys

out_root, thresholds_path, mode, diff_bin, *names = sys.argv[1:]
budgets = json.load(open(thresholds_path))
default = budgets["default_worst_mean_de"]
per = {f["name"]: f for f in budgets["fixtures"]}
report, failed = [], []
for name in names:
    tsv = os.path.join(out_root, name, "pages.tsv")
    if not os.path.exists(tsv):
        continue
    pages = []
    for line in open(tsv):
        page, path = line.split()
        d = json.load(open(path))
        pages.append({"page": int(page), "mean_de": d["mean_de"],
                      "p99_de": d["p99_de"], "ssim": d["ssim"]})
    json.dump(pages, open(os.path.join(out_root, name, "pages.json"), "w"), indent=1)
    worst = max(pages, key=lambda p: p["mean_de"])
    budget = per.get(name, {}).get("worst_mean_de", default)
    ok = worst["mean_de"] <= budget
    report.append({"fixture": name, "pages": len(pages), "worst_page": worst["page"],
                   "worst_mean_de": worst["mean_de"], "budget": budget, "passed": ok})
    flag = "ok  " if ok else "OVER"
    print(f"{flag} {name:26s} pages {len(pages):3d}  worst p{worst['page']:03d} "
          f"mean ΔE {worst['mean_de']:.3f}  (budget {budget:.3f})")
    if not ok:
        failed.append(name)
        for p in pages:
            if p["mean_de"] > budget:
                n = f"{p['page']:03d}"
                d = os.path.join(out_root, name)
                subprocess.run([diff_bin, "--json", "--heatmap", f"{d}/heat-{n}.png",
                                f"{d}/cpu-{n}.png", f"{d}/vello-{n}.png"],
                               stdout=subprocess.DEVNULL, check=False)
stale = [n for n, f in per.items() if n in names and any(r["fixture"] == n for r in report)
         and f.get("worst_mean_de", default) > default
         and next(r for r in report if r["fixture"] == n)["worst_mean_de"] <= default]
json.dump({"default_budget": default, "fixtures": report},
          open(os.path.join(out_root, "parity.json"), "w"), indent=1)
within = sum(r["passed"] for r in report)
print(f"\n{within}/{len(report)} fixtures within their CPU↔Vello budget")
for n in stale:
    print(f"note: {n} now measures within the default budget; drop its entry")
if failed:
    print("over budget: " + " ".join(failed))
    sys.exit(0 if mode == "advisory" else 1)
PY
status=$?
exit "$status"
