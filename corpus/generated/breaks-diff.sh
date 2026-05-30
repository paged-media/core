#!/usr/bin/env bash
# Cycle-7 Track 1 self-diff harness: regenerate
# corpus/generated/text-letterspacing.idml, render it with
# --emit-breaks, and diff the resulting JSONL against the in-tree
# snapshot at corpus/generated/text-letterspacing.breaks.jsonl.
#
# This is a complement to corpus/generated/diff.sh — the latter
# gates on pixel fidelity against an InDesign-baked PDF, the former
# catches composer regressions where the breaker's line-break
# decisions shift on a known IDML. No InDesign dependency.
#
# Exit codes:
#   0  every line's (first_byte, last_byte, baseline_y_pt, width_pt)
#      matches the snapshot exactly, OR --advisory was passed
#   1  one or more lines diverged
#   2  toolchain / harness misconfiguration
#
# Re-pin the snapshot when the divergence is intentional (e.g. a
# planned composer change):
#   cp /tmp/text-letterspacing.cand.jsonl \
#      corpus/generated/text-letterspacing.breaks.jsonl

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GEN="$ROOT/target/release/paged-gen"
INSPECT="$ROOT/target/release/paged-inspect"
SNAPSHOT="$ROOT/corpus/generated/text-letterspacing.breaks.jsonl"
IDML="$ROOT/corpus/generated/text-letterspacing.idml"
OUT="${IDML_BREAKS_DIFF_OUT:-/tmp/text-letterspacing.cand.jsonl}"

ADVISORY=0
[ "${1:-}" = "--advisory" ] && ADVISORY=1

[ -f "$SNAPSHOT" ] || { echo "missing snapshot $SNAPSHOT"; exit 2; }

if [ ! -x "$GEN" ] || [ ! -x "$INSPECT" ]; then
    echo "==> build paged-gen + paged-inspect (release)"
    (cd "$ROOT" && cargo build --release \
        -p paged-gen --bin paged-gen \
        -p paged-renderer --bin paged-inspect >/dev/null 2>&1)
fi

"$GEN" emit --sample text-letterspacing --out "$ROOT/corpus/generated" >/dev/null
"$INSPECT" \
    --font "$ROOT/corpus/fonts/OpenSans.ttf" \
    --emit-breaks "$OUT" \
    "$IDML" >/dev/null 2>&1

# Compare JSONL lines field-by-field. We tolerate trivial float
# noise (1e-3pt) on baseline_y / width but require byte-range
# equality.
python3 - "$SNAPSHOT" "$OUT" <<'PY'
import json
import sys

snap_path, cand_path = sys.argv[1:3]
snap = [json.loads(l) for l in open(snap_path)]
cand = [json.loads(l) for l in open(cand_path)]
if len(snap) != len(cand):
    print(f"line count differs: snapshot={len(snap)} candidate={len(cand)}")
    sys.exit(1)

fails = []
for i, (s, c) in enumerate(zip(snap, cand)):
    # Byte-range divergence is a real wrap shift.
    if (s["page_idx"], s["paragraph_idx"], s["line_idx"]) != \
       (c["page_idx"], c["paragraph_idx"], c["line_idx"]):
        fails.append(f"line {i}: positional ids differ")
        continue
    if (s["first_byte"], s["last_byte"]) != (c["first_byte"], c["last_byte"]):
        fails.append(
            f"page={s['page_idx']} line={s['line_idx']}: "
            f"bytes {s['first_byte']}-{s['last_byte']} → "
            f"{c['first_byte']}-{c['last_byte']}"
        )
        continue
    # Sub-millipt drift in baseline / width is acceptable (e.g.
    # rasteriser rounding); larger gaps indicate a real shift.
    if abs(s["baseline_y_pt"] - c["baseline_y_pt"]) > 1e-3:
        fails.append(
            f"page={s['page_idx']} line={s['line_idx']}: "
            f"baseline_y {s['baseline_y_pt']} → {c['baseline_y_pt']}"
        )
    if abs(s["width_pt"] - c["width_pt"]) > 1e-3:
        fails.append(
            f"page={s['page_idx']} line={s['line_idx']}: "
            f"width {s['width_pt']} → {c['width_pt']}"
        )

if fails:
    for f in fails[:20]:
        print(f"  FAIL: {f}")
    if len(fails) > 20:
        print(f"  ... and {len(fails) - 20} more")
    sys.exit(1)
print(f"text-letterspacing: {len(snap)} lines unchanged ✓")
PY
result=$?

if [ $result -ne 0 ] && [ $ADVISORY -eq 1 ]; then
    echo "==> advisory mode: not failing run"
    exit 0
fi
exit $result
