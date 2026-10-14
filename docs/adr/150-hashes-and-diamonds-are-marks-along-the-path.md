# ADR 150 — Hashes and diamonds are marks along the path

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-compose` (`stroke_marks`: `fit_marks`, `push_marks`; `DashPattern::marks`;
  `dash_fit::push_stroke`; the unit rectangle's anchor order), `paged-renderer` (the built-in
  stroke-style table in `color_paint.rs`). No model, wire or protocol change.

## Context

InDesign's built-in `Left Slant Hash`, `Right Slant Hash`, `Straight Hash` and `White Diamond`
stroke styles drew solid: the stroke-style table had no entry for them, and a solid line the full
weight wide stood where InDesign draws hairline marks. On `stroke-styles` (ADR 148) rows 6–9
held the page's whole residual: mean ΔE 1.246, p99 81.67, ink 29.93.

## The question to InDesign

The rows were already in `stroke-styles`: each style on a 120 × 60 rectangle at 2 pt, a 137 × 61
rectangle at 5 pt and an open 150 pt line at 4 pt. Their geometry is read from InDesign
20.0.1's PDF with `tools/indesign-export/pdf-strokes.py`. A hash style is one stroke of many
two-point subpaths; a diamond is a stroke of its own, a closed four-point path.

## Decision

Marks run along the **whole contour**. Dashes are fitted side by side (ADR 148); marks are
not. A closed contour's ends meet at its first anchor, and a rectangle runs the way InDesign
writes it, down its left edge first.

**Spacing.**
- A mark is centred at each of `n` places spread evenly between an inset from each end.
- The inset is half of what the mark paints along the path.
- `n − 1` is the room between the insets over the nominal pitch, rounded down.

| Style | Pen | Nominal pitch | Inset | Mark |
|---|---|---|---|---|
| Straight Hash | 0.187 w, butt caps | w / 2 | pen / 2 | square across the path, ± w / 2 |
| Left / Right Slant Hash | 0.187 w, butt caps | w / 2 | w / 2 | 45°, ends ± `a` along and across, `a` = w/2 − (pen/2)·cos 45° |
| White Diamond | 0.09 w, miter joins | w | 0.5248 × the fitted pitch | rhombus 0.4598 × pitch along, 0.438 w across |

**Slant hashes.**
- Each end of a slant hash sits at its own place along the path, at `s − a` and `s + a`.
- Each end is pushed across by `a` along that place's own normal. A hash through a corner
  therefore bends round it, as InDesign's does.
- A left slant leans its trailing end (`s − a`) towards the left of travel, the `(dy, −dx)`
  normal in page space. That is the side Wavy leaves its start on (ADR 147). A right slant
  leans its leading end there.

**Diamonds.** A diamond's pitch fills the contour exactly:
`pitch = L / (n − 1 + 2 × 0.5248)`, with `n − 1` counted at the nominal pitch `w`. A diamond
through a corner lies along the chord between its two tips. InDesign turns it the same way and
shrinks it slightly. The chord places its corners within about 0.2 pt of InDesign's.

**Numbers measured** on the three shapes:

| Style | 120 × 60 at 2 pt | 137 × 61 at 5 pt | 150 pt line at 4 pt |
|---|---|---|---|
| Straight Hash | 360 marks | 159 | 75 |
| Slant hashes | 359 | 157 | 74 |
| White Diamond | 179 | 79 | 37 |

The slant hashes' corner geometry matches InDesign's to its PDF's 0.001 pt, and so do the
first and last marks on every shape.

**Where it is drawn.**
- The style travels on the stroke as `DashPattern::marks`. `dash_fit::push_stroke` turns it
  into page-space geometry, so rectangles, polygons, ovals and graphic lines all get the marks.
- A stroke that never reaches `push_stroke` keeps drawing solid, because the dash array stays
  empty.
- The unit rectangle that frame strokes are drawn from is now traced in InDesign's order.
  A fill does not depend on the order, and dashes are fitted per side, so only the marks see it.

## Consequences

- `stroke-styles`: mean 1.246 → 0.241, p99 81.67 → 8.98, SSIM 0.9711 → 0.9933, ink
  29.93 → 7.17. What is left is the antialiasing of 0.37–0.94 pt hairlines (pdftoppm against
  tiny-skia).
- Measured with the default cap and join. A hash or diamond stroke's own `EndCap` / `EndJoin`
  is not applied to its marks.
- A gap colour is not drawn under the marks. How InDesign fills the gaps of a hash or diamond
  style has not been asked of it yet.
