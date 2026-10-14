# ADR 140 — A paragraph aligned to the baseline grid stands each line on the next grid line

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-model` (`GridAlignment`, `Paragraph` / `ParagraphStyleDef::grid_alignment`
  and `grid_align_first_line_only`, `TextFrame::baseline_frame_grid` (`BaselineFrameGrid`,
  `BaselineFrameGridRelativeOption`), `GridPreference::baseline_grid`), `paged-scene` (the cascade),
  `paged-renderer` (`pipeline/baseline_grid.rs`, applied in `emit_paragraph_lines`), `paged-gen`
  (`baseline-grid`, `baseline-grid-margin`). The IDML spelling is plugin-publish's (reads in
  `idml-import` `story.rs`, `styles.rs`, `spread.rs`; writes in `idml-export`). Every model addition
  is serde-default: no protocol bump.

## Context

`GridAlignment="AlignBaseline"` sets a paragraph's lines on the document's baseline grid
(`<GridPreference>` in `Resources/Preferences.xml`) or on its frame's own
(`<BaselineFrameGridOption>`). The engine read neither: `GridPreference` fed only the editor's
overlay, and an aligned paragraph kept its leading. The census shows the body style of 7 of its 271
InDesign documents aligned to the grid, which moves every line of their body text.

## Decision

The rule was asked of InDesign 20.0.1 through two fixtures and its DOM
(`tools/indesign-export/baseline-grid-probe.*` writes every line's baseline to
`corpus/generated/<stem>.baselines.json`):

- **A line of an aligned paragraph stands on the first grid line at or below where its leading put
  it, and every line after it is placed from there.** 10/12 text on a 14 pt grid advances 14 pt a
  line; a 24/28.8 heading takes three grid lines; leading 15 takes two. An empty paragraph's line
  aligns like any other; space before is added before the line is aligned.
- **`GridAlignFirstLineOnly`** aligns the paragraph's first line; the others keep their leading from
  it.
- **No grid line lies above the first.** The lines are `origin + k × step` for `k ≥ 0`: a frame that
  opens above the grid's first line sets its first line on that line, not on one extended upward.
- **The origin** is the page's top edge or top margin plus `BaselineStart`
  (`BaselineGridRelativeOption`, in InDesign's spelling `TopOfPageOfBaselineGridRelativeOption` /
  `TopOfMarginOfBaselineGridRelativeOption`); the defaults are 36 pt from the top of the page,
  every 12 pt. A frame with `UseCustomBaselineFrameGrid="true"` uses its own grid instead: its
  increment from the top of its inset, the frame, the page or the margin plus its offset (InDesign
  defaults: top of inset, 0, 12 pt). A frame grid that is declared but not used leaves the
  document grid in force.
- A frame that is rotated or scaled keeps its lines on their leading: a grid measured from the page
  needs the frame's coordinates to be the page's shifted.
- The em-box and ICF alignments align to the CJK layout grid; they are read and written back, not
  laid out.

Every one of the 123 body baselines of `baseline-grid` and the 50 of `baseline-grid-margin` matches
InDesign's to 0.001 pt (`baseline_grid_pipeline`). plugin-publish reads `GridAlignment` and
`GridAlignFirstLineOnly` on paragraph styles and ranges and `<BaselineFrameGridOption>` on text
frames, and writes them where InDesign does (InDesign writes `GridAlignFirstLineOnly` on a range
whose paragraph sets it itself); an unchanged package round-trips byte for byte.

## Consequences

- `baseline-grid` (two pages, twenty cases): worst page mean ΔE 4.339 → 0.308; `baseline-grid-margin`
  1.793 → 0.140. Both gated and text-gated.
- The baseline grid is laid out where a document aligns to it; it still never prints.
- Open: the CJK layout-grid alignments; grids in rotated or scaled frames.
