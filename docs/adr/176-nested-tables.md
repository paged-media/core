# ADR 176 — A table nested in a cell is drawn as any table is, as InDesign draws it

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-renderer`: the table emitter (`pipeline/tables.rs`) places its rows into the frame
    chain or at a fixed origin (`TablePlace`). Row growth is shared between drawing and
    measuring (`grow_rows`).
  - `paged-gen`: the `table-nested` sample, with InDesign's PDF as its reference.

## Context

A table can sit in a table cell's text. The engine drew such a table through a separate, reduced
path:
- a 0.5 pt grid as a placeholder for its strokes;
- no cell fills, per-edge strokes, cell styles or table styles;
- every span clamped to one cell;
- its columns scaled down to the host cell's width.

The host cell measured it the same reduced way, so the host row came out at the wrong height.

## What InDesign does (asked of InDesign 20.0.1)

The `table-nested` sample puts one nested table per page in a 300 pt host cell, above a plain host
row. The cases are:
- a plain 3 × 3;
- a column span and a row span;
- cell fills and per-edge strokes;
- a header row;
- columns narrower than the cell;
- columns wider than it (400 pt in 300).

InDesign's PDF shows that it sets a nested table exactly as it sets a table in a frame:
- its 1 pt default strokes;
- its fills, per-edge strokes and spans;
- its rows grown to their content.

It keeps the declared column widths: the 400 pt table runs 100 pt past its cell's edge. The host
row below starts where the nested table's rows, its outer strokes and the cell's insets end.

## Decision

- The table emitter takes a placement mode:
  - `Chain` places rows into the story's frame chain from the emitter's cursor, as before.
  - `Nested` places them at a fixed page-local origin, with no height limit. It has no start rule
    and no overset. It records no hit-test rects and no caret addresses, because the wire reaches
    top-level tables only; nested-cell editing stays deferred. It returns the height it took.
- The row growth that sizes a table's rows (`grow_rows`, with `region_cell_style`) is the one the
  host cell measures a nested table by. Its height there is its outer top stroke, its rows and its
  outer bottom stroke, so the measure and the drawing cannot disagree.
- Columns keep their declared widths.

## Consequences

- `table-nested` is gated with the text gate hard: its worst page went from mean 0.751 / p99 49.0
  to 0.045 / 0.0. `table_nested_pipeline` pins the host row's offset and the nested columns'
  extent to InDesign's PDF.
- The `tables` fixture's nested page went from 0.186 to 0.014, and its thresholds are tightened.
- Not built: caret addresses inside nested cells, and a start rule or overset inside a nested
  table (InDesign has no frame to move a nested row to).
