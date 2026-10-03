# Paged — Concept 3: PDF Export

June 2026. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

Sections not relevant outside the original planning context have been removed; numbering is unchanged.

*Status note (2026-10-02): source comments in this repository cite this document as "Concept 3" and [colour-and-swatches.md](colour-and-swatches.md) as "Concept 2", with the decision ids (E1 to E9) used below. "Concept 1" is a toolbar concept for the editor and is not in this repository. The exporter was built as `crates/paged-export-pdf` and diverged from this text in two ways: it is written directly on `pdf-writer` and has no `krilla` dependency, and there is no `SceneConsumer` trait; it consumes the display list. [ADR 119](../adr/119-pdf-export-backend.md) records what was built.*

*Subtitle: colour-managed PDF export with ICC profiles and PDF/X conformance.*

---

## Scope

This concept covers print-grade PDF export: a second rendering backend over the resolved scene, the writer library, colour-space encoding, the transparency model, fonts, placed images, marks and boxes, PDF/X conformance, and the invocation/runtime path. It also sketches how the same backend generalises to other output formats.

In scope:

- The "second backend over the resolved scene" architecture and the `SceneConsumer` trait it implies.
- The writer choice (krilla on `pdf-writer`) and the boundary where the high-level crate runs out.
- Colour encoding in PDF: `ICCBased`, `Separation`, `DeviceN`, `Lab`, `DeviceCMYK/RGB/Gray`, and the OutputIntent — driven by Concept 2's CMM in export-convert mode.
- The transparency model: groups, blend modes, soft masks, knockout/isolation, multi-level opacity, and the InDesign-effects → PDF mapping.
- Fonts: subsetting, CID-keyed embedding, `ToUnicode`, embedding-rights handling.
- Placed images and vector art: passthrough vs re-encode, ICC preservation, downsampling, clipping.
- Boxes and marks: MediaBox/CropBox/BleedBox/TrimBox/ArtBox, registration/crop/bleed marks, colour bars, page info.
- PDF/X levels (X-1a/X-3/X-4), the recommendation to target X-4 first, and the flattener boundary for X-1a.
- Invocation: the export command, the options composition, the background-task path, and the fact that export is a `request`, not a `mutate`.
- Generalisation to SVG / raster / IDML / interactive-PDF backends.

Out of scope:

- The transparency *flattener* implementation (needed for X-1a) — defined as a boundary here, built with `paged.flattener-preview` later.
- Imposition / printer spreads / booklet — a production add-on beyond document export.
- The colour *model and engine* themselves — Concept 2, which this concept consumes.

## Position in the architecture — and why it isn't a panel

*Status note (2026-10-02): this section predates the implementation; no `SceneConsumer` trait exists. The exporter walks `paged_compose::DisplayList`, the same per-page display list the rasterisers consume (`crates/paged-export-pdf/src/page.rs`). See [ADR 119](../adr/119-pdf-export-backend.md) and [ADR 101](../adr/101-display-list-single-intermediate.md).*

The panel catalogue deliberately has no `paged.export` panel; the nearest entries are `paged.background-tasks` (async progress), `paged.preflight`, and `paged.separations-preview`/`paged.flattener-preview` (view-only). That is correct: **export is not a panel, it is an output backend.** The central architectural claim:

> PDF export is a **second backend over the same resolved scene that Vello consumes** — parallel to the WebGPU renderer, not downstream of it.

The four-tier pipeline (content → per-story layout → resolution → per-page output) already produces a resolved, positioned scene: glyphs with positions, frames with geometry, strokes, fills, effects, placed-asset references. Vello turns that scene into GPU draws. The PDF exporter turns the **same** scene into PDF content streams. They are sibling consumers of one resolved representation.

```
                 four-tier pipeline (resolved scene, per page)
                          /            |            \
                  Vello / WebGPU   paged-export-pdf   (future: SVG, raster, IDML)
                 (screen pixels)  (content streams)
```

The payoff: export reuses layout, font shaping, and the colour engine wholesale. It does **not** re-implement layout, and it does **not** rasterise — text stays text, vectors stay vectors, placed images stay images. The right abstraction is a small trait:

