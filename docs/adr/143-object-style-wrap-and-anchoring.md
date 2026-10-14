# ADR 143 — An item takes the text wrap and anchoring it leaves out from its object style

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-model` (`ObjectStyleDef::text_wrap` / `anchored`, `own_text_wrap`,
  `own_anchored`; `AnchoredObjectSetting::stated` / `over`, `AnchoredStated`;
  `StyleSheet::item_text_wrap` / `item_anchored`), `paged-renderer` (the wrap shapes, the
  next-column obstacles, `anchored::with_style_anchoring`, and where a custom-anchored object is
  placed), `paged-gen` (`object-style-options`). The IDML spelling is plugin-publish's
  (`idml-import` `styles::read_object_style_text_frames`, `story::parse_anchored_object_setting`).
  Every model addition is serde-default: no protocol bump. Extends ADR 137 to two more categories.

## Context

ADR 137 found that InDesign writes the text-frame options an object style supplies off the frame.
The same question for the style's text wrap, anchored-object and frame-fitting options went to
InDesign 20.0.1 (`tools/indesign-export/object-style-options-probe.*`): a document built in its DOM
and exported, and the `object-style-options` fixture opened in it and read back.

## Decision

What InDesign answered:

- **It writes text wrap on every item**, its own or its style's, so an InDesign document never
  needs the fallback; a producer that leaves an item's wrap off gets the style's.
- **It leaves the anchored-object options a style supplies off the item and writes only those the
  item overrides**: an object of a custom-positioned style moved 20 pt is written
  `<AnchoredObjectSetting AnchorXoffset="20"/>`. The same holds for frame fitting.
- Opening an IDML whose items state nothing, an item takes its style's own options **only with the
  category on** (`EnableTextWrapAndOthers`, `EnableAnchoredObjectOptions`; an absent flag is
  off), **not down `BasedOn`** (a style based on one that wraps, with its category on and no
  element of its own, does not wrap), and an item's own option wins: case 10's object keeps the
  style's position, point, alignment and y offset with its own x offset.

So the model keeps each style's own `<TextWrapPreference>` and `<AnchoredObjectSetting>`, and an
item's anchoring is read over its style's attribute by attribute. The reader records which plain
values an `<AnchoredObjectSetting>` stated (`AnchoredStated`); a setting without that record (made
in paged, or read before it) is all its own. The renderer resolves both where it uses them: the
wrap shapes and next-column obstacles take `StyleSheet::item_text_wrap`, and a paragraph's
anchored objects are read over their styles once as it is emitted. Frame fitting has no effect on
what prints (InDesign bakes the fit into the image's transform), so it is not modelled.

The fixture also showed a placement error independent of styles: a custom-anchored object was
placed against the line after its paragraph, where InDesign places it against the line holding its
anchor (case 7: its top at the first line's baseline + 6 pt in a two-line paragraph, the engine's
19.2 pt lower). It now finds that line among the paragraph's laid-out lines.

## Consequences

- `object-style-options` (new, gate): every object and the first lines of every case stand where
  InDesign's DOM puts them, to 0.05 pt (`object_style_options_pipeline`); mean ΔE 1.521 → 0.213.
- `anchored` 0.148 → 0.146 on average; no other fixture moves.
- Open: the object style's paragraph style (InDesign applies it when the style is applied, so the
  text already carries it), transparency and effects (one corpus document sets them on a style).
