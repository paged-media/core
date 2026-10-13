# ADR 138 — A position is the exact sum of the advances before it, rounded once

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-text` (`shape_run_with_features`, `LayoutOptions::leading_override_exact` /
  `line_height_exact` / `first_baseline_exact`, `LaidOutLine::baseline_exact`,
  `line_leading_exact`, `layout_runs_cached`), `paged-renderer` (`StoryEmitter::y_cursor_exact` /
  `prev_line_height_exact`, the paragraph and line advances, the wrap twin pass, the wrap plan's
  bands, the resume marks). No model, wire or protocol change.

## Context

The engine keeps positions in 1/64 pt. It rounded every step to that grid before adding it up:
each glyph's advance, and each line's leading. A step that is not a multiple of 1/64 pt then
drifts by its rounding error once per glyph or line, always the same way.

Both drifts were measured once the `text-wrap` fixture set enough text to show them (it had set
11 lines that ended above its obstacles; it now sets 44):

- **Leading.** 12 pt auto leading is 14.4 pt; `round(14.4 × 64) / 64` is 14.40625. Down a
  47-line page the engine's baselines ran 0.29 pt below InDesign's, which steps exactly 14.4
  (`pdftotext` of its PDF). Every 9, 11, 12 and 14 pt auto-leaded text drifts this way.
- **Advances.** At 12 pt an Open Sans advance is its font units × 0.375 / 64 pt, and those
  fractions round up more often than down. A 78-glyph line summed to 447.281 pt, where the font's
  exact advances (and InDesign's PDF) give 447.141.

## Decision

A position is the exact sum of the steps before it, rounded once:

- `shape_run` gives each glyph the difference between the run's exact pen before and after it,
  each rounded. The run's total is the rounded exact total.
- A paragraph's baselines are its first baseline plus the exact leadings above each line, rounded
  per line. An explicit leading carries its unrounded value (`leading_override_exact`); auto
  leading is 1.2 × the size, unrounded.
- The renderer's cursor carries the exact sum across paragraphs, through space before and after
  and empty paragraphs. The exact value is trusted only while it rounds to the integer cursor, so
  a path that moves the cursor without it falls back to the integer value it set.
- The wrap twin pass and the wrap plan's bands use the same exact sums, so a band is measured
  where its line lands. The resume marks carry and compare the exact state, so an incremental
  rebuild and a full build agree.

## Consequences

- On `text-wrap`, the page without a wrap went from mean ΔE 1.38 to 0.40 (leading 0.73, then
  advances 0.40); the wrapped pages from 1.55 to 0.53. Other fixtures' measurements moved too;
  their thresholds were re-measured with this change.
- Tracking still adds a rounded amount per glyph; a non-zero tracking drifts by up to
  1/128 pt a glyph.
- Runs whose explicit leadings disagree (`run_leadings`) still step by rounded leadings.
