# ADR 126 — Scene-layer text draws in its own face

- **Status:** Accepted 2026-10-05.
- **Scope:** `crates/paged-compose/src/scene_layer.rs` (`SceneTextItem`),
  `crates/paged-renderer/src/pipeline/text_frame.rs` (`emit_frame_scene_layer`),
  `crates/paged-renderer/src/diagnostics.rs`, `crates/paged-canvas/src/model.rs` (font registry
  changes), the `sceneLayerApplied` reply (protocol 68)

## Context

A plugin that renders a frame's content itself hands the engine a scene layer: paths, fills,
images and single-line text runs in frame-content coordinates. The text run has carried `family`
and `style` since protocol 40, documented as reserved: the renderer shaped every run in the
document default font. A plugin that lays text out with real faces (the web content type shapes
with its own font stack) therefore saw its line breaks computed in one face and its glyphs drawn
in another. Nothing reported the difference.

The engine already resolves faces for document text: the host registers font bytes per family and
style, and the renderer resolves each run's `(family, style)` through that registry, falling back to
the default font and raising `FontSubstituted` when a face is missing.

## Decision

A scene-layer text run resolves its face through the same registry document text uses.

- **Request.** `family` names the family. The face within it is `style`, spelled like IDML's
  `FontStyle` (`"Bold Italic"`); when `style` is absent it is derived from two new fields, `weight`
  (CSS `100..900`) and `italic`, the way a type menu names faces (`700` + italic is
  `"Bold Italic"`). `weight`, or the weight the style name implies, sets the `wght` axis of a
  variable face.
- **Resolution.** The resolver's own fall-through applies: the styled entry, then the bare family.
  Its catch-all default counts as a miss.
- **Fallback.** A run whose family does not resolve draws in the document default font at its
  weight, exactly as a run with no family does. Each missed face is reported once per frame: a
  `FontSubstituted` diagnostic carrying the frame id and the requested face, and
  `SceneLayerApplied.fontFallbacks` on the submit reply.
- **Registry changes.** Registering or clearing fonts rebuilds the frames whose scene text names an
  affected family, so a layer submitted before its fonts arrive is redrawn without a resubmit.
- **Glyph cache.** Scene text uses the document text's face ids (`font id ^ wght`), so a face shared
  with the document shares outlines.
- **Protocol.** Every field is additive. The version still moves to 68, because the change is in
  behaviour: a host cannot tell from the message shape whether a worker draws `family`, and gates
  per-run faces on `protocol >= 68`.

## Consequences

- A plugin's measured line breaks and the drawn glyphs come from the same face whenever the host
  has registered it. When it has not, the plugin learns which faces are missing from the reply and
  can register them.
- Synthetic styles are not made: an italic request on a family with no italic face draws the
  upright face (the bare-family fall-through) and is not reported as a fallback.
- Scene layers stay ephemeral (not document content), so nothing here enters the undo history.
