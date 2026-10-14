# ADR 148 — Dashes are laid out side by side, as InDesign lays them out

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-compose` (`dash_fit`: `fit_dashes`, `push_stroke`; the rect and ellipse stroke
  primitives), `paged-renderer` (the stroke module's path branch, graphic lines, and the
  built-in stroke-style table in `color_paint.rs`), `paged-gen` (`stroke-styles`),
  `tools/indesign-export/pdf-strokes.py`. No model, wire or protocol change.

## Context

A dashed frame was one stroke with one dash array, run around the outline by the rasterizer.
InDesign's PDF of `strokes-fills` showed something else: each side of a dashed rectangle is its
own path with its own array, and the corners are drawn apart from the sides. Its dashes and dots
sat at the corners where ours ran past them (ink ΔE 46–52 on four pages). The built-in table was
partly invented: `Dashed` was 3 × the weight on and 2 × off, `Dotted` (a name InDesign does not
have) drew dots, and the dot spacings were guesses.

## The question to InDesign

`stroke-styles` (a `paged-gen` sample) strokes nine built-in styles on a 120 × 60 rectangle at
2 pt, a 137 × 61 rectangle at 5 pt and an open 150 pt line at 4 pt. Its PDF from InDesign 20.0.1
is read back operator by operator with `tools/indesign-export/pdf-strokes.py`, which prints every
stroke's width, cap, dash array and path in page space. `line-ends` adds dashed ellipses with a
`StrokeDashAndGap` of 14 10.

## Decision

**The patterns** (nominal, before fitting):

| Style | On | Off |
|---|---|---|
| `Dashed` | 12 pt | 12 pt, whatever the weight |
| `Canned Dashed 3x2` | 3 × weight | 2 × weight |
| `Canned Dashed 4x4` | 4 × weight | 4 × weight |
| `Canned Dotted` | 0 | 3 × weight, round caps |
| `Japanese Dots` | 0 | 2 × weight, round caps |

A `StrokeDashAndGap` on the item and a custom `<DashedStrokeStyle>` give their own pattern in pt. A
name InDesign does not know draws solid there (`strokes-fills` "Dotted"), so it does here.

**The layout.** Every anchor is a corner and every segment between two anchors is a side,
curves included (each quarter of a dashed ellipse gets its own array).

- A side holds a whole number of periods: its length over the nominal period, a half rounding
  **down** (InDesign's 2.5 → 2, 7.5 → 7, 12.5 → 12).
- A dashed side shares what the rounding leaves equally between dash and gap (`Canned Dashed
  3x2` at 5 pt, nominal 15 / 10, on a 61 pt side: 17.75 / 12.75). A dotted side gives it all to
  the gap.
- At a corner the two sides' dashes meet in one piece, half a fitted dash along each, drawn with
  the stroke's join. A dotted corner is a dot.
- An open path starts and ends on a whole dash (`L = k·dash + (k − 1)·gap`), or on a dot.

`paged_compose::dash_fit::fit_dashes` computes the pieces in page space, and `push_stroke` pushes
them as solid strokes (corners, dashes, and dots as hair-length segments with round caps, so
every rasterizer caps them). Both rasterizers and the PDF writer therefore draw the same dashes.
A pattern with more than two entries is not modelled and keeps the rasterizer's dashing.

## Consequences

- `strokes-fills` p11–p14 ink ΔE 46–52 → 0.6–2.0; its worst page is 0.040. `line-ends` 0.497 →
  0.155.
- **Not modelled yet:** the hash styles (`Left Slant Hash`, `Right Slant Hash`, `Straight
  Hash`) and `White Diamond`. InDesign draws them as hairline marks across the stroke's band
  (pen 0.187 × weight, every half weight; diamonds every weight). Here they stroke solid, which
  is what `stroke-styles` rows 6–9 measure. The striped built-ins and `Wavy` are drawn by
  `emit_styled_stroke`.
- Paragraph rules, table strokes and text decorations keep the rasterizer's dashing; whether
  InDesign fits them the same way was not asked.
