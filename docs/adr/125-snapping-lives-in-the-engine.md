# ADR 125 — Snapping lives in the engine

- **Status:** Accepted 2026-10-05.
- **Scope:** `crates/paged-canvas/src/snap_point.rs`, `crates/paged-canvas/src/snap.rs`,
  `crates/paged-canvas/src/gesture.rs`, the `requestSnapPoint` and `setSnapSettings` message kinds
  (protocol 67)

## Context

Before protocol 67 there were two snapping implementations with different rules.

The engine snapped one gesture, translate. Its targets were the page's edges and centre lines, ruler
guides and the edges and centres of sibling text frames and rectangles. The tolerance was 4 screen
pixels, and Ctrl bypassed it. Resize and path-point edits did not snap.

Drawing and point-editing tools snapped in TypeScript, in the vector plugin and in the editor's Pen
and Direct Selection. Their targets were the page and the points of the path being drawn or edited.
Every other element's geometry was one wire read per element away, so it was not a target. The
tolerance was 6 screen pixels, and Cmd bypassed it.

So moving an object and placing a point snapped to different targets, at different distances, with
different bypass keys. Neither could see the whole page.

## Decision

The engine owns snapping: its targets, its precedence and its settings. Hosts and plugins ask it.

- **Targets** are built once per document build (`SnapIndex`, keyed by the model's build
  generation). They are:
  - every visible leaf's anchors (for a frame drawn from its box, its corners; for an oval, its
    four quadrant points) and each element's centre;
  - the page's corners and centre;
  - the x and y lines through all of those points;
  - the page's edges and centre lines, ruler guides and the document grid;
  - every element's outline.
- **Precedence**, one resolver for every caller: the nearest point within tolerance wins outright.
  Otherwise each axis snaps on its own to the nearest line. Otherwise the point moves to the nearest
  point on an outline.
- **Settings** are session state (`SetSnapSettings`): a master switch, a tolerance in screen pixels
  and one switch per target family. They survive a document load and are neither saved nor undone.
  Grid snapping is off by default, as in InDesign.
- **Query.** `RequestSnapPoint` snaps one page-local point.
  - It can leave out an element, or the anchors being dragged. With anchors named, that element's
    outline is left out too, because the host is previewing a shape the engine has not seen.
  - It can add points only the caller knows, such as the anchors of a path still being drawn.
  - The reply names what the point landed on and the guide lines to draw.
- **Gestures** use the same index and settings. Translate aligns with all five leaf kinds. Resize
  snaps the edges its handle moves. A path-point edit snaps the dragged point through the resolver.
  The index is taken when the gesture begins and held until it ends, because the preview rebuilds
  the document on every tick.

## Consequences

- One tolerance, one precedence and one set of switches cover every tool. A plugin tool snaps
  exactly like the host's tools.
- A point query costs a scan of one page's targets, with distant elements skipped by their bounding
  boxes. It does not walk the document.
- A host whose tool runs its logic outside the engine pays one round trip per pointer sample. The
  editor's Direct Selection snaps locally at once and applies the engine's answer when it arrives
  for the position the drag is still at.
- The plugin-side snapper stays as a fallback for hosts whose engine predates protocol 67.
- Not built: snapping to text baselines or the baseline grid, snapping a rotated frame's edges
  during translate or resize (rotated members still pass through), and per-target tolerances.
