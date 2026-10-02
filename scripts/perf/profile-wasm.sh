#!/usr/bin/env bash
# scripts/perf/profile-wasm.sh — build a NAMED canvas-wasm for profiling.
#
# The published wasm has no name section, so a browser profile shows
# `wasm-function[14849]`. Both performance bugs found in October 2026 (the
# per-paragraph font hash and the per-repaint image decode) were only
# diagnosable with function names: keep the release optimisation, add the
# name section (`wasm-opt -Oz -g`).
#
#   scripts/perf/profile-wasm.sh [OUT_DIR]      # default: target/wasm-named
#
# Then point the editor at OUT_DIR (a `file:` override, never committed),
# record a Chrome trace with `disabled-by-default-v8.cpu_profiler`, and
# aggregate it:
#
#   python3 scripts/perf/aggregate-trace.py trace.json
#
# wasm-bindgen must match the `wasm-bindgen` version in Cargo.lock; the
# script refuses to run with a mismatched CLI instead of failing at the end.
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT="${1:-target/wasm-named}"
want="$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/version = |"/,""); print; exit}' Cargo.lock)"
have="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')"
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen CLI $have != Cargo.lock $want" >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked --root <dir>  (then put <dir>/bin on PATH)" >&2
  exit 1
fi
CARGO_INCREMENTAL=0 cargo build --release --target wasm32-unknown-unknown -p paged-canvas-wasm --features gpu
mkdir -p "$OUT"
wasm-bindgen target/wasm32-unknown-unknown/release/paged_canvas_wasm.wasm --target web --out-dir "$OUT"
wasm-opt -Oz -g "$OUT/paged_canvas_wasm_bg.wasm" -o "$OUT/paged_canvas_wasm_bg.wasm"
echo "named wasm in $OUT ($(du -h "$OUT/paged_canvas_wasm_bg.wasm" | cut -f1))"
