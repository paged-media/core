# ADR 160 — Footnotes in a frame with columns: under each column, or across the frame

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `paged-model`: `FootnoteOptions::enable_straddling` and `TextFrame::footnotes`
    (`FrameFootnotes`, with InDesign's frame defaults).
  - `paged-renderer`: `EmittedFootnote::{pool_rect_pt, frame_footnotes}`, `footnote_pool_rect`,
    the reservation keys, and `resolve_footnote_metrics` taking the frame's override.
  - `paged-gen`: `TextFramePref::footnotes_span_across_columns`; the `footnote-columns` and
    `footnote-straddle` samples; `tools/indesign-export/footnote-columns-probe`.
  - plugin-publish `idml-import`: `EnableStraddling` on `<FootnoteOption>` and the four
    `Footnotes*` attributes of `<TextFramePreference>`.
  - No wire or protocol change.

## Context

The engine lays a frame's columns out as one chain entry per column, so a note was always set
under the column holding its reference, at the column's width. InDesign can also set a frame's
notes across all its columns:
- the document says so with `EnableStraddling`, which 68 of the 85 census documents that write it
  set;
- a frame overrides the document with `FootnotesEnableOverrides`, bringing its own span and
  spacing.

The importer read neither.

## The question to InDesign

`footnote-columns` and `footnote-straddle` are one A4 page with two frames of two columns each:
- Frame A, on top, holds six paragraphs; the fourth starts the second column.
- Frame B, below it, holds four paragraphs; the third starts the second column.
- Frame B overrides the document's footnote layout.
- Every paragraph carries a note that wraps once at a column's width but fits the frame's.

The two samples differ in which way each frame goes:
- `footnote-columns`: the document does not span; frame B spans.
- `footnote-straddle`: the document spans; frame B does not.

InDesign 20.0.1 exported both, and the note lines were read with `pdftotext -bbox`.
`footnote-columns-probe` asked the DOM what the frames' settings are, and re-saved the document as
IDML.

## Decision

**A frame's notes stand under each column, or span the frame, as the frame's layout says.**
- Without spanning, each column's notes sit at the foot of that column, at its width, each column
  with its own rule.
- With spanning, all the frame's notes sit at its foot, at the frame's width, in reference order,
  under one rule. Every column then gives up the same height to them.

**The frame's layout is the document's unless the frame overrides it.**
- Without `FootnotesEnableOverrides="true"`, the document's `EnableStraddling`, `SpaceBetween` and
  `Spacer` apply.
- With it, the frame's `FootnotesSpanAcrossColumns`, `FootnotesSpaceBetween` and
  `FootnotesMinimumSpacing` replace them.
- What the override does not state takes InDesign's frame defaults, not the document's: no
  spanning, 6 pt between notes, 12 pt minimum spacing.
- The probe reads those values off the DOM for a frame that writes only the override flag. The PDF
  shows the 6 pt: frame B's notes stand 15.6 pt apart (9.6 pt leading plus 6), frame A's 9.6 pt
  (the document's 0).
- InDesign's own IDML writes the override and span attributes only, as the samples do.

**How it is built.**
- A note records the rect its area is set in: its column's content rect, or the union of every
  column of its frame when it spans. The columns of a frame are its chain entries that share its
  `Self`.
- Notes sharing a rect share an area.
- The reservation pass maps each column to that rect, so each column of a spanning frame reserves
  the whole area's height.

## Result

- `footnote_columns::notes_stand_under_their_column_unless_they_span` and
  `::a_document_that_straddles_spans_every_frame_that_does_not_override` check the widths, line
  counts and spacing.
- Both samples are gated on pixels and on text, and every note line lies within 0.05 pt of
  InDesign's.
- The pixel measures are mean ΔE 0.142 and 0.153. Main's engine, which set every frame's notes per
  column with the document's spacing, measured 0.486 and 0.672.

## Not yet

- **Splitting a note across columns or frames** (L-01e), the continuing rule, and overset growth
  (L-01f).
- **Object styles** that carry footnote options are not read.
- **No property path** sets `EnableStraddling` or a frame's override. These are footnote ops on
  the next protocol (v72).
