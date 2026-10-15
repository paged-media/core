# ADR 149 — A frame's corners are read per corner, in InDesign's tokens

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-model` (`CornerOption::from_idml`), `paged-renderer` (`per_corner_radii`,
  `per_corner_kinds`, `uniform_corner`, the object-style corner cascade), `paged-gen`
  (`corners`). No wire or protocol change.

## Context

The renderer took a frame's corners from its four per-corner specs and fell back to the
item-level `CornerOption` / `CornerRadius` pair, and the model read bare names (`Rounded`,
`Inset`, `Beveled`, `Fancy`) as options. `corners` (which wrote bare names and only the item-level
pair on its rectangles) measured 0.827 on its inset page, where InDesign's PDF shows square
corners.

## The question to InDesign

A DOM probe opened `corners` in InDesign 20.0.1 and read back every item's
`topLeftCornerOption` … `bottomRightCornerRadius`, for three spellings of the same file:

- **Bare names** (`Rounded`, `Inset`, …): every corner of every item reads `NONE`.
- **InDesign's tokens** (`RoundedCorner`, `InsetCorner`, …) in the item-level `CornerOption` with
  `CornerRadius="48"` on the rectangle: the rectangle's four corners read `NONE`, radius 12. The
  triangle's `TopLeftCornerOption="RoundedCorner"` reads `ROUNDED_CORNER` 20, and its other
  corners read `NONE` 12 next to a stated `CornerRadius="20"`.
- **`BeveledCorner`** reads `NONE`; **`BevelCorner`** reads `BEVEL_CORNER`.

InDesign writes both the item-level pair and the four per-corner attributes (every rounded frame
in the census does). It reads only the four.

## Decision

- **Corners come from the four per-corner specs only.** The item-level pair is kept and
  round-tripped, but it shapes nothing. A per-corner option with no radius takes 12 pt. A
  polygon's corners all follow its `TopLeft` slot (B-23).
- **`CornerOption::from_idml` reads InDesign's tokens** (`None`, `RoundedCorner`,
  `InverseRoundedCorner`, `InsetCorner`, `BevelCorner`, `FancyCorner`) and `BeveledCorner`, the
  member the wire's corner enum carries; the IDML writer spells it `BevelCorner`. Bare names are
  no option.
- **The object-style cascade fills the per-corner specs** of a rectangle that states none of
  its own corners, from the style's corner pair (which `objectStyleCornerOption` writes).
  Superseded 2026-10-15: from the style's own per-corner specs (see the amendment below).
- `corners` and `paste-into` write InDesign's tokens and all four per-corner attributes, as
  InDesign does, and their references are re-exported.

## Consequences

- `corners` p4 0.827 → 0.300; the worst corners page is now the Fancy shape (p5, 0.665), an
  approximation. `paste-into`, whose container InDesign had drawn square (a bare `Rounded`),
  now clips its children to a rounded container in both: 0.042.
- A document that spells corners only in the item-level pair, or in bare names, draws square
  corners, as it does in InDesign.

## Amendment 2026-10-15 — an object style's corners are read per corner too

The decision above took a style's corners from its item-level pair, which was not asked of
InDesign. `object-style-corners-probe` asked it: InDesign authored a style with four rounded
20 pt corners on a rectangle and exported it, the probe rewrote the style's corner attributes
(the rectangle's own removed, so it takes the style's) and InDesign read each variant back
through its DOM (`tools/indesign-export/object-style-corners-probe.json`):

| The style states | The style and its rectangle read |
|---|---|
| the pair alone (`RoundedCorner` 20) | `NONE` 12 on every corner |
| the four per-corner attributes alone | `ROUNDED_CORNER` 20 |
| both, disagreeing (pair `InsetCorner` 30) | `ROUNDED_CORNER` 20, the per-corner ones |
| per-corner options without radii, `CornerRadius="30"` | `ROUNDED_CORNER` 12 |

A style reads its corners as an item does: from the four per-corner attributes, with 12 pt for
an option that has no radius. So:

- `ObjectStyleDef` and `ResolvedObject` carry the four `corners`; each option and radius
  cascades through `BasedOn` on its own, inside the stroke-and-corner category.
- The cascade fills a rectangle that states none of its own corners from those four. A style
  that spells its corners only in the pair gives square corners, as in InDesign.
- `objectStyleCornerOption` / `objectStyleCornerRadius` set the pair and all four corners, so a
  style rounded in Paged opens rounded in InDesign. Undo replays the prior pair onto all four,
  exact whenever the two agreed.
- plugin-publish reads the per-corner attributes of an `<ObjectStyle>`, writes them for a new
  style, and now patches an edited source `<ObjectStyle>` on save (fill, stroke and corners),
  which until now passed through unchanged, so such an edit was lost.
