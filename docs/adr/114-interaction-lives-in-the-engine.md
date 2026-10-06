# ADR 114 — Interaction lives in the engine: hit testing, selection, gestures, snapping

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-canvas` (`hit.rs`, `selection.rs`, `geometry.rs`, `gesture.rs`, `snap.rs`), the interaction message kinds in `crates/paged-canvas/src/channel.rs`, and their dispatch in `crates/paged-canvas-wasm`

## Context

An editor has to answer which element is under the pointer, where a caret sits, what a
drag does to a frame and where it snaps. Each answer depends on the laid-out document:
paint order across layers, rotated frames, line breaks, threaded stories. That state is in
`CanvasModel`, the "worker-side data model" (`crates/paged-canvas/src/lib.rs:15`).

The repository gives reasons for parts of the design. A hit test "must sort that way or
selection disagrees with what the user sees on multi-layer documents"
(`crates/paged-canvas/src/hit.rs:26-28`). Selection references characters in a story, "not
pixels on a page", so that it "survives re-layout, zoom changes, frame moves, and pagination
shifts" (`crates/paged-canvas/src/selection.rs:19-23`). Element geometry is served "so the
overlay can draw selection chrome without re-deriving the math in TS"
(`crates/paged-canvas/src/channel.rs:924-926`).

Why interaction as a whole belongs to the engine and not to the host:
The repository does not record why. The module comments cite a design note by section number.

## Decision

Hit testing, selection state, caret and selection geometry, gestures and snapping were
implemented in `paged-canvas` as Rust over the model and the built pages. The host sends
pointer facts and receives answers.

- **Hit test.** `CanvasModel::hit_test_filtered` returns the topmost selectable element in
  the renderer's paint order, with oriented containment and without items on locked layers.
  The result is the leaf, its group ancestry, the composed transform and, for text, the
  story offset and table cell. `marquee_hits` answers a rectangle the same way.
- **Selection.** The model holds an element selection and a `ContentSelection`
  `{story_id, start, end, affinity, cell}`, both set by message and described in the code
  as mirrored from the main thread. The element selection never enters the operation log.
- **Text geometry.** Caret position, selection rectangles per line, vertical caret
  navigation and line bounds are queries against the built layout; word and paragraph
  bounds are computed from the story's text (`crates/paged-canvas/src/model.rs:8016-8039`).
- **Gestures.** One gesture is active at a time. `begin_gesture` snapshots the committed
  state of the nodes; `update_gesture` writes a preview into the scene and rebuilds;
  `commit_gesture` restores the snapshot and re-applies the final delta as one operation
  (a batch for several nodes) through `apply_operation`; `cancel_gesture` restores the
  snapshot. All nine `GestureType` variants are accepted.
- **Snapping.** `compute_snap_adjustment` runs inside `update_gesture`, adjusts the delta
  and returns the snap lines for the host to draw. The tolerance is 4 CSS px divided by the
  camera scale passed at begin.

## Evidence

- `crates/paged-canvas/src/hit.rs:17-46` — hit-test rules: paint order, oriented containment, locked layers, group chain; `:194-205` the entry points; `:103-113` marquee by separating axes
- `crates/paged-canvas/src/gesture.rs:17-22` — the four-phase lifecycle; `:414-427` the accepted gesture types; `:496-589` `update_gesture`; `:595-679` `commit_gesture`
- `crates/paged-canvas/src/snap.rs:70-115` — tolerance constant, translate-only and un-rotated-only guards, tolerance divided by `camera_scale`
- `crates/paged-canvas/src/selection.rs:17-48`, `crates/paged-canvas/src/geometry.rs:15-27`, `crates/paged-canvas/src/model.rs:1223-1238` — content-addressed selection, the geometry derived from it, and the model fields that hold selection and the active gesture
- `crates/paged-canvas/src/channel.rs:807-812`, `:829-930`, `:1257-1297` — the message kinds: `HitTest`, selection and caret queries, marquee, element geometry, the four gesture messages
- `crates/paged-cli/tests/cli_surface.rs:62-91` — 15 of the 30 message kinds the CLI does not send carry the reason "interaction: a command line has no pointer and no caret"
- `editor: apps/canvas/src/ui/ViewportCanvas.tsx:596-650`, `:676-700`, `:953-957` — the editor picks a gesture kind and targets, then calls `beginGesture`; a click is a `hitTest` round trip
- `editor: apps/canvas/src/ui/useTextEditing.ts:26-31` — up, down, Home and End go to the engine because they "need line metrics the main thread doesn't have"

## Alternatives considered

An ephemeral preview overlay, instead of mutating the scene and rebuilding on every gesture
update, is named as a later version: "only worth the complexity once per-update rebuild
perf hits a wall" (`crates/paged-canvas/src/gesture.rs:28-34`).

## Consequences

Hit results, selection geometry and what is painted come from one model, and interaction
is tested natively without a browser (`crates/paged-canvas/tests/translate_gesture.rs` and
its neighbours). A gesture commit is one entry in the undo log
([ADR 110](110-one-undo-timeline.md)).

Each gesture update costs a rebuild ([ADR 107](107-whole-document-build.md)), returns every
page id, and clears the GPU scene cache (`crates/paged-canvas-wasm/src/dispatch.rs:1263-1267`).

The host is not free of geometry. The editor classifies a pointer-down by testing it
against the bounding box of geometry the engine returned for the current selection
(`editor: apps/canvas/src/ui/ViewportCanvas.tsx:1127-1160`), and moves the caret left and
right locally.

Snapping applies only to `Translate` and only when no member is rotated. Three comments
are behind the code: `crates/paged-canvas/src/gesture.rs:23-25` says only Translate ships,
`:168` calls Rotate reserved, and `crates/paged-canvas/src/snap.rs:28-30` says the camera
conversion of the tolerance is still to come.

## Related

- [ADR 107](107-whole-document-build.md), [ADR 110](110-one-undo-timeline.md), [ADR 115](115-worker-boundary-transports.md) (the gesture mailbox), [ADR 117](117-story-local-text-offsets.md) (the offsets a `ContentSelection` carries)
- [ADR 008](008-read-surfaces-first-class-wire-collections.md) — read surfaces served by the engine as wire collections
- [ADR 202](https://github.com/paged-media/editor/blob/main/docs/adr/202-render-worker-owns-the-canvas.md), [ADR 208](https://github.com/paged-media/editor/blob/main/docs/adr/208-tools-are-data-plus-gesture-handler.md) — the editor side
- `../design/canvas-interaction.md` — the gesture sections the module comments cite by number