```rust
/// The contract a backend implements over the resolved per-page scene.
pub trait SceneConsumer {
    fn begin_page(&mut self, page: &ResolvedPage);
    fn fill_path(&mut self, path: &Path, paint: &Paint, transform: Affine);
    fn stroke_path(&mut self, path: &Path, stroke: &Stroke, paint: &Paint, transform: Affine);
    fn draw_glyphs(&mut self, run: &GlyphRun, paint: &Paint, transform: Affine);
    fn draw_image(&mut self, image: &PlacedImage, transform: Affine);
    fn begin_group(&mut self, group: &TransparencyGroup);  // blend/opacity/knockout/isolation
    fn end_group(&mut self);
    fn end_page(&mut self);
}
```

Vello is one implementer; `paged-export-pdf` is the first non-Vello implementer. This is the same "one resolved scene, many consumers" discipline the renderer already follows internally.

## Coordinate systems

One foot-gun to settle up front: IDML/InDesign space is y-down with the origin at the top-left of the page (points); PDF user space is y-up with the origin at the bottom-left. The exporter applies a single page-level flip (`y' = pageHeight − y`) plus the per-object affine, so every emitted path/glyph/image lands correctly without per-primitive sign juggling. Units are points throughout (the renderer is already pt-based), so there is no unit conversion, only the axis flip and box offsets for bleed.

## The writer: krilla on pdf-writer

*Status note (2026-10-02): this section predates the implementation; `krilla` is not a dependency. The exporter is built directly on `pdf-writer`, with `subsetter` for fonts and `xmp-writer` for the conformance metadata (`crates/paged-export-pdf/Cargo.toml`). See [ADR 119](../adr/119-pdf-export-backend.md).*

For Rust → PDF, **krilla** is the right high-level base. It is built on `pdf-writer` (the low-level crate from the Typst project), abstracts the plumbing behind fills/strokes/gradients/glyphs/images, and is the backend Typst itself is moving to. Capabilities it already carries that matter here: **ICC profile embedding (including custom CMYK), font embedding and subsetting, gradients with transparency, image XObjects, alpha/luminosity masks, outlines/destinations, and tagged-PDF plus PDF/A & PDF/UA export modes.** It compiles as a normal Rust crate and runs in the worker.

The honest boundary: krilla's standards focus has been **PDF/A and PDF/UA** (archival and accessibility). Full **PDF/X** output-intent conformance (the *print* standard) may not be turnkey. Where krilla's high-level surface does not yet cover a PDF/X requirement — the `GTS_PDFX` OutputIntent dictionary, `Separation`/`DeviceN` colourants with custom tint transforms, the PDF/X XMP keys, the `Trapped` flag — `pdf-writer` is the escape hatch underneath, and krilla does not hide it. Plan for **krilla for ~90% (pages, text, vectors, images, transparency, ICC), `pdf-writer` for the PDF/X conformance edges**, with `crates/typst-pdf` as the reading reference for how to drive krilla over a real document (see Prior art).

## Colour in PDF — preserve native spaces, don't collapse to display

This is where **Concept 2's CMM** pays off a second time, used in `convert_for_export` mode (preserve, don't collapse). The encodings:

- **Process CMYK** → `DeviceCMYK`, or better an **`ICCBased`** stream (`/N 4`) tagged with the document's CMYK working-space profile, so a conformant reader colour-manages it.
- **RGB** → **`ICCBased`** (`/N 3`) with the RGB working-space profile (X-3/X-4 readers colour-manage it), or converted-to-destination if the user chooses that policy.
- **Lab** (including every freieFarbe HLC swatch) → PDF **`Lab`** colour space (`[/Lab << /WhitePoint … /Range … >>]`), device-independent — survives to print exactly as authored.
- **Spot colours** → **`Separation`** colour space: `[/Separation /InkName altSpace tintTransform]`, where `altSpace` is the swatch's alternate (process or Lab) and `tintTransform` is a function mapping tint 0..1 to alternate components. A 50% tint of a spot is then a single `sc` operand, not an opacity.
- **Mixed inks** → **`DeviceN`**: `[/DeviceN [/Ink1 /Ink2 …] altSpace tintTransform attributes]`, one channel per spot.
- **Registration** → the `All` colorant (`DeviceN` with the reserved `/All` name) so crop/registration marks hit every plate.
- **OutputIntent** → embed the destination profile so the file declares its press condition: `/OutputIntents [ << /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (FOGRA39) /DestOutputProfile <profile stream> >> ]`. PDF/X *requires* this.

The two colour policies mirror InDesign's, both implemented via the CMM:

- **Preserve Numbers** — leave CMYK numbers untouched, only tag with the working-space profile. The default. The must-not-break case — pure 100% K text staying 100% K, not becoming rich black — falls out of this being the default for CMYK.
- **Convert to Destination** — run everything through the CMM to the output-intent space (used when the document mixes RGB into a CMYK destination, or when flattening to a single space is required).

