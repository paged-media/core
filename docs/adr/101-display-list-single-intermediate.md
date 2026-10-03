# ADR 101 — The display list is the single intermediate; a digest proves two builds produce the same scene

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-compose` (`display_list.rs`, `extent.rs`, `text.rs`) and every crate that consumes a `DisplayList`: `paged-gpu`, `paged-export-pdf`, `paged-canvas`, `paged-sdk`, `paged-cli`

## Context

Layout produces drawing for three back ends: the tiny-skia rasteriser, the Vello scene
builder ([ADR 100](100-two-rasterisers-one-trait.md)) and the PDF exporter
([ADR 119](119-pdf-export-backend.md)). The module doc of `display_list.rs` describes the
shape of the hand-off: "A flat command stream plus a path buffer", where the path buffer
"lets repeated shapes (especially glyphs) share tessellated data"
(`crates/paged-compose/src/display_list.rs:15-20`).

Several builds must produce the same drawing: the viewer SDK's load path and the stock
pipeline, an incremental rebuild and a cold one
([ADR 027](027-incremental-flow-invalidation.md)), a document reloaded from its native part
and the document it was saved from. The `digest` doc comment names the first of these as
its purpose: the "same code, same scene" tripwire (`crates/paged-compose/src/display_list.rs:1593-1597`).

Text is emitted as "one `FillPath` command per glyph" (`crates/paged-compose/src/text.rs:17`).
Glyphs are filled paths and not a text primitive. The repository does not record why.

## Decision

`paged_compose::DisplayList` is the one hand-off between layout and every drawing back end.
It is a flat `Vec<DisplayCommand>` plus pools the commands index into: an interning
`PathBuffer`, three gradient pools, decoded images and named spot inks.

- **Glyphs are interned paths.** Each glyph outline is stored once under a
  `(font_id, glyph_id)` key and referenced by `PathId` from one `FillPath` per glyph.
- **Side channels are opt-in.** `glyph_runs` and `link_regions` are `None` unless the build
  asks for them. The PDF exporter uses them for real text and link annotations. No
  rasteriser reads `link_regions`; the CPU rasteriser reads `glyph_runs` only when
  `snap_glyph_origins` is set, to pick the glyph fills it snaps
  (`crates/paged-gpu/src/cpu.rs:1046-1052`). The field comments still say "rasterizers
  never read it".
- **Scopes are bracket markers in the same stream.** `PushClip`/`PopClip`,
  `BeginBlendGroup`/`EndBlendGroup`, `PushLayer`/`PopLayer` and the three-marker soft mask
  (`BeginSoftMask`, `BeginMaskedContent`, `EndSoftMask`) are inline commands, not a tree
  and not a pool of nested command lists. Mismatched markers are tolerated. The stated
  reason: every pass that walks the list "by **flat command range**" keeps working, and a
  soft mask maps "1:1 onto PDF".
- **A group's `bounds` is a clip** in both rasterisers. `fit_transparency_group_bounds`
  runs on each finished page list and grows every group to what its contents paint.
- **`DisplayList::digest()`** is FNV-1a 64 over the `Debug` form of the commands, paths,
  gradient and spot pools, plus the raw image bytes. It is the equality test between builds.

## Evidence

- `crates/paged-compose/src/display_list.rs:1462-1485` — `DisplayList`, its pools and the two optional side channels; `:321-364`, `:454-457` — `PathBuffer::intern`, `GlyphCacheKey`
- `crates/paged-compose/src/display_list.rs:1158-1185` — "Why three markers and not a `SoftMaskId` pool": the rejected pool, the passes that walk by flat range, the PDF mapping
- `crates/paged-compose/src/display_list.rs:982-998` — `bounds` "is a CLIP, not an allocation hint"; `crates/paged-compose/src/extent.rs:36-44` — the fitting pass; `crates/paged-renderer/src/pipeline/build_engine.rs:3627` — where it runs
- `crates/paged-compose/src/display_list.rs:1586-1639` — `digest()`
- `crates/paged-sdk/tests/digest_equivalence.rs:15-23` — viewer build against stock build
- `crates/paged-canvas/src/model.rs:8917-8932`, `:9689-9717` — the digest gate: incremental build against cold build
- `crates/paged-store/tests/round_trip_render.rs:49-62` — native round trip against import
- `crates/paged-export-pdf/src/lib.rs:17-20` — the exporter is "A SECOND backend over the same resolved per-page display list"

## Alternatives considered

For soft masks, a `soft_masks: Vec<SoftMaskDef>` pool holding the mask artwork as a nested
command vector "was rejected because the artwork's paths/gradients/images would then live
in a *second* set of pools" and every range-walking pass "would need a parallel recursive
form" (`crates/paged-compose/src/display_list.rs:1170-1178`). For group bounds, letting the
CPU buffer's edge be the clip "made one document enclose different content at 72 dpi than
at 300" (`crates/paged-compose/src/display_list.rs:989-991`, commit `7751cda`); "stop
clipping" is rejected in `crates/paged-compose/src/extent.rs:40-44`.

## Consequences

Plugin scene layers ([ADR 013](013-in-frame-scenelayer.md)) are lowered into the list by
`emit_scene_layer` (`crates/paged-compose/src/scene_layer.rs:529`), so they reach every
back end without a rasteriser path of their own.

Commands are addressed by position. A pass that splices commands into a list that carries
glyph runs must use `insert_command`, which shifts the recorded indices
(`crates/paged-compose/src/display_list.rs:1497-1514`); a `BeginBlendGroup` spliced in
after the fitting pass "will be clipped short" (`:997-998`).

The digest is not a stored format: "don't store digests across engine versions". It does
not hash the side channels, and it does not cover caret and hit-test geometry; the digest
gate compares `story_layout` and the diagnostics separately. A comparison of the browser
wasm's digest with the native one is named as a follow-up and is not built
(`crates/paged-sdk/tests/digest_equivalence.rs:21-23`).

`crates/paged-compose/src/lib.rs:18-21` and `README.md:5` call the display list
"versioned". `paged-compose` contains no version constant or field.

## Related

- [ADR 100](100-two-rasterisers-one-trait.md), [ADR 119](119-pdf-export-backend.md) — the consumers; [ADR 027](027-incremental-flow-invalidation.md) — relies on the digest gate
- [ADR 013](013-in-frame-scenelayer.md), [ADR 124](124-opacity-masks-native.md) — plugin drawing lowered into the list; the construct the soft-mask markers draw
