# Canvas Interaction Plan — selection, gestures, transforms

May 2026. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

Sections not relevant outside the original planning context have been removed; numbering is unchanged.

*Status note (2026-10-02): the two sections kept here are the ones that source comments cite by number (`crates/paged-canvas/src/gesture.rs`). What was built from them is recorded in [ADR 114](../adr/114-interaction-lives-in-the-engine.md) (hit testing, selection, gestures, snapping) and [ADR 110](../adr/110-one-undo-timeline.md) (the undo timeline). The companion concept is [canvas.md](canvas.md); its §11.3 defines the acceptance criterion AC-E-9 cited in §2.4.*

---

## 2. Architectural commitments (load-bearing)

*Status note (2026-10-02): these commitments were built, with one difference in §2.1: the preview is not held in an ephemeral overlay. `update` writes the preview into the scene and rebuilds, and `commit` restores the snapshot and applies one operation (`crates/paged-canvas/src/gesture.rs`). See [ADR 114](../adr/114-interaction-lives-in-the-engine.md).*

These are decided up front; everything below conforms to them.

### 2.1 Two channels: Operation (committed) vs Gesture (ephemeral)

Per the editor architecture (an internal design note): a drag produces **one** `Operation` at
commit, not one per pointer frame. During the drag, an **ephemeral
overlay** holds the in-progress transform and the renderer draws from
it. On `commit`, the worker diffs the overlay against the committed
state and applies exactly one `paged-mutate::Operation` (or a `Batch`
for multi-select), which yields the inverse + invalidation for free.
On `cancel`, the overlay is dropped and nothing enters the log.

### 2.2 Gesture geometry lives in Rust (`paged-canvas`), not TypeScript

Rotation-about-pivot, locked-aspect scale, marquee over rotated
objects, and snapping are real geometry and belong in one place — the
same crate as the rest of the renderer math. The TS layer's job stays
narrow: receive pointer events, decide the active tool + what was hit,
call `begin/update/commit/cancel` on the worker, and draw the 2D
overlay chrome (handles, marquee rect, snap lines). **No transform math
in TS** beyond the camera viewport↔doc mapping that already exists.

### 2.3 Document state vs application state

- **Document state** (persisted, undoable, in the scene graph, mutated
  only via Operations): frame bounds, `ItemTransform`, fill, z-order.
- **Application state** (per-user, ephemeral, *not* in the Operation
  log): the element-selection set, the active tool, the viewport, and
  the in-flight gesture overlay. Element selection lives in the canvas
  app + a worker mirror (so geometry queries have a stable read), the
  same split the text `ContentSelection` already uses via
  `SetSelection`. **Selecting a frame never produces an Operation** and
  Cmd-Z never changes selection.

### 2.4 Coordinate spaces

Four spaces, with the conversions already available:

```
viewport px ──Camera.to_document──► document pt
document pt ──(− page origin)─────► page-local pt   (per layout.ts pageRects / built_page.spread_origin)
page-local  ──(+ spread_origin)───► spread pt
spread pt   ──item_transform──────► the frame's own content-box coords
```

Hit-testing, gesture deltas, and overlay handles all operate in
**spread / document pt** and only convert to viewport px at draw time,
so everything is zoom-independent by construction (AC-E-9).

---

## 3. The mutation model for transforms

*Status note (2026-10-02): this section predates the implementation; `apply.rs` is now the module `crates/paged-mutate/src/apply/`. `PropertyPath::FrameTransform` (§3.2) exists in `crates/paged-mutate/src/operation.rs`, and `crates/paged-mutate/src/apply/set_property.rs` handles frame bounds and frame transforms for text frames, rectangles, ovals, polygons and graphic lines (the transform also for groups). See [ADR 116](../adr/116-mutations-lowered-onto-operations.md).*

### 3.1 Move and resize reuse `FrameBounds` (no new Operation needed)

A **move** shifts all four bounds by `(dx, dy)`; a **resize** changes
the dragged edge(s). Both are expressible *today* as
`SetProperty{ FrameBounds, Value::Bounds }` for TextFrame and
Rectangle — `apply.rs` already implements it with inverse + the
`frame_geometry` invalidation hint. This makes translate/resize the
cheapest possible first slice: **no `paged-mutate` change required**,
only the gesture spine + overlay around it.

