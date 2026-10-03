# ADR 013 — In-frame plugin rendering via `SceneLayer` (C-1)

**2026-06-12 (records a 2026-06-10 decision) · decision record · status:
ACCEPTED (records the shipped C-1 contract, v0.39.0 vector / v0.40.0 text).**

**Sources:** the internal gap register, entry C-1 (the gap, the shipped
shape, the rejected alternatives); `crates/paged-compose/src/scene_layer.rs`
(the IR + `emit_scene_layer` converter +
font-aware text callback); `crates/paged-renderer/tests/scene_layer.rs` and
`crates/paged-canvas/tests/scene_layer.rs` (the splice + protocol tests);
the v39 `SubmitSceneLayer`/`ClearSceneLayer` channel + v40 `SceneItem::Text`
variant (`crates/paged-canvas/src/channel.rs`); SDK `capabilities.rendering:
["sceneLayer"]` + `ContributionSurface.sceneLayer()`; editor
`CanvasClient.submitSceneLayer`/`clearSceneLayer` + `PagedEditor.sceneLayers`.

## The decision

**A plugin renders *inside* a document frame by submitting a serializable
`SceneLayer` — a display-list subset (filled/stroked bezier paths + solid sRGB
paint + single-line text runs, in frame-content coordinates) — which core lowers
at compose time, applying the frame's `ItemTransform` and clipping to the content
box, through the *same* `DisplayList → Vello/tiny-skia` lanes as native content.**
No DOM overlay; no separate plugin rendering surface; one colour-managed,
print-correct path for native and plugin pixels alike. Shipped in two core
publish cycles: v0.39.0 (paths/fills), v0.40.0 (text).

## Why a composed scene subset beats the alternatives

The internal gap register weighed real alternatives (its C-1 entry); this records why the spliced
display-list won:

