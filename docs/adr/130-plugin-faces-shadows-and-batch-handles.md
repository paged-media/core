# ADR 130 — Scene-scoped faces, a drop shadow on every shape, handles on every batch lane

- **Status:** Accepted 2026-10-06.
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
  inverses. A polygon's shadow is stamped under its own outline. A line has no fill, so its
  shadow is cast by the stroke: the centreline is stroked at the line's width, caps, join and
  miter limit, and the resulting band is stamped (a dash is not cut out). The IDML adapter reads
  a polygon's and a line's `TransparencySetting` shadow and writes it for source and inserted
  ovals, polygons and lines.
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
- Shadows made on a pen path, an ellipse or a rule survive an IDML save and reopen. Whether
  InDesign draws a line's object shadow from its stroke as the engine does has not been compared
  against InDesign yet.
- Every addition is optional on the wire. Hosts gate on protocol 70.