Caveat — the move-via-bounds vs move-via-transform decision: IDML
frames carry both `bounds` (the content box) and an optional
`item_transform` (placement + rotation + scale). For an
**un-rotated** frame, editing bounds is exact and keeps text reflow
intuitive. For a **rotated** frame, a screen-space translation must be
applied in the *parent* space, i.e. composed into `item_transform.tx/ty`,
not into bounds (which live in the rotated content-box space). So:

- Un-rotated frame, move/resize → `SetProperty{FrameBounds}`.
- Rotated frame, move → `SetProperty{FrameTransform}` (§3.2),
  composing the world-space delta through the inverse of the rotation.

### 3.2 Rotate and scale need a `FrameTransform` Operation (new)

Add to `paged-mutate`:

- `PropertyPath::FrameTransform`
- `Value::Transform([f32; 6])` (the 2D affine `[a b c d tx ty]`)
- `apply.rs`: handle `(TextFrame|Rectangle, FrameTransform)` — read the
  current `item_transform` (default identity `[1 0 0 1 0 0]` when
  `None`), set the new matrix, return `Value::Transform(prev)` as the
  inverse value. `invert_set_property` already handles this generically.
- Invalidation: `frame_geometry` (rotation/scale don't reflow text;
  matrix scale of a text frame is a *visual* scale, not a reflow).

This is a contained, ~1-file extension mirroring the existing
`FrameBounds` arm. Oval / Polygon / GraphicLine transform support
follows the same pattern when those node kinds graduate from
`apply.rs`'s Stage-1 set (currently TextFrame + Rectangle only).

### 3.3 The gesture → Operation mapping

| Gesture | Commit Operation |
|---|---|
| Translate (un-rotated) | `SetProperty{FrameBounds}` (shift all four) |
| Translate (rotated) | `SetProperty{FrameTransform}` |
| Resize edge/corner (text/rect) | `SetProperty{FrameBounds}` |
| Rotate about pivot | `SetProperty{FrameTransform}` |
| Scale about pivot | `SetProperty{FrameTransform}` (or `FrameBounds` for "resize" semantics) |
| Multi-select any of the above | `Operation::Batch{ ops }` (one per node) |

### 3.4 Ephemeral overlay → preview rendering

*Status note (2026-10-02): not built. A gesture preview is written into the scene and followed by a rebuild; the module comment of `crates/paged-canvas/src/gesture.rs` names the overlay as a later step. See [ADR 107](../adr/107-whole-document-build.md).*

The gesture overlay is a small map `node_id → TransformOverride`
(either a replacement `item_transform` or a replacement `bounds`) held
on `CanvasModel`. The Tier-4 display-list build for the affected page
composes the override when present. Because a gesture touches only the
selected nodes (usually 1, rarely dozens), re-emitting **just the
affected page's** display list per `update` is cheap and reuses the
existing dirty-page → re-render path (`PagesDirty` / `MutationApplied`
already carry `page_ids`). No new render architecture; the override is
an extra lookup in the page slice.

### 3.5 Bridging the mutation-log fork

*Status note (2026-10-02): built. `paged-canvas` depends on `paged-mutate`, and `LoggedMutation` in `crates/paged-canvas/src/model.rs` is the entry type of the one undo stack; text edits keep their own `TextOp` entries in it. See [ADR 110](../adr/110-one-undo-timeline.md).*

`CanvasModel` currently owns a `TextOp` undo log; `paged-mutate` owns
the `Operation` log; the two are disjoint and `paged-canvas` doesn't
depend on `paged-mutate`. For frame gestures we need the `Operation`
log. Recommended bridge (smallest step that unifies undo):

1. Add `paged-mutate` as a dependency of `paged-canvas`.
2. Generalize `CanvasModel`'s undo log entry from `TextOp` to an enum
   `LoggedMutation { Text(TextOp), Frame(paged_mutate::AppliedOperation) }`
   so a single ordered undo stack covers both text edits and frame
   transforms (users expect one Cmd-Z timeline).
3. Route `Mutation::MoveFrame` / `ResizeFrame` (and new gesture-commit
   messages) through `paged_mutate::apply`, pushing the
   `AppliedOperation` onto the unified log.
4. Leave the `TextOp` path as-is for now; the eventual full convergence
   (folding `TextOp` into `paged_mutate::Operation`) is tracked
   separately and is **out of scope** for this plan.

This is the one cross-crate structural change; it should land early
because every committed gesture depends on it.
