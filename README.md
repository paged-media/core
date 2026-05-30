# paged

**Pixel-faithful Adobe IDML rendering, in Rust.** `paged` parses IDML
packages, lays out text with an InDesign-calibrated Knuth–Plass composer,
emits a versioned display list, and rasterises it through tiny-skia (CPU,
default) or Vello (wgpu/GPU). Public APIs target both native and WASM.

This repository is the **open engine** — the rendering pipeline, including
the fidelity-critical pieces (calibrated line-breaking, ICC colour, the
Vello/WebGPU print path). The commercial editor built on top of it is
closed and lives elsewhere; it consumes the published `@paged-media` SDK
across a package boundary, never as a source dependency.

The engine is format-agnostic in design; IDML is the first input format.

## License

Dual-licensed: **MPL-2.0 OR the Paged Media Enterprise License (PMEL)**
(commercial, from And The Next GmbH). MPL's file-level copyleft keeps the
engine freely embeddable while requiring changes to the engine's own files
to flow back. See [`LICENSE.md`](./LICENSE.md), [`LICENSE`](./LICENSE), and
[`CONTRIBUTING.md`](./CONTRIBUTING.md) (contributions are under a CLA).

## Pipeline

```
idml bytes
 └─ paged-parse     ZIP + designmap + spreads + stories + graphic → AST
     └─ paged-scene    owned Document (palette, spreads, stories, threading)
         └─ paged-text     rustybuzz shaping + Knuth–Plass compose + layout
             └─ paged-compose  display list (FillPath / StrokePath / Image /
                               gradients), glyph + path interning
                 └─ paged-gpu     tiny-skia (CPU, default) | Vello (wgpu/GPU)
                     └─ RgbaImage / PNG
                         └─ paged-fidelity   ΔE2000 + SSIM diff vs reference
```

```rust
use paged_renderer::{Document, pipeline, PipelineOptions, Color};

let document = Document::open(&idml_bytes)?;          // `idml_bytes`: the .idml package
let opts = PipelineOptions::default();
let built = pipeline::build(&document, &opts)?;                       // display list
let (built, image) = pipeline::render(&document, &opts, 144.0, Color::WHITE)?; // + raster (cpu)
```

## Workspace

Cargo workspace, 16 crates + spikes. Internal crates stay descriptively
named; the published artifact is the SDK.

| crate | role |
|-------|------|
| `paged-parse` | ZIP + XML → typed AST (container, designmap, spreads, stories, graphic, styles + cascade, paths) |
| `paged-scene` | `Document::open`; cascade resolution, frame-for-story, frame chaining |
| `paged-text` | shaping (rustybuzz), Knuth–Plass compose, multi-font layout, hyphenation, tabs |
| `paged-color` | ICC transforms (lcms2 native; naive fallback on wasm32) |
| `paged-compose` | display list, transforms, glyph cache, gradient/image pools |
| `paged-gpu` | `PathRasterizer` trait; CPU (tiny-skia) + Vello backends |
| `paged-renderer` | top-level `pipeline::build` / `render`; the `paged-inspect` CLI |
| `paged-fidelity` | ΔE2000 + SSIM diff library; the `paged-diff` CLI |
| `paged-mutate` | document mutation operations (undoable) |
| `paged-introspect` | scene-graph + property descriptors for inspection |
| `paged-canvas` | interactive canvas model over the renderer + mutate |
| `paged-script` | embedded scripting (Boa); the `paged.*` global API |
| `paged-canvas-wasm`, `paged-introspect-wasm` | wasm-bindgen surfaces for the editor |
| `paged-gen` | IDML fixture generator (the `paged-gen` bin) |
| `paged-sdk` | the published SDK wasm surface — a WebGPU `ViewerSession` (load → present to canvas → headless RGBA readback); npm `@paged-media/sdk` |

`spikes/` — Vello eval, composer calibration, WASM size.
`tools/indesign-export/` — drives InDesign to export reference PDFs for the
fidelity harness (Python/ExtendScript; outside the Cargo workspace).

## CLI

- **`paged-inspect <file.idml>`** — parse + summarise a package; render with
  `--render <out.png>` (flags: `--font`, `--display-list`, `--dpi`, …).
- **`paged-diff <reference.png> <candidate.png>`** — ΔE2000 + SSIM report;
  exits non-zero on fail.

## Build & test

Toolchain pinned in `rust-toolchain.toml`.

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target wasm32-unknown-unknown -p paged-sdk

# Fidelity gate over the public golden set:
./corpus/generated/diff.sh        # needs pdftoppm (poppler-utils)
```

CPU (tiny-skia) is the default and is what headless CI uses; Vello needs a
GPU. Don't run `cargo fmt --all` workspace-wide (it drifts unrelated files).

## Corpus

`corpus/generated/` (+ `generated-fixtures`, `fonts`, `seeds`,
`calibration`) is a **license-clear** golden set committed here so external
contributors can run the fidelity gate. The full Envato-backed corpus and
the InDesign export harness are internal (private) and not required to
contribute.
