#!/usr/bin/env bash
# corpus/generated/diff.sh
#
# Hard-failing fidelity gate over `corpus/generated/*.idml + *.pdf`.
#
# Pipeline per fixture:
#   1. Regenerate the IDML via `cargo run -p paged-gen -- emit --sample <name>`.
#      Generated IDMLs are gitignored and reproducible.
#   2. Render every page through the CPU backend → cand-NNN.png
#      (delegates to corpus/samples/diff.sh, which already wires up
#      paged-inspect + per-fixture font flags).
#   3. Rasterise each PDF page via pdftoppm → ref-NNN.png.
#   4. Run paged-diff per page → JSON report (mean ΔE / p99 ΔE / SSIM).
#   5. Compare against per-fixture worst-page tolerances in
#      corpus/generated/fidelity-thresholds.json. Any page exceeding
#      a threshold fails the run.
#
# Outputs (per fixture, under $IDML_GENERATED_OUT/<name>):
#   cand-NNN.png      candidate (renderer)
#   ref-NNN.png       reference (rasterised PDF)
#   heat-NNN.png      heatmap, only on threshold violations
#   report.json       per-page metrics from corpus/samples/diff.sh
#   gate.json         { fixture, pages_checked, passed, failures: [...] }
#
# Usage:
#   ./corpus/generated/diff.sh                  # gate every fixture, fail on any miss
#   ./corpus/generated/diff.sh geometry text    # gate only the named fixtures
#   IDML_DIFF_GATE=advisory ./corpus/generated/diff.sh    # never fail
#
# Exit status:
#   0  every fixture's gated pages stayed within threshold (or advisory mode)
#   1  one or more fixtures regressed past threshold

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GENERATED_DIR="$ROOT/corpus/generated"
THRESHOLDS="$GENERATED_DIR/fidelity-thresholds.json"
SAMPLES_DIFF="$ROOT/corpus/samples/diff.sh"
GATE_OUT="${IDML_GENERATED_OUT:-/tmp/idml-generated-diff}"
GATE_MODE="${IDML_DIFF_GATE:-strict}"   # strict | advisory

[ -f "$THRESHOLDS" ] || { echo "missing $THRESHOLDS"; exit 2; }
[ -x "$SAMPLES_DIFF" ] || { echo "missing $SAMPLES_DIFF"; exit 2; }
command -v pdftoppm >/dev/null || { echo "install poppler-utils (pdftoppm)"; exit 2; }
command -v python3 >/dev/null || { echo "install python3"; exit 2; }

# Default fixture list = every entry in fidelity-thresholds.json. Pass
# names on the CLI to restrict; unknown names will fall through to
# "fixture has no PDF" and be skipped with a warning.
if [ "$#" -gt 0 ]; then
    FIXTURES=("$@")
