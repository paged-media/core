# IDML Faithful Renderer

Pixel-faithful renderer for Adobe IDML documents, built in Rust.

See `idea.md` for the full technical specification and
`/root/.claude/plans/do-a-deep-research-humming-wigderson.md` for the
development strategy this repository implements.

## Current pipeline

```
idml bytes
 └─► idml-parse              ZIP + designmap + spread + story + graphic
      └─► idml-scene         owned Document (palette, spreads, stories,
                              frame-for-story index)
           └─► idml-text     rustybuzz shape + Knuth-Plass compose +
                              aligned layout (Left / Right / Center / Justify)
                 └─► idml-compose  DisplayCommand { FillPath | StrokePath },
                                    glyph outlines (ttf-parser), unit-rect
                                    + glyph path interning
                       └─► idml-gpu   tiny-skia CPU rasterizer (feature:
                                       "cpu", default-on); Vello backend
                                       placeholder for Spike A
                             └─► RgbaImage
                                   └─► image PNG encode
                                   └─► idml-fidelity  ΔE2000 + SSIM diff
                                                       against a reference
```

The pipeline is exposed as two library functions:

```rust
use idml_renderer::{Document, pipeline, PipelineOptions};

let document = Document::open(&idml_bytes)?;
let opts = PipelineOptions::default();

// Display list only.
let built = pipeline::build(&document, &opts)?;

// Display list + rasterised image (cpu feature).
let (built, image) = pipeline::render(&document, &opts, 144.0, Color::WHITE)?;
```

## Workspace layout

```
crates/
├── idml-parse/       ZIP + XML → typed AST (container, designmap,
│                     spread, story, graphic)
├── idml-scene/       Owned Document: parsed container, palette,
│                     spreads, stories, frame-for-story index
├── idml-text/        Shape (rustybuzz) + compose (Knuth-Plass) +
│                     layout (alignment, positioned glyphs)
├── idml-color/       ICC transforms placeholder (lcms2 non-wasm)
├── idml-compose/     Display list, path buffer, emit primitives
│                     (emit_rect, emit_stroke_rect, emit_paragraph),
│                     glyph outlining via ttf-parser
├── idml-gpu/         PathRasterizer trait + cpu backend (tiny-skia);
│                     Vello backend gated behind vello-backend feature
├── idml-renderer/    Top-level library (`pipeline::build`,
│                     `pipeline::render`) + `idml-inspect` CLI
├── idml-fidelity/    ΔE2000 + SSIM diff harness + `idml-diff` CLI
└── idml-wasm/        wasm-bindgen surface: `render_to_png`,
                      `parse_summary`

spikes/
├── vello-eval/             Spike A harness (GPU feature coverage)
├── composer-calibration/   Spike B harness (InDesign line-break parity)
└── wasm-size/              Spike C harness (compressed artefact size)

corpus/
└── seeds/           Golden IDMLs + reference PDFs (to be populated)

.github/workflows/   ci.yml + fidelity.yml
```

## CLI tools

- **`idml-inspect <file.idml>`** — parse a container, walk spreads and
  stories, print a human-readable summary.
  Flags: `--font <path>`, `--display-list`, `--render <out.png>`,
  `--dpi <n>`, `--column-width-pt <n>`, `--default-size <n>`.
- **`idml-diff <reference.png> <candidate.png>`** — ΔE2000 + SSIM
  report against the §13.2 pass criteria (mean ΔE ≤ 1.0, p99 ΔE ≤ 2.5,
  SSIM ≥ 0.99). Exits 0 on pass, 1 on fail.

## IDML features supported today

- Container: ZIP + mimetype validation + sub-resource access
- `designmap.xml` manifest: spreads, stories, master-spread refs
- `Resources/Graphic.xml`: `<Color>` + `<Swatch>` + one-level
  indirection; CMYK / RGB / Gray → linear RGB (naive, non-ICC)
- `Spread_*.xml`: `<Page>` + `<TextFrame>` + `<Rectangle>` +
  `ItemTransform`, `GeometricBounds`, `FillColor`, `StrokeColor`,
  `StrokeWeight`; nested-in-`<Group>` frames surfaced as skipped
- `Story_*.xml`: `<ParagraphStyleRange>` (+ `Justification`,
  `FirstLineIndent`, `SpaceBefore`, `SpaceAfter`) wrapping
  `<CharacterStyleRange>` (+ `AppliedFont`, `FontStyle`, `PointSize`,
  `FillColor`, `Tracking`) wrapping `<Content>` and `<Br/>`
- Paragraph alignment: Left / Right / Center / Justify
  (last-line-left)
- Per-run fill colour picker (cluster → Paint)
- FillPath + StrokePath commands; glyph + rect path sharing via
  interned `PathBuffer`

## Verification

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./spikes/wasm-size/measure.sh          # requires binaryen + brotli
cargo check --target wasm32-unknown-unknown -p idml-wasm
```

All green at the latest commit on the current branch.

## What is **not** yet implemented

The idea.md spec calls for a multi-year effort (3–5 engineers + 1
typographer over 36–39 months). This repository is the scaffolding +
first slice — substantial, but far short of a print-grade renderer.
Material items remaining include:

- **Spike A** — Vello feature-coverage evaluation needs real execution.
  The harness is in place; running it requires a GPU or software
  Vulkan (lavapipe).
- **Spike B** — Paragraph Composer calibration against InDesign
  output. Needs an InDesign-exported reference corpus; the 95%
  line-break parity gate is unpaved.
- **Spike C** — WASM size measurement script needs binaryen + brotli
  installed to run. The 3.5 MB pass criterion is not yet verified.
- **ICC colour management** — naive CMYK/RGB → linear RGB today;
  `idml-color` wraps `lcms2` natively but the pipeline doesn't use it.
- **Effects** — drop shadow, feather, glow, blend modes other than
  source-over are not implemented.
- **Text** — no justification of metric-kerning ratios, no
  hyphenation, no drop caps / nested styles / GREP styles / tables /
  footnotes / text-on-path / CJK composition (§8.5).
- **Images, gradients, spot colours** — `<Oval>`, `<Polygon>`,
  `<GraphicLine>`, gradients, placed images, `<PageItem>` trees
  inside groups.
- **Master spreads** — referenced from the manifest, ignored today.
- **Font resolver** — hosts pass bytes directly; the spec calls for
  an async `AssetResolver` interface.
- **Multi-page output** — the pipeline unions all pages into one
  canvas; per-page rasterisation is a natural extension.
- **Fidelity corpus** — the diff harness works; the 500-document
  corpus is empty.
- **CI on real corpus** — workflow exists but gates nothing until the
  corpus is populated.

See idea.md §17 for the full risk register and §16 for the phased
roadmap. The plan document at
`/root/.claude/plans/do-a-deep-research-humming-wigderson.md`
captures the de-risking sequence that this repository implements
through Phase 0 foundations.

## Branch

All development lands on `claude/read-idea-file-vHroZ` per the
session-branch requirement.
