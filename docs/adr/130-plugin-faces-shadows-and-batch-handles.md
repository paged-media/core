# ADR 130 — Scene-scoped faces, a drop shadow on every shape, handles on every batch lane

- **Status:** Accepted 2026-10-06. Amended 2026-10-07: rectangles and text frames cast their
  shadow by the same rule (the `rect-shadows` comparison).
- **Scope:** `crates/paged-canvas/src/channel.rs` (`FontScope`, `RegisterFont.scope`,
  `ClearFontRegistry`'s optional payload), `crates/paged-canvas/src/model.rs` (the scene face
  table, the translate lane's `minted`), `crates/paged-renderer/src/pipeline/text_frame.rs`
  (`PipelineOptions::scene_fonts`), `crates/paged-renderer/src/module/drop_shadow.rs`,
  `crates/paged-mutate/src/apply/set_property.rs`, `crates/paged-model` (`Polygon` and
  `GraphicLine` `drop_shadow`), `crates/paged-script` (the value bridge), the IDML adapter in
  the plugin-publish repository (protocol 70, no new message or operation kind)

## Context

A plugin that renders a content type of its own (HTML and CSS inside a frame) and then bakes it
into native page items met five gaps in the engine's doors:

1. **Fonts.** One registry served everything: document layout, scene-layer text
   ([ADR 126](126-scene-text-in-its-own-face.md)), the Fonts panel's `isMissing` and the
   substitution report. A plugin that registered the face its content is set in therefore made
   that family present for the document. A document asking for the same family stopped being
   flagged missing and composed in the plugin's bytes, without the user supplying the font.
2. **Batch handles.** A batch whose children all translate into one operation dropped its
   `bindCreated` children at translation and replied `handle: null` for every mint. The same
   batch on the mixed lane named them. A caller had to know which lane its batch took.
3. **Shadows.** The drop-shadow paths applied to text frames and rectangles. An oval carried the
   field with no setter, and polygons and lines had none, so a shadow on a drawn path or a rule
   could not be made. The IDML adapter did not read or write one for those kinds.
4. **A path from a script.** `framePath` was advertised and applied, but the script value bridge
   had no form for it, so `paged.set` returned false. Nothing checked that an advertised path
   can be built from a script value.
5. **Document labels in IDML.** The document's own labels (`setDocumentMetadata`,
   [ADR 127](127-fields-and-document-labels-for-data.md)) were kept in the native model only.

## Decision

- **Scene-scoped faces.** `RegisterFont` takes `scope`: `"document"` (the default and the old
  behaviour) or `"sceneLayer"`. A scene face goes to a second table that only scene-layer text
  reads, before the document registry. The font table, `fonts()` and substitution tracing never
  read it. Registering or clearing scene faces rebuilds only the frames whose scene text names
  the family. `ClearFontRegistry` takes an optional payload `{ scope }`. It is a newtype over
  `Option` because an adjacently tagged struct variant requires its payload, which would refuse
  the payload-less message hosts send today. That message still clears the document registry.
- **Handles on both lanes.** The translate lane recovers each binding from the batch's children
  beside the operations they produced. A binding names the most recent creation before it, and
  the last binding of an element wins, as on the mixed lane.
- **A shadow on every shape.** Polygons and lines gain the object's `drop_shadow`. The toggle and
  the six field paths apply to all five shape kinds through one slot finder, with the same
  inverses. A shape's shadow is cast from what it paints. A line has no fill, so its
  shadow is cast by the stroke: the centreline is stroked at the line's width, caps, join and
  miter limit, and the resulting band is stamped (a dash is not cut out). An oval, polygon,
  rectangle or text frame with a fill stamps its outline (a rounded one its rounded outline),
  pushed out by the part of a visible stroke that lies outside it; one with no fill and a
  visible stroke stamps its stroke band, as a line does. A text frame with no fill also casts
  its text's shadow, each glyph stamped at the object shadow's softness. The IDML
  adapter reads a polygon's and a line's `TransparencySetting` shadow and writes it for source
  and inserted ovals, polygons and lines.
- **Script values.** The bridge reads a path as an anchor array or `{ anchors, subpathStarts? }`,
  an anchor as `[x, y]` or `{ anchor, left?, right? }`, and a dash array as its numbers. A gate
  (`paged-script/tests/advertised_values_construct.rs`) requires, for every advertised path, a
  script literal that the bridge converts and the apply layer accepts by type.
- **Document labels in IDML.** The adapter reads and writes them as
  `<Document><Properties><Label><KeyValuePair/>`, the place an item's labels use. A designmap that
  already agrees is left byte-identical.

## Consequences

- A plugin can draw in its own faces without changing what the document reports or how it lays
  out. A face the document needs is still the user's to supply.
- A host gets the same reply whichever lane its batch took.
- Shadows made on a pen path, an ellipse or a rule survive an IDML save and reopen.
- The rule was compared with InDesign 20.0.1 on 2026-10-06 (the `line-shadows` fixture, gated in
  `corpus/generated/fidelity-thresholds.json` and pinned by
  `crates/paged-renderer/tests/line_shadows_pipeline.rs`). InDesign casts a line's shadow from the
  stroke band, caps included, with the same offset, softness and opacity as the engine: shadow
  profiles across a 6 pt and a 1 pt line agree within 0.05 coverage. The same comparison showed
  that InDesign casts an unfilled polygon's shadow from its stroke, where the engine cast none,
  and an oval's from the ellipse plus its stroke, where the engine stamped the bounding
  rectangle. Both now follow InDesign: the page went from mean ΔE 0.382 / p99 13.14 / SSIM
  0.9864 to 0.051 / 1.88 / 0.9991.
- The rectangular kinds were compared with InDesign 20.0.1 on 2026-10-07 (the `rect-shadows`
  fixture, gated the same way and pinned by
  `crates/paged-renderer/tests/rect_shadows_pipeline.rs`). InDesign follows the same rule. A
  stroke-only rectangle casts the shadow of its stroke band: across a 6 pt centred stroke the
  shadow peaks at the stroke's offset position (0.58 darkness, as for a 6 pt line), a 1 pt
  stroke casts a faint one (0.13), and an Outside stroke's band sits outside the frame edge.
  A filled, stroked rectangle's shadow edge lies half the weight past the frame edge for a
  centred stroke, the full weight for Outside and at the edge for Inside: the three profiles are
  the fill-only profile shifted by 3, 6 and 0 pt. A rounded rectangle's shadow is rounded. A
  text frame with no fill shadows its text as well as its stroke, and one with neither fill nor
  stroke shadows only its text, glyph by glyph at the same softness as the frame's (σ = Size /
  2). A filled text frame's shadow comes from the fill and stroke, the text inside adding
  nothing. The engine cast nothing for the stroke-only frames, the fill rectangle for the
  stroked ones, the bounding rectangle for the rounded one and no text shadow. With the rule
  applied the page went from mean ΔE 1.18 / p99 36.15 / SSIM 0.956 to 0.145 / 3.85 / 0.997.
  The residue is not the rule: every engine shadow, the plain filled rectangle's included, falls
  off about 0.4 pt inside InDesign's, and rounded corners and glyph positions differ by a
  fraction of a point.
- Every addition is optional on the wire. Hosts gate on protocol 70.
