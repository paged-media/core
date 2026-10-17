# ADR 175 — A table row starts where its Start Row option asks, as InDesign sets it

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-model`: `TableRow.start_row`.
  - `paged-renderer`: the table row loop (`pipeline/tables.rs`).
  - `paged-mutate` / `paged-introspect` / `paged-canvas`: the `rowStartRow` and
    `rowKeepWithNext` property paths, written through any cell of the row.
  - `paged-gen`: the `table-start-row` sample, with InDesign's PDF as its reference.
  - `tools/indesign-export/table-start-row-probe`.
  - plugin-publish: `idml-import` reads `Row@StartRow`; `idml-export` writes it, and patches it
    back onto a source row whose model value changed.

## Context

A table row's Start Row option (Table Options ▸ Rows and Columns, `Row@StartRow` in IDML)
moves a row to the next column, frame or page. The reader dropped the attribute, so every row
was placed as if it said `Anywhere`, and a save lost it. Nothing could set it.

## What InDesign does (asked of InDesign 20.0.1)

`table-start-row-probe` builds four facing pages, each with a two-column text frame, threaded
into one story holding a table of twelve 20 pt rows. Case by case, row 4 gets a Start Row value.
The probe reports the page and column of every row:

| `StartRow` | rows 1–3 | rows 4–12 |
|---|---|---|
| `Anywhere` | page 1, column 1 | page 1, column 1 |
| `NextColumn` | page 1, column 1 | page 1, column 2 |
| `NextFrame` | page 1, column 1 | page 2 |
| `NextPage` | page 1, column 1 | page 2 |
| `NextOddPage` | page 1, column 1 | page 3 (page 2's frame stays empty) |
| `NextEvenPage` | page 1, column 1 | page 2 |

So InDesign treats a row's Start Row like a paragraph's `StartParagraph` (ADR 028): the row,
and every row after it, moves to the next column, frame or page, or to the next page of the
asked parity, past the frames in between.

The `table-start-row` sample asks the remaining questions with the `start-paragraph` fixture's
chain (a two-column frame A and a one-column frame B on page 1, one frame on each of pages
2–4), and InDesign's export is its reference:
- the six rules on row 4;
- the rule on the table's first row, at the top of frame A;
- `NextColumn` on a row that already opens column two;
- a header row above a row the rule moves to frame B.

## Decision

- `TableRow.start_row: Option<StartParagraph>` (serde default; `None` is `Anywhere`).
- The row loop asks `keeps::start_target` (the paragraph rule) before it places a body row. A
  row that the rule moves closes its frame as a full frame closes: repeating footers, then the
  frame's extent. Then the row opens the target frame under its repeating headers. A frame the
  row leaves before any body row keeps no header, as the last frame keeps none (measured
  2026-09-07); the target frame replays it.
- No target in the chain: the row and the rows after it are overset, as a paragraph is.
- The reader takes `Row@StartRow` with `StartParagraph::from_idml`. The writer emits it for a
  new row and patches a source row only when the model value changed (`row_attrs`), so an
  unmutated save stays byte-identical.
- Two property paths reach a row through any `NodeId::TableCell` in it, InDesign's
  `cell.parentRow`:
  - `rowStartRow` (`Value::Text`, the IDML spelling; empty clears it);
  - `rowKeepWithNext` (`Value::Bool`), the row option the layout already honours
    (`tables-rows` page 11), which nothing could set.
  They are additive (no protocol bump), and the cell inspector lists both.

## Consequences

- Tables in long documents place a row that starts a section where InDesign places it.
- The `table-start-row` fixture joins the fidelity gate, with a structural test
  (`table_start_row_pipeline`) that pins each case's rows to their frame.
- Not measured: a start rule on a row inside a run of `KeepWithNextRow` rows. The rule moves
  the row first, then the run is fitted in the target frame.
