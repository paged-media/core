# ADR 154 — A placed PDF is drawn from its own drawing operations

- **Status:** Accepted 2026-10-14.
- **Scope:**
  - `paged-renderer`: `pipeline/placed_pdf.rs` (the interpreter device), `pipeline/images.rs`
    (`Placed::{Raster, Pdf}`), a `hayro-interpret` dependency.
  - `paged-gen`: `PdfPlacement` and the `<PDF>` writer, the `placed-pdf` sample and its
    `placed-pdf-Links/placed-art.pdf` asset.
  - The workspace's declared `rust-version` (1.80 → 1.92).
  - plugin-publish `idml-import`: a `<PDF>`, `<EPS>` or `<ImportedPage>` keeps its own
    `ItemTransform`, as an `<Image>` does.
  - No wire or protocol change.

## Context

A placed PDF drew nothing. The image pipeline decoded rasters only, so a frame holding a PDF fell
back to its own fill. The corpus has 41 placed PDFs in 5 documents, most of them Illustrator
`.ai` files (which are PDF-compatible), and 3 EPS.

A placed PDF reaches the engine two ways:
- linked, through a `<Link LinkResourceURI>` the asset resolver can open;
- embedded, as a `<PDF><Contents>` base64 payload (the importer's `has_inline_pdf`).

The importer also dropped the `ItemTransform` of every placed graphic that was not an `<Image>`,
so even a drawn PDF would have stood at the frame's origin at 100 %.

## The question to InDesign

`placed-pdf` (a `paged-gen` sample) places one 200 × 120 pt page of vector art four times on an
A4 page:
1. at 100 %, in a frame its own size;
2. at 50 %, in a 100 × 60 frame;
3. at 100 %, in a frame rotated 30°;
4. at 100 %, in a 120 × 80 frame that crops it, the art offset by (−40, −20).

The art (`make-placed-art.py`) holds the operations Illustrator writes most: a DeviceCMYK and a
DeviceRGB fill, a dashed stroke, a Bézier fill, a clipped rectangle, and a 50 % fill opacity
from an ExtGState. InDesign 20.0.1 exported the page with the link at
`/tmp/paged-placed-art/placed-art.pdf`.

## Decision

**Interpret, don't rasterise.**
- `hayro-interpret` 0.8 (Apache-2.0 OR MIT, pure Rust, builds for wasm32) walks the placed page
  and hands each operation to `ListDevice`, which writes it into the page's display list under
  the placement's transform:
  - paths become `FillPath` / `StrokePath`, with the stroke's width, cap, join, miter limit and
    dash;
  - clips become `PushClip` / `PopClip`;
  - transparency groups become `BeginBlendGroup` with their opacity and blend mode;
  - images become `Image` with decoded RGBA (and their soft-mask alpha);
  - glyph runs become their outlines, filled or stroked as their text render mode says.
- The placement therefore stays vector: on the CPU rasterizer, on Vello and in the PDF export.
  There is no resolution to choose and nothing to cache per zoom level.

**Placement.**
- The page's box (`render_dimensions`) is the placed graphic's natural size, as an image's pixel
  size is.
- The `<PDF>` element's `ItemTransform` maps the page's top-left-origin points into the frame,
  through the same path an `<Image>` takes, so scale, rotation, the frame's own transform and
  the frame's clip all apply unchanged.
- Bytes are recognised as PDF by `%PDF-` within their first KB (an Illustrator file may carry a
  header before it). Anything else still goes to the raster decoder.

**Colour.** hayro hands colours over converted to sRGB; the device takes them into linear light,
which the display list composites in.

**Cost in the browser.** Measured on the SDK wasm (`paged-sdk --features gpu`, wasm32 release,
`wasm-opt -Oz`, brotli q11), 2026-10-14:

| Build | `-Oz` | brotli |
|---|---|---|
| without hayro (main) | 9.91 MB | 2.95 MB |
| hayro with its base-14 fonts and CMaps | 13.25 MB | 4.01 MB |
| hayro without them (shipped) | 12.52 MB | 3.49 MB |

- The embedded fonts and predefined CMaps cost as much again as the interpreter, so they are
  native-only: `paged-renderer` takes `hayro-interpret` with no default features and adds
  `embed-fonts` / `embed-cmaps` for non-wasm targets only.
- In the browser, a placed PDF that names a base-14 font without embedding it, or a CID font
  through a predefined CMap, draws no text for those runs. Illustrator embeds or outlines its
  fonts, so the corpus placements are unaffected.
- The image decoders (JPEG 2000, JBIG2, CCITT) come with the interpreter and cannot be left out.
- The canvas wasm grows by about the same amount. Both stay well inside the editor's app-wide
  wasm budget.

**Toolchain.** hayro needs Rust 1.92. The toolchain is pinned at 1.94.1, so only the
workspace's declared `rust-version` moves (1.80 → 1.92). No crate is published to crates.io,
so nothing downstream reads it.

## Result

`placed-pdf` against InDesign's PDF: mean ΔE 0.111, p99 2.24, SSIM 0.9989, ink 1.66. All four
placements land where InDesign puts them; what remains is edge antialiasing. The export lane
(`export-diff.sh`) measures 0.011.

## Not yet

Each of these is drawn as stated, and recorded in `status.md`:
- **CMYK goes through hayro's RGB conversion**, not the document's CMYK profile, so a placed
  CMYK colour can differ from the same colour set in the document. Needs component access from
  the interpreter.
- **Even-odd fills and clips (`f*`, `W*`) are filled non-zero.** The display list has one fill
  rule. A compound path whose subpaths wind the same way loses its holes.
- **Patterns and shadings paint mid-grey.**
- **Soft masks on transparency groups are ignored**; the group's opacity and blend apply.
- **Stencil images (`ImageMask`) and Type 3 glyphs are skipped.**
- **The first page is drawn.** `PDFAttribute@PageNumber` and `@PDFCrop` are not read; the page's
  crop box is used.
- **EPS is not interpreted.** An Illustrator EPS that carries PDF-compatible data could take
  this path later (plan item D-11b).