The Ink Manager settings from Concept 2 (spot→process, aliasing, standard-Lab-for-spots) are honoured here: an aliased spot collapses to its target colorant; "all spots to process" converts every `Separation` to the destination CMYK.

## The transparency model

PDF 1.4+ has a native transparency model, and **InDesign's effects map onto it almost one-to-one** — which is the whole reason to target PDF/X-4 (live transparency) rather than flattening. The mapping (`paged.effects` → PDF):

| InDesign / `paged.effects` | PDF construct |
| -------------------------- | ------------- |
| Object / fill / stroke / text opacity | constant alpha (`/ca`, `/CA`) at the right graphics-state scope |
| Blend mode (Multiply, Screen, Overlay, …) | `/BM` blend mode — the 16 PDF modes match InDesign's set |
| Drop shadow / inner shadow | a blurred, offset copy painted through a **soft mask** (`/SMask` luminosity) |
| Outer/inner glow, feather (basic/directional/gradient) | **soft mask** from the feathered alpha |
| Bevel, satin | composited fills with blend + mask |
| Transparency group (object group with effects) | a **transparency group XObject** (`/Group << /S /Transparency >>`) |
| Knockout group | group with `/K true` |
| Isolated blending | group with `/I true` |

So the exporter emits graphics-state dictionaries for opacity/blend, transparency-group XObjects for grouped effects, and soft-mask XObjects for shadows/glows/feather. None of it rasterises under X-4. The gradient-feather *angle handle* is an authoring gesture (Concept 1); at export it is just a gradient soft mask.

## PDF/X levels — recommend X-4 first

| Standard | ISO | Transparency | Colour | Paged fit |
| -------- | --- | ------------ | ------ | --------- |
| PDF/X-1a | 15930-1/-4 | must be **flattened** | CMYK + spot only; no RGB, no live ICC | needs the flattener; defer |
| PDF/X-3 | 15930-3/-6 | flattened | CMYK + spot + ICC-tagged RGB | superseded by X-4 |
| **PDF/X-4** | 15930-7 | **live** (PDF 1.6) | CMYK + spot + ICC + layers (OCGs) | **target first** |

**Target PDF/X-4 first.** Paged's effects map onto live PDF transparency (above); X-4 keeps all of it live, no flattening, with ICC colour management and optional layers. Conformance is declared in **XMP** (`pdfxid:GTS_PDFXVersion = "PDF/X-4"`), the document must carry an OutputIntent, the `Trapped` key must be `True` or `False` (never `Unknown`), and a `TrimBox` (or `ArtBox`) is mandatory on every page.

**PDF/X-1a comes later** because it forbids live transparency and therefore requires a **transparency flattener** — the boundary defined below — which is exactly what `paged.flattener-preview` previews. Ship the flattener and the X-1a path together.

## The transparency flattener (the X-1a boundary)

Flattening decomposes overlapping transparent regions into opaque atomic regions, keeping vectors where possible and rasterising only where blends cannot be expressed opaquely. It is non-trivial (atomic-region computation, vector/raster split decisions, flattener-resolution presets, text-on-transparency handling) and is **deliberately deferred**. The boundary: the `SceneConsumer` for X-1a wraps the live one with a flattening pass that runs over the resolved scene before emission. `paged.flattener-preview` renders the same pass's region classification (what stays vector, what rasterises) as a view-only overlay. Nothing in the X-4 path depends on it.

## Fonts

*Status note (2026-10-02): this section predates the implementation; fonts are subset with the `subsetter` crate, not through krilla (`crates/paged-export-pdf/src/text.rs`), and shaping uses `harfrust`, not `rustybuzz` ([ADR 102](../adr/102-text-stack.md)).*

- **Subset and embed.** The renderer already owns font data (the Inter side-channel + font handling); krilla subsets to embedded **CID-keyed Type0** fonts (CFF or TrueType outlines), with the conventional 6-letter subset tag. Subset, never full-embed, for size.
- **`ToUnicode` CMaps.** Emit them so text is copyable and searchable, and so tagged-PDF/accessibility works. Without `ToUnicode`, the file is a picture of text.
- **Embedding rights.** Honour the font's `fsType` bits. If a font forbids embedding, surface it in preflight rather than silently shipping a non-compliant file; offer substitution or outlining as an explicit, user-chosen fallback. (For print, outlining text is a last resort — it kills `ToUnicode` and bloats the file.)
- **OpenType features** already resolved at layout time (the shaping ran through `rustybuzz`); the exporter emits positioned glyph IDs, so features need no re-resolution at export.

