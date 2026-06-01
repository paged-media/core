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

## Capabilities

What the engine does with each IDML construct today, split by pipeline
stage: **Parsed** (the parser reads it into the model) and **Rendered**
(the renderer acts on it in output). `✅` full · `◑` partial (see note) ·
`—` not yet · `n/a` not a render concern. Grouped by the same feature
taxonomy as the [IDML reference](https://docs.paged.media). The detailed
engineering roadmap for the remaining gaps is tracked internally.

| Feature | Parsed | Rendered | Notes |
|---------|:------:|:--------:|-------|
| **Foundations & document open** | | | |
| `DOMVersion` capture | ✅ | n/a | Captured; the parser is deliberately version-agnostic. |
| `META-INF/container.xml` (UCF root) | — | — | Designmap read at the fixed path. |
| `<Section>` page numbering | ✅ | ✅ | Roman/alpha/Arabic + prefix labels; `Page Name` stays authoritative. |
| **Package anatomy** | | | |
| `Graphic` / `Styles` / `Preferences` parts | ✅ | ✅ | Loaded by fixed path. |
| `Resources/Fonts.xml` (`idPkg:Fonts`) | — | — | Fonts supplied to the renderer by the host. |
| Tagged XML (`XMLElement` / `XMLStory` / `Mapping`) | — | — | Backing-store skipped entirely. |
| **Layout, masters & layers** | | | |
| `ShowMasterItems` | ✅ | ✅ | Master items suppressed when false. |
| `MasterPageTransform` (full matrix) | ✅ | ✅ | Full affine applied at stamp time. |
| `Spread` / `MasterSpread` `ItemTransform` | ✅ | ◑ | Translation is faithful (cancels per-page); rotation/scale deferred. |
| Override-chain resolution | ✅ | ◑ | Single-item override exercised; long multi-master lists partial. |
| Nested layer groups (folders) | ✅ | ✅ | A child inside a hidden/locked group inherits that state. |
| Layer visible / printable gating | ✅ | ✅ | Hidden / non-printable items skipped. |
| **Stories & text** | | | |
| Knuth–Plass + dictionary hyphenation | ✅ | ✅ | total-fit composer; InDesign penalty calibration ongoing. |
| `NestedStyle` run splitting | ✅ | ✅ | Per-delimiter fragments restyled. |
| Conditional-text visibility | ✅ | ✅ | Hidden-condition runs filtered before layout. |
| `Position` (super/subscript), `VerticalScale` | ✅ | ✅ | Baseline shift + per-glyph y-scale applied. |
| Footnote bodies | ✅ | ◑ | Drawn at the frame bottom; space-reservation (overlap) + cross-frame flow pending — overflow is reported. |
| Overset (last-frame overflow) | ✅ | ◑ | Reported via diagnostics; trailing lines still clipped (matches InDesign). |
| Ruby | ✅ | ◑ | GroupRuby MVP drawn; per-character distribution pending. |
| CJK vertical writing | ✅ | — | `StoryDirection` captured; vertical layout not honoured. |
| In-story hyperlink regions | — | — | Not surfaced on runs. |
| **Typography** | | | |
| Optical kerning | ✅ | — | Falls back to Metrics. |
| Kinsoku CJK line-break rules | ✅ | — | Named sets parsed; hard-kinsoku heuristic only. |
| **Color & swatches** | | | |
| `Tint` on color swatches | ✅ | ✅ | TintValue applied. |
| `GradientStop` midpoint | ✅ | ✅ | Midpoint-skewed interpolation. |
| CMYK overprint | ✅ | ◑ | Plane-aware on the CPU backend; the RGB path approximates with Darken. |
| Mixed-ink / Lab primary / spot-without-CMYK | ✅ | — | RGB/CMYK fallback; no spectral model. |
| **Tables** | | | |
| Cell `VerticalJustification` | ✅ | ✅ | Top / Center / Bottom honoured. |
| Cell `RotationAngle` | ✅ | ✅ | Content rotated about the cell centre; borders unrotated. |
| Row + column dividers | ✅ | ✅ | Style-cascade dividers drawn; row strokes win at crossings. |
| Table / cell style cascade (strokes, fill) | ✅ | ◑ | Region defaults mostly applied; `StrokeOrder` precedence pending. |
| Table break across threaded frames | ✅ | ◑ | Common case; a row taller than a frame isn't split. |
| **Frames, paths & strokes** | | | |
| `GraphicLine` multi-segment / open paths | ✅ | ✅ | |
| `GraphicLine` arrowheads | ✅ | ◑ | Triangle / circle / bar drawn; size calibration + rarer shapes pending. |
| Decorative corner options | ✅ | ◑ | Rounded / Inverse / Bevel exact; Inset / Fancy approximate (calibration pending). |
| Dotted strokes | ✅ | ✅ | 12 variants incl. Japanese Dots + custom defs. |
| Striped / Wavy strokes | ✅ | — | Render as a solid stroke of the declared width. |
| AutoSizing growth | ✅ | ◑ | Width + height grow to fit text; visible-box stretch + wrap cascade pending. |
| **Images & graphics** | | | |
| Placed image (decode via `AssetResolver`) | ✅ | ✅ | Host supplies bytes; missing links → placeholder + diagnostic. |
| Vello (WebGPU) Image / Clip / blend groups | ✅ | ✅ | `DropShadow` / `BevelEmboss` approximated on Vello; CPU is the fidelity path. |
| EPS / PostScript decode | ✅ | — | Recognised; decode returns None (needs a Ghostscript-class sidecar). |
| **Diagnostics** | | | |
| Render diagnostics channel | ✅ | ✅ | Overset, missing/undecodable image, footnote overflow, section fallback reported on `BuiltDocument`. |
| **Companion formats** | | | |
| Snippets / libraries / story-only / assignments | — | — | Full-document (mimetype + designmap) entry only. |

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
