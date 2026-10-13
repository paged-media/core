# ADR 137 — A text frame takes the options it does not state from its object style

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-model` (`ObjectStyleDef::enable` / `text_frame`, `ObjectStyleCategories`,
  `TextFrameOptions`, `ObjectStyleDef::own_text_frame_options`, `StyleSheet::resolve_object`),
  `paged-renderer` (`text_frame::with_object_style_options`, reached through
  `text_area_frame`), `paged-gen` (`object-style-frames`). The IDML spelling is plugin-publish's
  (`idml-import` `styles::read_object_style_text_frames` and the `Enable*` flags). Every model
  addition is serde-default: no protocol bump.

## Context

An object style can set a text frame's options: vertical justification, insets, columns, the first
baseline, auto-sizing. The engine read those options from the frame only. InDesign writes an
option the style supplies **off** the frame. Asked through its DOM (InDesign 20.0.1), a frame of a
style that centres its text is exported with no `VerticalJustification` at all. Every frame whose
style set such an option was therefore laid out with InDesign's defaults: top-aligned, one column,
no insets.

The census shows the categories matter: the `Enable*` flags of object styles carry both values in
271 of its documents, and InDesign writes every flag on every style.

## Decision

**Where the options come from.** A text frame that does not state an option takes it from its
applied object style. The rule was asked of InDesign through the `object-style-frames` fixture and
three variants of it, opened in InDesign and read back through its DOM:

- only a category the style **switches on** reaches the frame
  (`EnableTextFrameGeneralOptions`: vertical justification, insets, columns;
  `EnableTextFrameBaselineOptions`: first baseline; `EnableTextFrameAutoSizingOptions`: auto-sizing).
  An **absent** flag is off: InDesign reports the category disabled;
- the options come from the applied style's **own** `<TextFramePreference>`. They do not pass down
  `BasedOn`: a style based on one that centres its text, with the category on and nothing of its
  own, sets its frames top-aligned. An option the style's element leaves out is InDesign's default;
- an option the frame states wins.

Fill, stroke and the corner options keep cascading through `BasedOn`, the behaviour the `swatches`
fixture was measured against, but now stop at a style that switches their category off
(`EnableFill="false"`, …).

**Where it happens.** `text_area_frame` already hands the layout a resolved copy of a frame (with a
stroke's share folded into its insets); it now also fills in the options from the object style, so
the chain, the column split, auto-sizing and vertical justification all see them. The document
model keeps the frame as written, and the writer has nothing new to write.

## Consequences

- `object-style-frames`: mean ΔE 0.160, SSIM 0.9942, text-gated; before this, cases 1–5 and 8 set
  their text as an unstyled frame.
- Still open (A-12): the object style's text-wrap, anchored-object and frame-fitting options, its
  paragraph style, and its transparency, which InDesign writes the same way.