## Placed images and vector art

The quality differentiators live here:

- **Raster passthrough.** A placed JPEG embeds as a `DCTDecode` image XObject **without re-encoding** (no generational loss). TIFF/PSD/PNG embed as `FlateDecode` (or DCT if the user opts for lossy). Preserve any **embedded ICC profile** on the image (`/ColorSpace [/ICCBased …]`) — the `paged.links` panel already tracks each link's colour space, so the exporter carries it through rather than re-converting.
- **Downsampling.** Offer the usual targets (e.g. 300 ppi for colour/grey, 1200 ppi for bitmap) with bicubic resampling, applied only when the effective ppi exceeds the threshold — an export-option, off by default for "preserve".
- **Clipping & alpha.** Honour the frame's clip path and the image's alpha/clipping path (soft mask or hard clip).
- **Vector placed art.** A placed PDF/EPS/AI embeds as a **Form XObject by passthrough** — reuse the placed PDF's own content rather than rasterising. This keeps placed logos and charts resolution-independent and is a major fidelity win over tools that flatten placed art.

## Boxes and marks

- **Boxes per page:** `MediaBox` (the sheet, incl. bleed + slug + marks), `CropBox`, `BleedBox` (trim + bleed), `TrimBox` (finished page — mandatory for PDF/X), `ArtBox` (optional).
- **Printer's marks:** crop marks, registration marks (in the `All` colorant so they hit every plate), bleed marks, colour bars, page-information slug — with configurable weight and offset, drawn outside the trim into the bleed/slug area.
- These are emitted by the exporter directly (not part of the document scene), so they never pollute the resolved scene the canvas renders.

## How it's invoked and where it runs

*Status note (2026-10-02): this section predates the implementation; the worker side is an export session of four messages, `ExportPdfBegin`, `ExportPdfPage`, `ExportPdfFinish` and `ExportPdfCancel` (`crates/paged-canvas/src/channel.rs`, `crates/paged-canvas/src/export.rs`). The command, the options dialog and the progress panel belong to the editor repository.*

Export is heavy and belongs in the worker (the same Rust WASM module, or a dedicated export entry the worker calls). The plumbing reuses existing constructs cleanly, with no new SDK surface:

- **A command** `paged.file.exportPdf` in the `CommandRegistry`, reachable from the palette and a menu later.
- **An options dialog** — a composition popover/dialog: intent (Print vs Interactive), PDF/X level, output-intent profile, colour policy (Preserve/Convert), page range, spreads-vs-pages, marks & bleed, image downsampling/compression, font-embedding fallback. Pure composition; no new binding kind.
- **Progress** via `paged.background-tasks` — the panel catalogue already places that panel outside the document read model, reading the client's task queue; export reports per-page progress and is cancellable.
- **Export is a `request`, not a `mutate`.** It reads the resolved document and returns bytes; it never goes through `paged.mutate` and touches no `Operation`. This is a read-shaped output path, like snapshot fetching. The bytes come back over the worker bridge and the shell offers a download (or hands to a future "package/upload" path).

For **determinism**, fix the XMP timestamps to the document's own metadata (not wall-clock) where the standard allows, so the same input yields the same bytes — invaluable for golden-file tests, the same discipline Typst uses.

## Generalising the backend

*Status note (2026-10-02): this section predates the implementation; no `SceneConsumer` trait exists (see [ADR 119](../adr/119-pdf-export-backend.md)).*

Because export is a `SceneConsumer`, the architecture pays forward:

- **SVG export** — a `SceneConsumer` emitting SVG; near-free given the same scene walk.
- **Raster export (PNG/JPEG)** — reuse the Vello raster path at a chosen ppi; it is already a `SceneConsumer`.
- **IDML round-trip** — the document model (not the resolved scene) serialises back to IDML; orthogonal but in the same "output" family.
- **Interactive PDF** — a *second PDF mode*, not PDF/X: bookmarks (`paged.bookmarks`), hyperlinks (`paged.hyperlinks`), buttons/forms and media, page transitions, RGB colour, tagged structure. The options dialog's "intent: Print vs Interactive" switch selects which feature set and which conformance the same krilla backend targets. Print (PDF/X) is v1; Interactive is a fast follow once those panels exist.

