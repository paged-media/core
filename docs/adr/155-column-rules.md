# ADR 155 — Column rules: where InDesign draws the rule between two columns

- **Status:** Accepted 2026-10-14.
- **Scope:**
  - `paged-model`: `ColumnRule`, `TextFrame::column_rule`.
  - `paged-renderer`: `StoryEmitter::apply_column_rules` and the deepest line's descent per frame
    (`frame_max_descent_64`).
  - `paged-gen`: `ColumnRulePref` and the `column-rules` sample.
  - plugin-publish `idml-import`: the `ColumnRule*` attributes of `<TextFramePreference>`.
  - No wire or protocol change.

## Context

InDesign 2019 added column rules: a stroke in each gutter of a multi-column text frame. InDesign
writes ten `ColumnRule*` attributes on every `<TextFramePreference>` (74 of the 305 census
documents), and `ColumnRuleOverride="true"` switches them on (2 documents). Nothing read them,
so a ruled frame rendered without its rules.

## The question to InDesign

`column-rules` (a `paged-gen` sample) is a Letter page of nine 168 × 180 pt frames of Open Sans
8/10, one case each:
1. the default 1 pt rule;
2. three columns, 0.5 pt at 50 % tint;
3. a 2 pt `Dashed` rule;
4. top and bottom insets 12 and 24, unchained;
5. a 4 pt offset;
6. insets of −8;
7. a frame inset of 10;
8. three lines of text only;
9. one column.

InDesign 20.0.1 exported it, and every rule was read from the PDF's stroke operators
(`tools/indesign-export/pdf-strokes.py`): its width, cap, dash and both end points.

## Decision

**Where the rule stands.**
- One rule per gutter, at the gutter's centre plus `ColumnRuleOffset` (case 5: 306 → 310).
- The frame's inset narrows the columns symmetrically, so it does not move the rule (case 7).

**How far it runs.**
- It starts at the text area's top (the frame's top plus its top inset), moved down by
  `ColumnRuleTopInset`.
- It ends at the frame's deepest line, moved up by `ColumnRuleBottomInset`. The deepest line ends
  at its baseline plus its face's descent (hhea): 250.55 + 8 × 600/2048 = 252.895, exactly
  InDesign's end.
- The end is not the frame's bottom. Case 8 holds three lines in its first column and its rule
  stops under them.
- Negative insets run the rule past the text area (case 6: 292 to 488.895).

**How it is stroked.**
- Butt caps.
- The colour, tint and stroke style are the rule's own.
- A dashed rule is fitted like any open path's dashes (ADR 148): case 3's 2 pt `Dashed` is 12.06 on
  and 12.06 off, starting and ending on a dash.
- A frame with one column draws no rule (case 9).

**In the engine.**
- A columnar frame reaches the story emitter as one chain entry per column
  (`expand_column_chain`): clones that share the frame's `Self`. The emitter groups consecutive
  entries that share a `Self`; each gutter is the midpoint between two neighbouring column boxes.
- The emitter already tracked each frame's deepest baseline. It now also keeps that line's descent:
  its faces' hhea descenders at their sizes, the largest.
- After vertical justification, `apply_column_rules` pushes each frame's rules and splices them at
  the start of its first column's command range. So they stand above the frame's fill and stroke
  and under its text, and they move with the text into the frame's z-slot.
- The rule is drawn in the frame's inner coordinates under its `ItemTransform`, so a rotated frame
  rotates its rules.

## Result

`column-rules` against InDesign's PDF:

| | mean ΔE | p99 | SSIM | ink |
|---|---|---|---|---|
| without rules | 0.521 | 8.63 | 0.979 | 6.29 |
| with rules | 0.331 | 7.28 | 0.996 | 3.41 |

- Every rule's extent matches InDesign's to the pixel.
- What remains is glyph edges, plus pdftoppm snapping the 0.5 pt hairlines to whole pixels where
  the engine antialiases them across two.
- `tests/column_rules.rs` pins every rule's position, length and width to InDesign's numbers
  within 0.05 pt.

## Not yet

- **Vertical justification.** The rule ends at the deepest line as the emitter laid it out before
  vertical justification moved the text; a centred or bottom-aligned ruled frame is not measured.
- **Object styles** that switch rules on (`<TextFramePreference>` under an `<ObjectStyle>`) are not
  read.
- **Span and split column regions** (`span_columns.rs`) are not ruled.
- **Not editable.** No property path writes the rules yet. An existing frame keeps its source
  attributes on save, but a frame the engine creates writes none.
