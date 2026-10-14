# ADR 147 — A rotated frame's gradient starts where InDesign puts it

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-renderer` (`module::fill_paint::rotated_linear_default`, used by
  `place_linear_at_start` for a linear fill with no `GradientFillStart`),
  `tools/indesign-export/gradient-defaults-probe.*`. No model, wire or protocol change.

## Context

A linear gradient fill that states no `GradientFillStart` starts, in InDesign, at the inner
box's bottom-left corner and is as long as the box is wide (ADR 120, R-02a). That holds for an
upright frame. On `gradient-geometry` page 1 the column of squares turned 30° still differed
(mean ΔE 1.63, p99 49): InDesign's own re-export of that frame writes
`GradientFillStart="-7.32 12.68" GradientFillLength="109.28"`, not `"0 80"` and `80`.

InDesign documents always carry an explicit start (91 of 91 gradient-filled items in the corpus
packs), so this is the default for IDML written by other tools: the generator, other
converters, and an editor that clears a start.

## The question to InDesign

`tools/indesign-export/gradient-defaults-probe.sh` turns the 18 rectangles of a copy of
`gradient-geometry` into 80 × 60 pt frames rotated 0°, 15° … 255°, each with
`GradientFillAngle="0"` and no start or length, opens it in InDesign 20.0.1 and reads back the
start and length InDesign writes into its own IDML (`gradient-defaults-probe.json`). A second
run, not committed, added flips, a shear and non-uniform scales.

## Decision

Only the rotation θ of the item's transform counts: a scale, shear or vertical flip leaves the
answer of the rotation alone, and a horizontal flip is θ = 180° (InDesign wrote the same start
for 30° scaled 1.5 × 0.5 as for 30°). Turn the inner box by θ into its parent's axes:

- **The length is the width of the turned box.**
- **At exactly 90°, 180° and 270°** the start is a corner of the inner box: top-right,
  bottom-right and bottom-left.
- **Strictly between 0° and 180°** it is the left-centre of the turned box, carried back into
  inner coordinates.
- **Strictly between 180° and 360°** it is the inner corner the turn puts furthest left.

The angle stays the frame's `GradientFillAngle`, in inner coordinates, so the axis turns with
the frame from that start. A stated `GradientFillLength` still wins over the default length. An
upright frame keeps the R-02a default.

The rule is piecewise, and the probe shows the breaks are InDesign's own: at 75° the start is
`(29.82, −8.01)`, at 105° `(50.18, −8.01)`, and at exactly 90° the corner `(80, 0)`.

## Consequences

- `gradient-geometry` page 1: mean ΔE 1.626 → 0.081, p99 49.2 → 1.1; the fixture's thresholds
  are re-sized to the new measurement.
- Gradient strokes (`GradientStrokeStart`) and gradient feathers were not asked; they keep the
  upright default. Case 28 of the fixture, a gradient feather with no start, still differs.