## What not to do

- **Don't rasterise the page.** Text stays text, vectors stay vectors, placed images stay images. Rasterising is the difference between a real DTP export and a screenshot.
- **Don't re-encode placed JPEGs.** Passthrough `DCTDecode`; re-encoding is generational loss for no benefit.
- **Don't flatten transparency for X-4.** Emit live groups/soft-masks. Flattening is only for the deferred X-1a path.
- **Don't convert CMYK numbers by default.** Preserve Numbers is the default; pure-K must stay pure-K.
- **Don't strip embedded image ICC profiles.** Carry them through; the link panel already knows the space.
- **Don't omit `ToUnicode`.** A PDF without it is unsearchable and inaccessible.
- **Don't outline text silently to dodge embedding restrictions.** Surface it in preflight; make outlining an explicit user choice.
- **Don't route export through `paged.mutate`.** It is a read-shaped `request`; it mutates nothing and needs no Operation.
- **Don't build the flattener for v1.** Target X-4; define the flattener boundary and defer it with `paged.flattener-preview`.
- **Don't reimplement colour conversion.** Use Concept 2's CMM in export-convert mode.

## Acceptance criteria

1. A multi-page document with text, vectors, gradients, placed raster and placed-PDF art, spot colours, and transparency effects exports to a valid PDF/X-4 that opens cleanly in Acrobat with no rasterisation of text or vectors.
2. Text in the output is selectable and searchable (correct `ToUnicode`); fonts are subset and embedded.
3. A 100%-K text run exports as 100% K (Preserve Numbers default), verified in Acrobat's Output Preview / separations.
4. A spot colour exports as a `Separation` plate (not converted to process unless the Ink Manager says so); a 50% tint shows as 50% on that one plate.
5. A placed JPEG embeds without re-encoding; its embedded ICC profile is preserved; a placed PDF embeds as a Form XObject, not a raster.
6. Transparency effects (drop shadow, multiply blend, opacity) appear live in the PDF and survive Acrobat's transparency flattener preview without surprises.
7. The file carries a valid OutputIntent with an embedded destination profile, a `TrimBox` on every page, and `Trapped` set to a definite value; Acrobat's PDF/X preflight passes X-4.
8. Export runs in the worker, reports per-page progress through `paged.background-tasks`, is cancellable, and returns bytes without mutating the document.
9. The same document exported twice yields byte-identical output (determinism).
10. The PDF backend is a `SceneConsumer`; the scene walk it consumes is the same one Vello consumes (no parallel layout path).

## Decision triggers

1. **After the krilla skeleton + text, before colour.** Confirm krilla's surface covers what's needed and locate precisely where `pdf-writer` is required; if the PDF/X edges are larger than expected, that reshapes the budget now.
2. **When colour encoding meets Concept 2's CMM.** This is the real test of the engine's preserve-native-spaces contract; refine `convert_for_export` against `Separation`/`DeviceN` needs.
3. **When transparency lands.** Validate the effects→PDF mapping against the messiest real case (a gradient-feathered group with a blend mode over a spot fill). If it survives that, X-4 is sound.
4. **When X-1a is requested.** Only then build the flattener (with `paged.flattener-preview`); decide raster-fallback resolution and text-on-transparency policy at that point, not before.

## Decisions register

*Status note (2026-10-02): the statuses in the table are those of the draft. E2 was built differently (`pdf-writer` only, no krilla), and the `SceneConsumer` trait of E1 and E9 was not built; see [ADR 119](../adr/119-pdf-export-backend.md).*

