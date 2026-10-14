# ADR 151 — Which side text wraps to, and the contour it wraps round

- **Status:** Accepted 2026-10-14.
- **Scope:**
  - `paged-model`: `TextWrap::side` / `TextWrapSide`, `TextWrap::apply_to_master_page_only`,
    `TextFrame::ignore_wrap`.
  - `paged-renderer`: `WrapShape`'s side and contour, the wrap collector, master-item wraps, the
    per-line carve, and `rectangle_drawn_from_path`.
  - `paged-gen`: the `text-wrap-sides` sample.
  - plugin-publish `idml-import` / `idml-export`: `TextWrapSide`, `ApplyToMasterPageOnly`,
    `IgnoreWrap`, and a `<Rectangle>`'s non-box path.
  - No wire or protocol change.

## Context

A wrap obstacle pushed text to both of its sides whatever `TextWrapSide` said. Three more things
were wrong:
- A contour wrap was the obstacle's bounding box.
- A text frame's `IgnoreWrap` was not read.
- A master item's wrap acted on nothing: the wrap collector walked the live spreads only.

The importer kept a `<Rectangle>`'s path only when it had more than four anchors, so an oval or
a triangle drawn as a rectangle element painted, and wrapped, as its box.

## The question to InDesign

`text-wrap-sides` (a `paged-gen` sample) is a facing Letter document with one case per page:
- a 468 × 612 frame of 12 pt text with hyphenation off;
- one obstacle, offset 6 pt.

Its PDF from InDesign 20.0.1 was read:
- as pages;
- for the contour cases, line by line from the content stream's text matrices (`Tm`/`Td`), which
  give each segment's exact start and baseline.

## Decision

**Sides.**
- `LeftSide` keeps text left of the obstacle and `RightSide` right of it.
- `LargestArea` takes whichever side has more room on the line.
- The spine sides resolve from the page. A verso's spine is on its right, so text towards the
  spine stands right of the obstacle there and left of it on a recto; away from the spine is the
  reverse.
- The resolved side turns the obstacle's zone on a line into the span the line loses:
  - both sides lose `wl..wr`;
  - text on the left loses everything from `wl` on;
  - text on the right loses everything up to `wr`.
- The heuristic that shrinks the column beside a frame-height obstacle takes the named side too.

**Contour.**
- A contour wrap's zone is the outline grown with round joins by the wrap's top offset plus the
  outer half of a visible stroke.
- The triangle's bottom corner reaches exactly 386 + 6 + 0.25 = 392.25. That is where its line
  starts once floored.
- A line's zone is the grown outline's extent across the line's slug, from the previous
  baseline down to its own.
- Both ends are floored onto the text area's whole-point grid, the grid a shaped frame's bands
  sit on. Worked by hand from InDesign's text matrices, this gives all ten right-side starts
  beside the triangle (317, 326, 334 … 392). On both contour pages every line breaks where
  InDesign's does (the text gate in lines mode).
- The outline is the item's path (an oval's ellipse, a polygon's or rectangle's anchors),
  sampled every 0.25 pt.
- The grown extent is computed exactly from those samples: a point a distance `dy` above or
  below the slug still reaches `sqrt(r² − dy²)` either side.

**IgnoreWrap.** A frame with `IgnoreWrap="true"` composes as if no page item wrapped.

**Master items.**
- A master item's wrap acts on the text of every page that shows the item: not hidden by
  `ShowMasterItems`, not overridden, and on that page's master page.
- `ApplyToMasterPageOnly="true"` keeps it to the master's own text.

**Rectangles.** A `<Rectangle>` paints, and wraps, its path whenever the path is not the plain
axis-aligned box. This is the same test a text frame already used.

## Consequences

- `text-wrap-sides` pages 1–14 measure mean ΔE 0.33–0.54. Before this ADR, pages 2–13 measured
  4.9–5.4.
- **Inverse is measured, not modelled.** InDesign sets the text of an inverted wrap's band
  inside the shape, eroded by the offset; it is hidden under the obstacle's Paper fill.
  - Rows whose slug leaves the eroded shape stay empty, and rows outside the shape's height
    are untouched.
  - The composer cannot leave a row empty in mid-frame, so the engine still wraps round an
    inverted obstacle.
  - The case is page 15 of the sample, outside `max_pages_with_pdf`.
- A contour from an image (`DetectEdges`, `AlphaChannel`, `PhotoshopPath`) uses the frame's path.
- An object style's `IgnoreWrap` is not read.
