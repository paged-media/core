# ADR 116 — Wire mutations are lowered onto invertible operations

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `Mutation` in `crates/paged-wire`, `Operation` and `ids` in `crates/paged-mutate`, the translation and id minting in `crates/paged-canvas/src/model.rs`

## Context

`paged-mutate` is built around one type. Its crate documentation calls `Operation` "a
single typed, serializable, invertible" value that is "the sole committed mutation
surface" for the inspector, scripting, gesture commits and undo
(`crates/paged-mutate/src/lib.rs:16-20`). An operation addresses a node by its `Self` id,
"never by path or index, so an Op generated on one client applies meaningfully on another
even after the tree has shuffled" (`crates/paged-mutate/src/operation.rs:56-58`).

The message channel carries a second enum, `Mutation`. It speaks in the terms a client
has: a page id and page-local bounds for a new frame, a story id and offsets for text.
The repository records why ids come from one counter (below). For having two enums rather
than exposing `Operation` on the channel: The repository does not record why.

## Decision

A client sends a `Mutation`; the canvas model lowers it onto an `Operation`, which
`paged_mutate::apply` executes and inverts. The engine mints the ids of what is created.

- **Two enums.** `Mutation` (118 names, serialised as `{op, args}`) lives in `paged-wire`
  and borrows payload types such as `PropertyPath` and `Value` from `paged-mutate`.
  `Operation` (87 variants, serialised as `{kind}`) lives in `paged-mutate`, which does
  not depend on `paged-wire` or `paged-canvas`.
- **The bridge.** `CanvasModel::apply_mutation` calls
  `try_translate_frame_mutation_to_operation`; a translated operation goes to
  `apply_operation`, which runs `paged_mutate::apply`, rebuilds and logs the returned
  `AppliedOperation {op, inverse, invalidation}`. A `Batch` whose children all translate
  becomes one `Operation::Batch`.
- **What is not lowered.** `InsertText` and `DeleteRange` become a `TextOp` in
  `paged-canvas`, with its own captured inverse. Five settings mutations
  (`SetDocumentDefaults`, `SetColorSettings`, `SetProofSetup`, `SetInkSetting`,
  `SetUseStandardLabForSpots`) are handled in `apply_mutation` itself; sent on their own
  they leave no undo entry.
- **Ids.** Mutations that create page items, pages or groups carry no id. The model mints
  `u<hex>` ids at translation time as the successor of the highest such number in the
  document, with a counter threaded through a batch so each child gets a distinct id. The
  group-id minter in `paged-mutate` reads the same floor. `InsertPage` is translated with
  empty id fields, which the apply layer fills on the operation it returns, so redo creates
  the same ids. The style-creating mutations accept an optional `self_id` from the caller.
- **Reporting.** The reply carries `created_id` and, for a batch, `minted`: every element
  created, in order.

## Evidence

- `crates/paged-wire/src/lib.rs:610-626` — `Mutation` and its `{op, args}` tagging; `:1686-1805` the roster; `crates/paged-script/tests/script_surface.rs:149` asserts 118 names
- `crates/paged-mutate/src/operation.rs:2783-2790` — `Operation`: "The canonical mutation primitive. A closed set, extended only with deliberation"; the enum body runs to `:3826`
- `crates/paged-canvas/src/model.rs:2116-2123` — routing through the bridge; `:2647-2660` the translation function and `mint_offset`; `:4107-4153` `apply_operation`
- `crates/paged-canvas/src/model.rs:2200-2238` — a text edit "has no `paged_mutate::Operation` form"; the `TextOp` lane; `:1898-2115` the five mutations handled inline
- `crates/paged-mutate/src/ids.rs:15-34` — one `u<hex>` number line, and the duplicate ids a per-kind scan produced; `crates/paged-canvas/src/model.rs:3972-3989` the minter; `crates/paged-mutate/src/apply/layer.rs:392-400` its twin for groups
- `crates/paged-mutate/src/operation.rs:2913-2915` — `InsertPage` ids "are filled on the op echo so redo re-creates the exact ids"
- `crates/paged-canvas/src/channel.rs:540-567` — `MintedElement` and why a batch reports every mint

## Alternatives considered

- `Operation` as the only vocabulary is what the `paged-mutate` crate documentation still
  describes. A raw-operation entry remains on the inspector binding
  (`crates/paged-introspect-wasm/src/lib.rs:103`).
- A minter that scanned only the page-item kinds for the next free id came first. It handed
  out a number twice: "a real document came back holding `Hyperlink/ueef094` twice with a
  table `ueef094` beside them" (`crates/paged-mutate/src/ids.rs:27-32`).

## Consequences

Minted ids follow the numbering the `ids` module attributes to InDesign: one document-wide
counter. Decimal namespaces (`Story/u<n>`, `Section/u<n>`, `Color/u<n>`) and guide ids are
numbered separately (`crates/paged-mutate/src/ids.rs:36-39`). A caller cannot know an id
before the engine mints it, so a batch needs handles (`bindCreated`, `$h:<name>`) to
address what an earlier child created (`crates/paged-canvas/src/batch_handles.rs:17-21`).

A mutation that existing operations cannot express needs a variant in both enums and a
translation arm. The reply type is shaped for text: a non-text mutation returns an empty
`TextOp` as its `inverse`; "Future convergence folds both into one shape"
(`crates/paged-canvas/src/model.rs:2116-2121`).

Comments are behind the code. `crates/paged-mutate/src/lib.rs:30-31` and
`crates/paged-mutate/src/operation.rs:14-17` describe five variants.
`crates/paged-wire/src/lib.rs:610-611` says the worker rejects every `Mutation`.
`crates/paged-canvas/src/model.rs:2647-2650` says the bridge returns `None` for `MoveFrame`
and `InsertFrame`; it translates both (`:2681`, `:2692`).

## Related

- [ADR 005](005-wire-recipe.md), [ADR 110](110-one-undo-timeline.md) — every operation self-describing and invertible; the log that stores `AppliedOperation` and `TextOp` inverses
- [ADR 111](111-wire-vocabulary-leaf-crate.md), [ADR 107](107-whole-document-build.md) — where `Mutation` lives; why a batch is one rebuild
- [ADR 010](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/010-raw-mutate-gate-capability-enforcement.md) — the gate on raw mutation for plugins
