# ADR 179 — Each frame of a chain sets its lines at its own measure

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-renderer`: the measure a paragraph composes at (`StoryEmitter::region_measure_pt`,
    `StoryEmitter::chain_line_measures`).
  - `paged-gen`: the `chain-widths` sample, with InDesign's PDF as its reference.

## Context

A story's text frames need not share a width. A story can run from a full-width frame into a
narrow sidebar, or from a narrow lead-in frame into a wide body frame. The emitter took every
paragraph's measure from the chain's head frame, unless a span-columns plan supplied
per-region widths. So a paragraph in the second frame of a 400 / 250 pt chain was set 400 pt wide
and ran 150 pt past its frame's edge. The `footnote-split-narrow` fixture (ADR 178) showed the
same defect for a paragraph that a footnote reservation moved on.

## What InDesign does (asked of InDesign 20.0.1)

The `chain-widths` sample threads one story through frames of 400, 250 and 400 pt, and another
through frames of 250 and 400 pt. It is set in Inter 10/12 pt, so each frame holds whole lines.
Its paragraphs cross each change of width mid-paragraph.

InDesign's PDF keeps each frame's lines at that frame's measure. A paragraph that runs on into a
frame of another width is set at the new measure from the first line that lands there. The lines
before the boundary are the ones a single frame of the first width would have set.

## Decision

- **A region's measure.** In order:
  - the span plan's measure, when there is one;
  - for the head region, its column width, which includes the wrap shrink and the caller's
    fallback width;
  - otherwise, the region's own inner width (bounds minus its side insets).

  A column clone's inner width is its column's.
- **A paragraph composes at the measure of the region it starts in.**
- **Per-line measures across a width change.** When a region from the paragraph's region onwards
  has another measure, the composer gets per-line measures:
  - the lines the current region holds below the paragraph's first baseline, above its text
    area's bottom and its footnote reservation, at its measure;
  - then each next region's lines at its own.

  The current region's lines are exact; past it, the lines are predicted from the next frame's top
  inset and the paragraph's leading.
- **Uniform chains are unchanged.** When every region has the current measure (within 0.25 pt),
  the scalar width serves as before. So chains of equal frames, and equal columns, do not change.
- **Wrap plans and drop caps keep their own per-line widths.** This plan applies only where neither
  set any.

## Consequences

- `chain-widths` is gated with the text gate hard: p1 went from mean 1.497 / SSIM 0.919 to
  0.137 / 0.998, p2 from 0.530 to 0.065.
- `footnote-split-narrow` went from 1.224 to 0.279.
- Not built:
  - a paragraph that a keep or start rule moves into a frame of another width is composed at the
    measure of the frame it was in;
  - mixed leading in the lines past a boundary can misplace the predicted width change by a line.