| # | Decision | Status |
| - | -------- | ------ |
| E1 | PDF export is a second backend (`SceneConsumer`) over the four-tier resolved scene, sibling to Vello. No rasterisation; text/vectors/images stay native. | Proposed |
| E2 | `paged-export-pdf` built on **krilla** (on `pdf-writer`); drop to `pdf-writer` for PDF/X OutputIntent, `Separation`/`DeviceN`, XMP, and `Trapped` edges krilla doesn't cover. | Proposed |
| E3 | Reuse Concept 2's CMM in export-convert mode. Preserve native spaces (CMYK→DeviceCMYK/ICCBased, spot→Separation, mixed→DeviceN, Lab→PDF Lab); embed OutputIntent; honour Ink Manager. | Proposed |
| E4 | Target **PDF/X-4 first** (live transparency, ICC). X-1a + the transparency flattener ship together with `paged.flattener-preview`, later. | Proposed |
| E5 | Default CMYK policy is **Preserve Numbers** (pure-K stays pure-K); Convert-to-Destination is opt-in via the options dialog. | Proposed |
| E6 | Export is a worker `request` returning bytes — not a `mutate`. Command `paged.file.exportPdf`; options are a composition; progress via `paged.background-tasks`; cancellable; deterministic output. | Proposed |
| E7 | Placed raster passes through without re-encoding (DCT) and preserves embedded ICC; placed PDF/EPS/AI embeds as a Form XObject, never rasterised. | Proposed |
| E8 | Emit `ToUnicode`, subset/CID-embed fonts, honour `fsType`; outlining text is an explicit user-chosen fallback surfaced in preflight, never silent. | Proposed |
| E9 | The backend is a `SceneConsumer`, generalising to SVG/raster/IDML and to a second **Interactive PDF** mode (RGB + bookmarks/links/forms/media) selected by export intent. | Proposed |

## Prior art / reference implementations

*Status note (2026-10-02): this section predates the implementation; krilla is not a dependency ([ADR 119](../adr/119-pdf-export-backend.md)), salsa was not adopted ([ADR 107](../adr/107-whole-document-build.md)), and shaping uses `harfrust` ([ADR 102](../adr/102-text-stack.md)). The module comment of `crates/paged-export-pdf/src/lib.rs` keeps `typst-pdf` as "the reading reference, never a dependency".*

The Rust ecosystem already has a large-scale production user of nearly the exact stack this concept rests on: **Typst** (`github.com/typst/typst`). Treat it as a reference implementation and a validation of the leaf technologies — with one sharp caveat about *which* parts transfer.

**Borrow the leaves, not the trunk.** Typst is a *batch compiler*: immutable source markup → layout → PDF, recompiled (incrementally) on change. It has no mutable scene graph, no gesture mutations, no `paged.mutate`/Operation model, no IDML round-trip. Paged's four-tier incremental pipeline with salsa and per-story layout is the right model for an *interactive* editor; Typst's `comemo`-memoized batch model is the right model for a *compiler*. So the overall architecture does not transfer — only the leaf technology does.

| Paged concern | Typst reference | What to take |
| ------------- | --------------- | ------------ |
| **PDF export (E2)** | `crates/typst-pdf` (drives **krilla**, which came out of the Typst project, same author, on Typst's `pdf-writer`) | The readable reference for driving krilla over a real document model — colour encoding, font subsetting, transparency groups, image XObjects. Read this *before* krilla's own examples. De-risks E2. |
| **Text stack** | `rustybuzz` (HarfBuzz shaping), `ttf-parser`, `unicode-bidi` | Align the renderer, layout, and export on the font types krilla expects, so all three speak one vocabulary. |
| **Incrementality** | `comemo` (memoized pure functions) | Not to adopt — to corroborate. Typst proves incremental layout in Rust holds up; Paged's **salsa** choice is a deliberate divergence (a mutable-document editor wants query-style invalidation, not pure-function memoization). |
| **Line breaking** | Typst's Knuth-Plass implementation | A readable reference for the algorithm Paged already checkpoints, rather than re-deriving it. |
| **Determinism** | Typst's reproducible-output discipline | Fix XMP timestamps to document metadata so the same input yields the same bytes; enables golden-file tests. |

**Where Typst is a *weak* reference — and it's exactly the hard part.** Typst is a document compiler, not a prepress tool. Its colour story is screen/oklab-leaning, and spot colours as `Separation`, mixed inks as `DeviceN`, ICC **OutputIntent**s, and **PDF/X** conformance are not its priorities. That is precisely the krilla edge in **E2** where Paged drops to `pdf-writer` and hand-writes the conformance dictionaries. Net: Typst shows ~90% of how to drive krilla; the prepress 10% Paged owns either way.

**Dependency posture.** Paged depends on **krilla + pdf-writer**, not on Typst the application. Typst is read as a guide and cited as prior art.

## How this fits with the other two

- **This concept reuses everything upstream** — the resolved scene from the pipeline, the fonts from the renderer, the link colour spaces from `paged.links`, and above all the **CMM from Concept 2** (export-convert mode) and the **Ink Manager** settings.
- **Suggested position in the build sequence: last.** It leans on the resolved scene, the colour engine (Concept 2), and the font stack. Build `paged-color` first, then the toolbar (Concept 1), then this — so export stands on a finished colour engine rather than re-deriving colour conversion.
