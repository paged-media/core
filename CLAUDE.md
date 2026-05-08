# CLAUDE.md

Quick orientation for Claude sessions working on this repo. Source of
truth for *plan / status* is `docs/plan.md`; this file covers
mechanics (commands, layout, conventions) that don't fit there.

## What this is

Pixel-faithful Adobe IDML renderer in Rust. 13-crate workspace, parses
IDML packages, lays out text with InDesign-calibrated Knuth-Plass,
emits a versioned display list, rasterises through tiny-skia (CPU,
default) or Vello (wgpu/GPU). Public APIs target both native and WASM.

Pipeline:

```
idml-parse → idml-scene → idml-text → idml-compose → idml-gpu
                                              ↘ idml-renderer (top)
                                                   ↘ idml-fidelity (diff)
```

## Common commands

```bash
# Build / typecheck
cargo build --workspace
cargo check --workspace --all-targets

# Tests
cargo test --workspace                    # everything
cargo test -p idml-fidelity               # diff-harness unit + CLI
cargo test -p idml-renderer self_diff     # self-diff against golden

# Render a sample
cargo run --release --bin idml-inspect -- \
  --render /tmp/out corpus/generated/<name>.idml

# Diff harness (positional: REFERENCE then CANDIDATE)
cargo run --release -p idml-fidelity --bin idml-diff -- \
  --json --heatmap heat.png ref.png cand.png

# Hard fidelity gate over corpus/generated/*.idml + *.pdf
./corpus/generated/diff.sh
# Requires pdftoppm (poppler-utils): `brew install poppler` on macOS.

# Lint
cargo clippy --workspace --all-targets
```

`cargo fmt --all` is **not** kept clean across the workspace — running
it produces ~1000-line drifts on unrelated files. Format only files
you've touched.

## Layout

- `crates/idml-parse/` — ZIP + XML → AST. Container, designmap,
  spreads (TextFrame / Rectangle / Oval / Polygon / GraphicLine /
  Group), stories, graphic, gradients, ItemTransform, NextTextFrame,
  TextFramePreference, tabs, Image, styles + BasedOn cascade,
  bullets/numbering. Also parses `<GeometryPathType>` subpath
  boundaries for compound paths.
- `crates/idml-scene/` — `Document::open`. Resolves cascade,
  `frame_for_story`, `text_frame_index`, `frame_chain` (threading).
- `crates/idml-text/` — `shape_run`, `compose_paragraph`,
  `layout_paragraph` / `layout_runs` (multi-font). Knuth-Plass +
  hyphenation. `apply_tab_stops`.
- `crates/idml-compose/` — display list, `Transform::for_rect_in`,
  glyph cache, gradient/image pools.
- `crates/idml-gpu/` — `PathRasterizer` trait. CPU (tiny-skia,
  default) + Vello backend (FillPath / StrokePath / Image /
  LinearGradient; DropShadow stub).
- `crates/idml-renderer/` — top-level `pipeline::build` /
  `pipeline::render`. `StoryEmitter` per-story state. `idml-inspect`
  CLI.
- `crates/idml-color/` — lcms2 wrapper (native); naive fallback on
  wasm32.
- `crates/idml-fidelity/` — ΔE2000 + SSIM diff library + `idml-diff`
  CLI. Reference is positional arg #1, candidate #2.
- `crates/idml-edit/` + `crates/idml-edit-wasm/` — editing surface +
  WASM bindings.
- `crates/idml-wasm/` — `render_to_png` / `parse_summary`.
- `corpus/generated/` — license-clear generator-produced fixtures
  (IDML + paired InDesign-exported PDF + meta JSON). Hard fidelity
  gate runs over these via `diff.sh` + `fidelity-thresholds.json`.
- `corpus/samples/` — manually-staged samples, gitignored, advisory
  only.
- `corpus/seeds/hello/` — golden snapshot (PNG-pinned regression
  test).
- `corpus/fonts/` — license-clear TTFs (Open Sans, Cormorant
  Garamond, Inter, Lora, Roboto, Source Serif 4, Roboto Slab).
- `spikes/` — Vello eval, composer calibration, WASM size.
- `docs/plan.md` — **active backlog and phase status. Read first.**
- `docs/idea.md` — original technical spec.

## Conventions worth remembering

- **Don't loosen fidelity thresholds to make a failure go away.** The
  per-fixture thresholds in `corpus/generated/fidelity-thresholds.json`
  are sized to current measurements + ~15–25% headroom. If a regression
  trips them, fix the regression. Tighten thresholds *after* fixing,
  never before.
- **CPU backend for headless / CI.** Vello requires a GPU; the CI
  fidelity gate uses tiny-skia.
- **Reference PDFs are baked.** When a fixture's reference PDF was
  exported on a host without the IDML's declared font installed,
  InDesign falls back to its bundled serif and the PDF carries that
  serif baked in. Either re-export the PDF on a host that has the
  font, or substitute in the renderer to match the PDF — but pick
  one consciously and document it in the per-fixture `*.fonts.sh`.
- **Path: `subpath_starts` for compound paths.** A Polygon /
  TextFrame / GraphicLine with multiple `<GeometryPathType>` children
  must record per-contour boundaries. Without them the renderer joins
  contours into one polyline and silently mis-renders holes.
- **Per-paragraph state on `StoryEmitter`.** Vertical-justify
  distribute mode, numbering counter, frame-chain bookkeeping, and
  per-frame paragraph command ranges all live on `StoryEmitter` —
  add new per-story state there, don't sprinkle it across closures.
- **Comments earn their place.** The codebase generally avoids
  narrating comments. Add one when the WHY is non-obvious (a hidden
  IDML constraint, a rasterizer-specific invariant, a workaround for
  a specific upstream bug). Otherwise let the code speak.

## Where the agents land

When parallel agents work in worktrees, they live under
`.claude/worktrees/agent-<id>/` and the runtime locks them while
active. Worktrees are `git worktree remove -f -f`-able after merge.
