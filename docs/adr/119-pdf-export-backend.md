# ADR 119 — PDF export is a second backend over the display list

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-export-pdf`, the export session in `crates/paged-canvas/src/export.rs`, the glyph-run side-channel in `crates/paged-compose` and `crates/paged-renderer`, and the gate `corpus/generated/export-diff.sh`

## Context

The engine resolves every page into a display list
([ADR 101](101-display-list-single-intermediate.md)), which the rasterisers draw. Print
output needs a PDF in which text is still text, vectors are still vectors, images keep
their encoding, colours keep their spaces and transparency is not flattened.

The crate documentation states what the exporter is and what it preserves
(`crates/paged-export-pdf/src/lib.rs:17-26`). It does not say why the display list, and not
the document model, is the exporter's input, or why the writer was built directly on
`pdf-writer`; its one remark is that "`typst-pdf` is the reading reference, never a
dependency". For both questions: The repository does not record why.

## Decision

PDF export was built as a second consumer of the per-page display list, in the engine
crate `paged-export-pdf`, written directly on `pdf-writer`, `subsetter` and `xmp-writer`.

- **Text.** With `PipelineOptions::collect_glyph_runs` set, the build records a glyph-run
  table beside each list: per glyph outline command, the font, glyph id, transform, paint
  and character. The exporter skips the outline command and writes PDF text over subset
  CID fonts with `/ToUnicode`. The flag defaults to `false`. A font whose `fsType` forbids
  embedding stays as outlines and is reported.
- **Images and colour.** A placed JPEG passes through as `DCTDecode` without re-encoding;
  other images are embedded with Flate. Colour keeps `DeviceCMYK`, `ICCBased`,
  `Separation` and Lab. Transparency stays live; the conformance targets are `Pdf17` and
  `PdfX4`.
- **Effects.** Shadows, glows, satin and bevel are written as raster stamps: an 8-bit grey
  `/SMask` on a tinted image, the emitters in `effects.rs` each a port of the matching CPU
  rasteriser routine. The basic and directional feather instead put a luminosity soft mask
  over the object's own paint.
- **Determinism.** No wall-clock; object ids in walk order; XMP ids derived from content.
- **How it is reached.** `paged-canvas` links the crate. A host drives four messages:
  `ExportPdfBegin` builds the document once more with the glyph table on and parks a
  session, `ExportPdfPage` exports one page per call, `ExportPdfFinish` returns the bytes
  with diagnostics and findings, `ExportPdfCancel` drops the session. The CLI reaches the
  same session; a `paged-export` binary sits behind the crate's `cli` feature.

## Evidence

- `crates/paged-export-pdf/src/lib.rs:17-26` — the design statement; `:28-30` determinism; `:45-54` the two conformance targets
- `crates/paged-export-pdf/Cargo.toml:19-21`, `:33-36`, `:44` — `pdf-writer` 0.15, `subsetter` 0.2, `xmp-writer` 0.3; the `cli` feature that keeps `clap` out of wasm; `lopdf` as a test dependency
- `crates/paged-renderer/src/pipeline/mod.rs:283-287`, `crates/paged-compose/src/display_list.rs:1378-1384` — the glyph-run side-channel and its default
- `crates/paged-export-pdf/src/text.rs:15-19`, `crates/paged-export-pdf/src/image.rs:15-19` — text as text and the restricted-font rule; JPEG pass-through
- `crates/paged-export-pdf/src/effects.rs:17-34` — effects ported from the CPU rasteriser and their PDF encoding
- `crates/paged-canvas/src/export.rs:15-26` — the one-shot build and the per-page session; `crates/paged-canvas/src/channel.rs:1177-1191` the four messages
- `corpus/generated/export-diff.sh:4-21`, `.github/workflows/ci.yml:334-339`, `:434-472` — the gate and its hard and advisory steps
- `plugin-publish: packages/pdf-bundle/src/io/pdf.ts:114-121` — the PDF plugin registers an importer; its manifest lists no exporter

## Alternatives considered

- `typst-pdf` as a dependency: read as a reference, not linked (quoted above).
- A transparency flattener: not built; "transparency stays LIVE (PDF/X-4 — no flattener)"
  (`crates/paged-export-pdf/src/lib.rs:23-24`).
- The glyph table on every build: not done; the live canvas build "never pays for it and
  the command stream stays byte-identical" (`crates/paged-renderer/src/pipeline/mod.rs:285-286`).

## Consequences

Layout is decided once: the exporter reads the commands the rasterisers read. What it can
get wrong is its own encoding, and the gate measures that. Each generated fixture is
exported, rasterised by poppler and compared with the CPU render of the same scene (mean
ΔE at most 1.5, SSIM at least 0.93, with measured budgets for four fixtures). It is a
self-consistency check, "NOT the InDesign fidelity gate".

Export costs a second full build, because the live build's caches splice command ranges
and would break the index the glyph table relies on (`crates/paged-canvas/src/export.rs:16-21`).
A new display command needs an arm in the exporter's walk as well as in the rasterisers,
and an effect is implemented in the CPU rasteriser and again in the exporter. The PDF
plugin in `plugin-publish` ships no writer and registers no exporter.

Open points at this commit. The fixtures `effects`, `footnotes` and `swatches` run as
advisory, not as a hard gate (`.github/workflows/ci.yml:441`). The job's header comment
still says inner shadow, glows, bevel, satin and feather are not exported
(`.github/workflows/ci.yml:361-363`); `crates/paged-export-pdf/src/effects.rs` exports them. Gradient shadings
interpolate in `DeviceRGB` where the renderer interpolates in CMYK, so the `gradients`
fixture has its own ceiling (`corpus/generated/export-diff.sh:40-44`). A ligature maps to
its first character only (`crates/paged-compose/src/display_list.rs:1399-1402`). The gate
degrades to advisory when CI cannot fetch a CMYK profile.

## Related

- [ADR 101](101-display-list-single-intermediate.md), [ADR 100](100-two-rasterisers-one-trait.md) — the list and its other consumers
- [ADR 106](106-colour-resolved-at-build-time.md), [ADR 003](003-lcms2-color.md), [ADR 105](105-fidelity-gate.md) — the CMYK channels and the CMM the exporter uses; the InDesign fidelity gate, which this gate is not
- [ADR 017](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/017-importer-exporter-door-shape.md), [ADR 654](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/654-pdf-ir-and-mapper.md) — the plugin importer and exporter door; PDF import in `plugin-publish`
- `../design/pdf-export.md` — the design note for PDF export, with status notes where the code differs
