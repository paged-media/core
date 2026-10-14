# ADR 152 — A cell's fill tint, on the cell and from its style

- **Status:** Accepted 2026-10-14.
- **Scope:**
  - `paged-model`: `TableCell::fill_tint`; `CellStyleDef` / `ResolvedCell` fill and edge-stroke
    tints.
  - `paged-renderer`: cell fill and edge painting in `tables.rs`.
  - `paged-mutate` / `paged-canvas`: the `cellFillTint` path stores, reads and undoes.
  - `paged-gen`: the `table-cell-tints` sample.
  - plugin-publish `idml-import` / `idml-export`: `<Cell FillTint>`, `<CellStyle FillTint>` and the
    style's edge-stroke tints.
  - No wire or protocol change.

## Context

- A `<Cell FillTint>` was not read, so every tinted cell painted its swatch at 100 %.
- `cellFillTint` read `null` on every cell and stored nothing it was given.
- A cell style's `FillTint` and its edge-stroke tints were not read either, although its fill
  colour and edge colours were.

## The question to InDesign

`table-cell-tints` (a `paged-gen` sample) is one table of twelve cells, each with one case:
- a cell's own colour and tint;
- a style's colour and tint;
- a style's tint with the cell's colour, and a cell's tint with a style's colour;
- a tint inherited through `BasedOn`;
- a tint with no fill;
- tints of 0 and 100.

Each cell's colour in InDesign 20.0.1's PDF was compared with the engine's.

## Decision

- **Fill.** A cell's fill tint is its own `FillTint`, else its style's, resolved through
  `BasedOn` like the colour. It tints whichever colour the cell fills with, the cell's own or the
  style's. A tint with no fill paints nothing. All twelve cells match InDesign's colour to
  1/255 (mean ΔE 0.027).
- **Edges.** A cell style's `TopEdgeStrokeTint` … `RightEdgeStrokeTint` resolve the same way, the
  cell's own first. This is the precedence the engine already gives the edges' colour and weight.
- **Export.** A cell the engine writes carries `FillTint`. A source cell style passes through
  byte for byte.

## Consequences

- **Insets:** a cell style's insets are not cascaded. InDesign writes every cell's insets on the
  cell itself: in the corpus, all 252 cells applying an inset-bearing style restate them. Fill,
  edge and vertical-justification attributes a style supplies are left off the cell, and the
  engine cascades those.
- **Edge priority:** not modelled. InDesign writes both cells of a shared edge with the same
  values, so `EdgeStrokePriority` only decides input that InDesign itself would not write.
