# CLAUDE.md

Quick orientation for Claude sessions working on this repo. Covers
mechanics (commands, layout, conventions). This is the **public, open
engine** (`paged-media/core`).

## What this is

Pixel-faithful Adobe IDML renderer in Rust. 16-crate workspace, parses
IDML packages, lays out text with InDesign-calibrated Knuth-Plass, emits a
versioned display list, rasterises through tiny-skia (CPU, default) or
Vello (wgpu/GPU). Public APIs target both native and WASM.

```
paged-parse → paged-scene → paged-text → paged-compose → paged-gpu
                                              ↘ paged-renderer (top)
                                                   ↘ paged-fidelity (diff)
```

**Open/closed split.** This repo is the open engine, dual-licensed
**MPL-2.0 OR PMEL** (every source file carries the MPL header; new files
must too — copy from any `crates/**/*.rs`). The commercial editor is a
*separate, private* repo (`paged-media/editor`) that consumes the published
`@paged-media` SDK/wasm packages across a package boundary — there is **no
Cargo path dependency** from the editor into this code. Internal planning
docs live in the private `paged-media/thoughts` repo; the full Envato
fidelity corpus lives in the private `paged-media/corpus` repo.

## Common commands

```bash
# Build / typecheck
cargo build --workspace
cargo check --workspace --all-targets

# Tests
cargo test --workspace                    # everything
cargo test -p paged-fidelity              # diff-harness unit + CLI

# Render a sample
cargo run --release --bin paged-inspect -- \
  --render /tmp/out corpus/generated/<name>.idml

# Diff harness (positional: REFERENCE then CANDIDATE)
cargo run --release -p paged-fidelity --bin paged-diff -- \
  --json --heatmap heat.png ref.png cand.png

# Hard fidelity gate over corpus/generated/*.idml + *.pdf
./corpus/generated/diff.sh
# Requires pdftoppm (poppler-utils): `brew install poppler` on macOS.

# Lint + dependency-license audit
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                          # licenses + advisories + sources
```

`cargo fmt --all` is **not** kept clean across the workspace — running it
produces ~1000-line drifts on unrelated files. Format only files you've
touched. `Cargo.lock` **is** tracked here (reproducible public build).

## Layout

- `crates/paged-parse/` — ZIP + XML → AST. Container, designmap, spreads
  (TextFrame / Rectangle / Oval / Polygon / GraphicLine / Group), stories,
  graphic, gradients, ItemTransform, NextTextFrame, TextFramePreference,
  tabs, Image, styles + BasedOn cascade, bullets/numbering. Parses
  `<GeometryPathType>` subpath boundaries for compound paths.
- `crates/paged-scene/` — `Document::open`. Resolves cascade,
  `frame_for_story`, `text_frame_index`, `frame_chain` (threading).
- `crates/paged-text/` — `shape_run`, `compose_paragraph`,
  `layout_paragraph` / `layout_runs` (multi-font). Knuth-Plass +
  hyphenation. `apply_tab_stops`.
- `crates/paged-compose/` — display list, `Transform::for_rect_in`, glyph
  cache, gradient/image pools.
- `crates/paged-gpu/` — `PathRasterizer` trait. CPU (tiny-skia, default) +
  Vello backend (FillPath / StrokePath / Image / LinearGradient).
- `crates/paged-renderer/` — top-level `pipeline::build` / `render`.
  `StoryEmitter` per-story state. `paged-inspect` CLI.
- `crates/paged-color/` — lcms2 wrapper (native); naive fallback on wasm32.
- `crates/paged-fidelity/` — ΔE2000 + SSIM diff library + `paged-diff` CLI.
  Reference is positional arg #1, candidate #2.
- `crates/paged-mutate/`, `paged-introspect/`, `paged-canvas/`,
  `paged-script/` — mutation ops, scene/property introspection, the
  interactive canvas model, and the embedded `paged.*` scripting API. These
  back the editor; `paged-canvas-wasm` / `paged-introspect-wasm` are the
  wasm-bindgen surfaces it consumes.
- `crates/paged-gen/` — IDML fixture generator (the `paged-gen` bin).
- `crates/paged-sdk/` — published SDK wasm surface (`render_to_png`,
  `parse_summary`; npm `@paged-media/sdk`).
- `corpus/generated/` — license-clear generator fixtures (IDML + paired
  InDesign-exported PDF + meta JSON). Hard fidelity gate runs over these via
  `diff.sh` + `fidelity-thresholds.json`.
- `corpus/fonts/` — license-clear TTFs. `corpus/seeds/` — golden snapshot.
  `corpus/generated-fixtures/`, `corpus/calibration/` — more fixtures.
- `tools/indesign-export/` — drives InDesign to export the reference PDFs
  the fidelity harness consumes (Python/ExtendScript; outside the Cargo
  workspace, so core is polyglot — CI must not assume everything is Cargo).
- `spikes/` — Vello eval, composer calibration, WASM size.

## Conventions worth remembering

- **Don't loosen fidelity thresholds to make a failure go away.** The
  per-fixture thresholds in `corpus/generated/fidelity-thresholds.json` are
  sized to current measurements + ~15–25% headroom. If a regression trips
  them, fix the regression. Tighten *after* fixing, never before.
- **CPU backend for headless / CI.** Vello requires a GPU; the CI fidelity
  gate uses tiny-skia.
- **Reference PDFs are baked.** If a fixture's reference PDF was exported on
  a host without the IDML's declared font, InDesign falls back to its
  bundled serif and the PDF carries that serif baked in. Either re-export on
  a host with the font, or substitute in the renderer to match — pick one
  consciously and document it in the per-fixture `*.fonts.sh`.
- **Path: `subpath_starts` for compound paths.** A Polygon / TextFrame /
  GraphicLine with multiple `<GeometryPathType>` children must record
  per-contour boundaries, or the renderer joins contours into one polyline
  and silently mis-renders holes.
- **Per-paragraph state on `StoryEmitter`.** Vertical-justify distribute
  mode, numbering counter, frame-chain bookkeeping, and per-frame paragraph
  command ranges all live on `StoryEmitter` — add new per-story state there.
- **Keep the format/project naming line.** `idml` as the *project* prefix
  was renamed to `paged`; `idml` as the *format* (`.idml`, `idml_bytes`,
  "parses IDML", `designmap`, the `idml-package` mimetype) is kept. Don't
  blanket-rename across that line.
- **Comments earn their place.** Add one when the WHY is non-obvious (a
  hidden IDML constraint, a rasterizer invariant, an upstream-bug
  workaround). Otherwise let the code speak.
