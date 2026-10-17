# ADR 177 — A frame holds the table rows its text area holds, strokes included

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-renderer`: the row fit in the table emitter (`pipeline/tables.rs`).
  - `paged-gen`: the `table-fit` sample, with InDesign's PDF as its reference.
  - `tools/indesign-export/table-fit-probe`.

## Context

A table row that does not fit its frame moves to the next one. The emitter tested the fit against
the frame's whole height and stopped at the bottom of the row's cell. It left out two things:
- the frame's bottom inset, including the share of the frame's stroke folded into it;
- the half of the stroke that lies below the row.

The `table-start-row` fixture first showed the difference. With six 20.826 pt rows in a 126 pt frame
with a 0.25 pt stroke, the engine set six rows and InDesign five.

## What InDesign does (asked of InDesign 20.0.1)

`table-fit-probe` builds frames of stepped heights, each threaded to a second frame, and puts a
table of 20 pt rows with 1 pt strokes in each. It reports how many rows the first frame holds:

| Frame | Six rows need | Fit |
|---|---|---|
| Plain | 121 pt: 120 of rows, half a stroke above and below | 120.9 holds five, 121 six |
| 4 pt bottom inset | 125 pt | 124.9 holds five, 125 six |
| 1 pt centred frame stroke | 122 pt: half the stroke at each end | 121.9 holds five, 122 six |

The `table-fit` sample asks the same six edge cases in an IDML. InDesign opens it and sets 5, 6, 5,
6, 5, 6 rows in the first frame.

## Decision

A frame holds a row when the frame's text area holds everything down to the outer half of the
stroke under the last row:
- the text area is the frame's height above its bottom inset, with the stroke's share already
  folded in;
- the last row is the row itself, the end of a run of kept rows, or the replayed footer.

`row_bottom_half` takes the widest bottom edge of the cells that end in that row. A cell's default
edge is 1 pt, and an edge in `Swatch/None` counts for nothing.

## Consequences

- `table-fit` is gated with the text gate hard: worst page 0.069 → 0.008. `table_fit_pipeline`
  counts each frame's rows.
- Tables in frames with a bottom inset or a stroke now break where InDesign breaks them.
