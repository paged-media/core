# ADR 107 — Layout is a pure whole-document build; a batch pays for one rebuild

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-renderer` (`pipeline::build_document`), `crates/paged-canvas` (`CanvasModel`), `crates/paged-mutate` (`apply`)

## Context

The module doc of the canvas model records the first choice: the implementation is
"intentionally synchronous and non-incremental: `load()` parses + builds the whole
document, and every mutation triggers a fresh rebuild", and the point of that phase was "to
nail down the **API surface** the main thread depends on"
(`crates/paged-canvas/src/model.rs:17-23`). The same comment expects a later salsa database
memoising the layout tiers. No `Cargo.toml` in the workspace names such a dependency.

The cost of that choice is stated where it was later bounded: a rebuild "walks the whole
document, so it scales with content rather than with the edit", and a compound authoring
step (insert a frame, pour its text, style it, assign a layer) used to pay it once per
child (`crates/paged-canvas/src/model.rs:1323-1334`, commit `1740f50`).

## Decision

Layout is one function over the whole document, and the batch is the unit that pays for it.

- `pipeline::build_document(&Document, &PipelineOptions)` lays out every page and returns a
  `BuiltDocument` with one display list per page. Running headers and cross-references are
  a second run of the same builder; page growth is a loop of whole builds.
- The renderer keeps no layout between calls. Its one piece of state that outlives a call
  is a thread-local memo of font ids, the hash of each font buffer, capped at 256 entries
  (`crates/paged-renderer/src/pipeline/font_table.rs:608-629`); its comment states that
  output is unchanged. Everything else that makes a rebuild cheaper is owned by the caller
  and passed in as optional `PipelineOptions` fields (decoded images, the font table, the
  CMYK transform, per-master and per-story emission deltas, keep seeds, resume marks, the
  previous pages); every field left `None` gives the cold build. The
  per-paragraph layout cache is installed thread-locally for the duration of one build and
  keyed by a blake3 digest of the layout inputs.
- `CanvasModel` holds the scene and one `BuiltDocument`. A committed mutation, an undo, a
  redo and each gesture update edit the scene and then call `build_document`.
- `Mutation::Batch` costs one build. A batch whose children all translate becomes one
  `Operation::Batch`; a batch that mixes text edits with other operations runs its children
  with the rebuild deferred and settles one build after the last child.
- A batch is all-or-nothing (on a child's failure the applied children are reverted,
  newest first) and is one undo step.
- One layer down, `paged_mutate::apply` rebuilds the document's derived indices wholesale,
  once per top-level call and not once per batch child.

## Evidence

- `crates/paged-renderer/src/pipeline/mod.rs:1189-1302`, `:1341-1379` — `build_document`, its grow loop, and the single re-run for running headers and text-anchor destinations
- `crates/paged-renderer/src/pipeline/mod.rs:339-424` — the caller-owned cache and hint fields, each an `Option`
- `crates/paged-text/src/cache.rs:17-40`, `:243-275` — the paragraph cache, its blake3 key, and `with_layout_cache`
- `crates/paged-canvas/src/model.rs:8779-8832` — `rebuild_after_mutation`: under deferral it sets `rebuild_owed` and returns; otherwise it calls `pipeline::build_document(&self.scene, &options)`
- `crates/paged-canvas/src/model.rs:2306-2364` — `apply_mixed_batch`: atomicity, one rebuild, a nested batch leaves the debt to the outer one
- `crates/paged-canvas/src/model.rs:3218-3236` — a batch that translates whole collapses to one `Operation::Batch`
- `crates/paged-mutate/src/apply/mod.rs:75-97`, `crates/paged-mutate/src/apply/layer.rs:2274-2314` — derived indices rebuilt once per top-level `apply`, with the measurement; `apply_batch` reverts applied children when one fails
- `crates/paged-canvas/tests/batch_composition.rs:884-900` — the test that a four-child mixed batch builds once

## Alternatives considered

- Demand-driven memoisation with salsa: named as the later phase
  (`crates/paged-canvas/src/model.rs:17-19`) and listed as "Explicitly out of scope (later)"
  in `crates/paged-canvas/INCREMENTAL-RELAYOUT.md:156-160`. Not built.
- A rebuild per batch child: the behaviour before commit `1740f50`.
- An ephemeral preview overlay for gestures instead of mutate-and-rebuild: deferred as
  "only worth the complexity once per-update rebuild perf hits a wall"
  (`crates/paged-canvas/src/gesture.rs:28-34`).
- Narrower rebuilds of the derived indices: "weighed and rejected on correctness, not cost"
  (`crates/paged-mutate/src/apply/mod.rs:85-97`).

## Consequences

A cold build is always available as the reference. `digest_gate_check` builds the scene
again with a fresh layout cache and grow hint and without the font table, the emission
deltas, the keep seeds, the resume marks and the emission prints (the decoded-image cache
and the CMYK transform cache stay shared), and compares page ids, display-list digests,
story layout and diagnostics (`crates/paged-canvas/src/model.rs:8917-8972`).

Callers that author documents must put compound edits in one batch; otherwise each child
pays a build. Inside a mixed batch the built pages lag the scene, so nothing in the apply
path may read them (`crates/paged-canvas/src/model.rs:2328-2330`). A batch names its own
creations through `bindCreated` handles and reads the ids from the reply's `minted` list.

The comments are behind the code. `crates/paged-canvas/src/model.rs:15-23` and
`crates/paged-canvas/src/lib.rs:34-45` still describe a first phase with nothing reused,
and `crates/paged-canvas/INCREMENTAL-RELAYOUT.md:49-54` says a text edit clears the whole
body-story cache. Today a text edit of a story that has a frame chain of its own drops only
that story's entries, and the build may adopt the previous build's untouched pages
(`crates/paged-canvas/src/model.rs:8681-8776`, `:8795-8826`). That work is
[ADR 027](027-incremental-flow-invalidation.md). It keeps this rule: the reuse is passed
into `build_document` as options, and the cold build stays the reference.

## Related

- [ADR 027](027-incremental-flow-invalidation.md), [ADR 026](026-auto-growing-region-chains.md) — incremental invalidation and page growth, both on top of this build model
- [ADR 101](101-display-list-single-intermediate.md) — the display list and the digest the gate compares
- [ADR 110](110-one-undo-timeline.md), [ADR 116](116-mutations-lowered-onto-operations.md) — the undo log, and how a wire batch becomes `Operation::Batch`
