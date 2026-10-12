# ADR 133 — Locks, container deletes and emptied groups follow InDesign

- **Status:** Accepted 2026-10-12.
- **Scope:** `crates/paged-mutate/src/apply/lock.rs`, `crates/paged-mutate/src/apply/remove_node.rs`,
  `crates/paged-mutate/src/apply/insert_node.rs`, `crates/paged-mutate/src/apply/layer.rs`
  (`DissolveGroup`), `crates/paged-canvas/src/gesture.rs`; `OperationError::Locked` and
  `GestureFailure::Locked` (additive, no protocol bump)

## Context

Three operations on page items did something InDesign does not, and each reported success.

- **A lock was not enforced.** `elementLocked` round-tripped and the editor's hit-tester kept a
  locked item out of a click selection, but `moveFrame` and `deleteFrame` applied to a locked frame.
  A selection made in a panel, a script or a plugin went straight through the lock.
- **A container's pasted-in content survived the container.** Deleting the container left the
  children in their lists under a host that no longer existed, the editor showed them as free
  top-level items, and undo brought the container back beside them with the nesting lost.
- **A group outlived its members.** Removing every member left an empty group in the z-order. A batch
  that removed both members of a group and then dissolved it applied, and its single undo restored
  nothing: the dissolve's inverse was a `CreateGroup` with no members, which is refused, so the whole
  inverse failed.

These are behaviour rules rather than rendering, so no exported PDF can answer them. InDesign was
asked by script instead. `tools/indesign-export/object-integrity-probe.jsx` runs each case on a
scratch document, and its answers are committed beside it in `object-integrity-probe.json`
(InDesign 2025, 20.0.1).

## Decision

**A locked page item stays where it is.** The engine refuses an operation that would move, resize,
rotate, scale, flip, reshape, restack or remove a locked item, with `OperationError::Locked` naming
the item. The refused operations are:

- `RemoveNode`;
- `SetProperty` on the box, the transform and its parts, the path and its points, and the placed
  content's transform;
- `MoveNode` and `ReorderNode`;
- a group transform when a member is locked;
- the pathfinder and join verbs on a locked input;
- the drag-a-copy insert.

Everything else applies, because InDesign allows it:

- attribute writes such as fill, stroke, opacity, corner options, effects, nonprinting and
  text-frame options;
- editing the text in a locked text frame;
- `DuplicateNodes` (the copy keeps the lock);
- the write that unlocks the item, which is an ordinary property write.

The content of a locked container counts as locked. The rule is checked once per operation, and
once per batch child, at the top of the mutation dispatch, so the wire, `paged invoke`, `paged.*`
and plugins all meet it. The canvas refuses to begin a gesture on a locked item, so the preview
never moves one.

InDesign ignores a path-point write on a locked item without an error. The engine refuses it like
every other reshape, so the caller learns why nothing changed.

Undo is never refused by the lock. An inverse runs against the state its forward operation left,
and that state cannot include a lock the operation did not set: a lock set later is undone first.
The one insert that could create a locked item, the drag-a-copy, is refused for a locked source.

**Deleting a container deletes what was pasted into it.** InDesign does this too. The removal is the
batch a user could have sent: release and remove each child, then the container. Its inverse puts
the container back, then each child, pasted in at the index it held.

**A group goes with its last member.** In InDesign, removing members keeps the group down to a
group of one, and removing the last member removes the group. `RemoveNode` of a last member now
takes the group out as well, together with every ancestor group that this leaves empty. Each removed
group is captured whole: the model struct, its index in the spread's group list, the list that held
it and its slot there, and its labels. These captures ride inside the removed item's own capture.
Re-inserting the capture re-creates the groups, outermost first, and puts the item back. Removing it
again takes the groups again. Undo and redo are therefore the same pair of operations at any depth.

So a batch that removes a group's members and then dissolves the group is refused whole, because the
dissolve names a group that no longer exists. Removing the members on their own removes the group
too, and one undo restores both. An empty group can still come from a file. Dissolving one removes
it through the same capture, so its undo puts it back.

## Consequences

- A host no longer needs its own guard against moving or deleting a locked item, and a script meets
  the same refusal the editor's tools do. A host that wants InDesign's "unlock, then act" flow sends
  the unlock write first, in the same batch if it should be one undo step.
- A gesture on a selection that holds a locked item fails at `beginGesture`
  (`GestureFailure::Locked`), not at commit.
- Deletion can no longer leave an empty group behind, so the scene tree and the IDML export see an
  empty group only when one came from a file.
- Locking a group as a whole is not modelled (`Group` has no lock), so the rule applies to leaf
  items and to the containers they are pasted into. A group holding a locked member does not move as
  a unit. InDesign cannot produce that state, because it does not lock a group member on its own.
- A layer lock is still enforced only by the hit-tester. Bringing it under the same rule is separate
  work.