- **DOM/CSS overlay over the canvas — rejected.** A second rendering surface
  positioned over the frame would be neither colour-managed nor print-correct,
  would not clip to a rotated content box, and would diverge from the Vello
  output the moment a transform or export touched it. The platform's "render from
  the document model via Vello" doctrine forbids a second paint surface (the same
  rule that rejects in-browser ECharts for charts — see
  [ADR 016](https://github.com/paged-media/plugin-sheets/blob/main/docs/adr/016-chart-engine-plotters-chartgeometry.md)).
- **GPU texture first — deferred, not chosen as the contract.** Letting plugins
  paint into a host `GPUTexture` is the *interactive-viewport* path (the image
  plugin's interactive viewport) and is sequenced as a later C-1 stage (raw-`GPUTexture` + `GPUDevice`
  door, still open in the internal gap register). It is the wrong default: most plugin content
  (a spreadsheet grid, a chart, vector art) is resolution-independent vector that
  belongs in the display list, not a raster the engine must own and re-upload.
- **A spliced display-list subset — chosen.** It reuses the renderer's existing
  path type and lanes (zero new rasterizer), inherits colour management and
  export fidelity for free, clips correctly under rotation (the content-space
  principle — a rotated
  frame clips to a rotated box), and is **headlessly unit-testable** (converter +
  renderer-splice + protocol round-trip, no GPU/corpus needed). The submission is
  serializable, so it crosses the wire and the plugin isolate boundary cleanly.

## Consequences

- **This is the plugin rendering contract for every future plugin.** Sheets' in-
  frame grid (gridlines + cell fills + cell values), charts (ADR 016 geometry),
  and any vector plugin express themselves as `SceneLayer` submissions. It is the
  load-bearing surface K-1's modal editor
  ([ADR 012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md))
  renders *into* (no `<input>` overlay).
- **A protocol bump per capability expansion** ([ADR 006](006-protocol-coupled-versioning.md)): v39 added the
  vector channel + `CanvasModel` registry + dispatch; v40 added `SceneItem::Text`.
  Editor + plugin-sdk locked to 0.40.0 in lockstep.
- **Text v1 is honestly partial:** default font, glyphs emitted upright in page
  space. Per-run face selection and full per-glyph affine for rotated-frame text
  are sequenced follow-ons, not silently faked ([ADR 007](007-carry-through-rendering-honesty.md) honesty).
- **Open follow-on stages** (sequenced, not part of this decision): raw-
  `GPUTexture` path + `GPUDevice` door (image's interactive viewport),
  per-run face selection. Recorded in the internal gap register (C-1).

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The decision stands: a plugin's in-frame content is a
serialisable `SceneLayer`, lowered at compose time inside the frame's clip through the same
display-list lanes as native content. The text above describes the subset as it was at
protocol 40. Two statements no longer match the code (items 1 and 2); item 3 lists what is
unchanged.

**1. The item vocabulary grew from three kinds to ten.** This supersedes, in "The decision",
"a display-list subset (filled/stroked bezier paths + solid sRGB paint + single-line text
runs". It does not change the mechanism: each added item kind lowers to a display command
that already existed. One paint was new: the sweep gradient of protocol 46 added a gradient
pool and `Paint::SweepGradient` to the display list, with a branch for it in each rasteriser
(`crates/paged-canvas/src/channel.rs:284-286`, `crates/paged-gpu/src/cpu.rs:2917`,
`crates/paged-gpu/src/vello_rs.rs:2783`).

- `crates/paged-compose/src/scene_layer.rs:54-210` — `SceneItem` has ten variants: `FillPath`,
  `StrokePath`, `Text`, `Image`, `FillPathGradient`, `FillPathBlend`, `DropShadow`,
  `InnerShadow`, `StrokePathGradient`, `FillPathGradientBlend`.
- `crates/paged-compose/src/scene_layer.rs:269-300` — `SceneGradient` is linear, radial or
  sweep; `:220-236` — `SceneBlendMode` has fifteen modes.
- `crates/paged-canvas/src/channel.rs:228-236` (protocol 41, `Image`), `:278-283` (45,
  `FillPathGradient`), `:284-293` (46, sweep gradient, `FillPathBlend`, `DropShadow`),
  `:295-299` (47, `InnerShadow`), `:301-307` (48, `StrokePathGradient`,
  `FillPathGradientBlend`) — each is recorded as a payload-only addition to
  `SubmitSceneLayer`, with no new message.
- `crates/paged-compose/src/scene_layer.rs:593-632` (`Image` → `DisplayCommand::Image`),
  `:633-652` (gradient fill → `DisplayCommand::FillPath`), `:699-714`
  (→ `DisplayCommand::FillPathBlend`), `:715-751` (→ `DisplayCommand::DropShadow`),
  `:752-789` (→ `DisplayCommand::InnerShadow`) — the lowerings.

The module doc at `crates/paged-compose/src/scene_layer.rs:17-19` still describes the layer as
"filled / stroked bezier paths with solid paint" and is stale.

**2. A second in-frame channel exists for raster tiles.** Protocol 50 added
`SubmitPixelLayer` / `ClearPixelLayer`. A `PixelLayer` is a set of RGBA8 tiles; the worker
lowers it to a `SceneLayer` of `SceneItem::Image` items and stores it in the same per-frame
registry, so it is the same contract with a different wire shape
([ADR 108](108-plugin-raster-tiles.md)). This adds to the Consequences bullet "A protocol
bump per capability expansion", which names v39 and v40 only.

- `crates/paged-canvas/src/channel.rs:315-325`, `:1059-1079` — the two message kinds.
- `crates/paged-compose/src/pixel_layer.rs:88-112` — `PixelLayer::into_scene_layer`.
- `crates/paged-canvas/src/model.rs:9044-9058` — `set_pixel_layer` stores the lowered layer
  through `set_scene_layer`.
- `plugin-sdk: packages/plugin-api/src/wire.d.ts:93` — in the plugin contract (plugin-sdk at
  `d90f727`) the pixel layer occurs only in the vendored wire types; the host contract there
  has no door for it.

**3. Unchanged, and still open.** Scene-layer text is still set in the document's default
face with glyphs upright in page space (`crates/paged-compose/src/scene_layer.rs:68-74`,
`:327-331`; `crates/paged-renderer/src/pipeline/text_frame.rs:1380-1384`, `:1398-1402`). The
GPU texture door is still not built
([ADR 018](018-stage-b-gpu-texture-defer-record-only.md)). The layer registry is still
render-time state on the canvas model, not part of the document
(`crates/paged-canvas/src/model.rs:1152-1157`).