else
    # `mapfile` is bash 4+; macOS ships bash 3, so we read into an
    # array the portable way.
    FIXTURES=()
    while IFS= read -r line; do
        [ -n "$line" ] && FIXTURES+=("$line")
    done < <(python3 -c "
import json
print('\n'.join(f['name'] for f in json.load(open('$THRESHOLDS'))['fixtures']))
")
fi

rm -rf "$GATE_OUT"
mkdir -p "$GATE_OUT"

echo "==> build paged-gen + paged-diff + paged-inspect (release)"
(cd "$ROOT" && cargo build --release \
    -p paged-gen --bin paged-gen \
    -p paged-fidelity --bin paged-diff \
    -p paged-renderer --bin paged-inspect >/dev/null 2>&1)

OVERALL_PASS=1

for fixture in "${FIXTURES[@]}"; do
    pdf="$GENERATED_DIR/$fixture.pdf"
    if [ ! -f "$pdf" ]; then
        echo "==> [$fixture] no reference PDF at $pdf — skipping"
        continue
    fi

    echo
    echo "==> [$fixture] regenerate IDML"
    "$ROOT/target/release/paged-gen" emit --sample "$fixture" --out "$GENERATED_DIR" >/dev/null

    fixture_out="$GATE_OUT/$fixture"
    mkdir -p "$fixture_out"
    echo "==> [$fixture] render + rasterise + per-page diff -> $fixture_out"
    # corpus/samples/diff.sh already handles the heavy lifting:
    # picks up the IDML from corpus/generated/ when present (line 26),
    # writes report.json to $IDML_DIFF_OUT, applies per-fixture font flags.
    # NOTE: samples/diff.sh `rm -rf $OUT` before writing, so we capture
    # the log in $GATE_OUT (sibling of $fixture_out) where it survives.
    log="$GATE_OUT/$fixture.log"
    IDML_DIFF_OUT="$fixture_out" "$SAMPLES_DIFF" "$fixture" \
        > "$log" 2>&1 || true
    if [ ! -f "$fixture_out/report.json" ]; then
        echo "==> [$fixture] diff.sh did not produce report.json:"
        tail -40 "$log" || true
        OVERALL_PASS=0
        continue
    fi

    # Apply per-fixture thresholds. Python is fine here because we
    # only need it once per fixture and the Cargo cache may not have
    # serde-toml available in restricted environments. We disable
    # `set -e` for this block so the wrapper can collect failures
    # across fixtures rather than aborting on the first regression.
    set +e
    python3 - "$THRESHOLDS" "$fixture" "$fixture_out/report.json" \
        "$fixture_out/gate.json" <<'PY'
import json
import sys
from pathlib import Path

(_, thresholds_path, fixture, report_path, gate_path) = sys.argv
thresholds = json.load(open(thresholds_path))
spec = next((f for f in thresholds["fixtures"] if f["name"] == fixture), None)
if spec is None:
    print(f"[{fixture}] not in fidelity-thresholds.json — skipping gate")
    Path(gate_path).write_text(json.dumps({
        "fixture": fixture, "skipped": True, "reason": "not in manifest"
    }))
    sys.exit(0)

pages = json.load(open(report_path))
gated = [p for p in pages if p["page"] <= spec["max_pages_with_pdf"]]
failures = []
for p in gated:
    page_failures = []
    if p["mean_de"] > spec["max_mean_de"]:
        page_failures.append(
            f"meanΔE {p['mean_de']:.3f} > {spec['max_mean_de']:.3f}"
        )
    if p["p99_de"] > spec["max_p99_de"]:
        page_failures.append(
            f"p99ΔE {p['p99_de']:.3f} > {spec['max_p99_de']:.3f}"
        )
    if p["ssim"] < spec["min_ssim"]:
        page_failures.append(
            f"ssim {p['ssim']:.4f} < {spec['min_ssim']:.4f}"
        )
    if page_failures:
        failures.append({
            "page": p["page"],
            "mean_de": p["mean_de"],
            "p99_de": p["p99_de"],
            "ssim": p["ssim"],
            "violations": page_failures,
        })

result = {
    "fixture": fixture,
    "pages_checked": len(gated),
    "pages_total": len(pages),
    "passed": not failures,
    "thresholds": {
        "max_mean_de": spec["max_mean_de"],
        "max_p99_de": spec["max_p99_de"],
        "min_ssim": spec["min_ssim"],
    },
    "failures": failures,
}
Path(gate_path).write_text(json.dumps(result, indent=2))

if failures:
    print(f"[{fixture}] FAIL: {len(failures)}/{len(gated)} pages over threshold "
          f"(mean<= {spec['max_mean_de']}, p99<= {spec['max_p99_de']}, "
          f"ssim>= {spec['min_ssim']})")
    for fail in failures:
        print(f"  page {fail['page']:03d}: " + "; ".join(fail["violations"]))
    sys.exit(1)
print(f"[{fixture}] PASS: {len(gated)}/{len(gated)} gated pages within tolerance "
      f"(mean<= {spec['max_mean_de']}, p99<= {spec['max_p99_de']}, "
      f"ssim>= {spec['min_ssim']})")
PY
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then
        OVERALL_PASS=0
    fi
done

echo
if [ "$OVERALL_PASS" -eq 1 ]; then
    echo "==> all gated fixtures within tolerance"
    exit 0
fi

echo "==> one or more fixtures regressed"
if [ "$GATE_MODE" = "advisory" ]; then
    echo "==> advisory mode (IDML_DIFF_GATE=advisory) — exiting 0 anyway"
    exit 0
fi
exit 1
