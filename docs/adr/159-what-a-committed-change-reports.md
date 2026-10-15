# ADR 159 — What a committed change reports, and where a page sits

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `crates/paged-canvas/src/channel.rs`: `GestureCommitted.reflow`, `ScriptResult.writes` and
    `ScriptResult.page_ids`, `PageSummary.page_origin` and `PageSummary.spread_id`.
  - `crates/paged-canvas/src/gesture.rs` (`commit_gesture`) and `src/model.rs`
    (`FrameMutationOutcome.reflow`, the page-change collection, `pages()`, the table entries).
  - `crates/paged-script/src/lib.rs`: the run's own writes and pages, per segment.
  - `crates/paged-mutate` and `crates/paged-introspect`: the read-only `PropertyPath`s
    `TableHeaderRowCount` and `TableFooterRowCount`.
  - `crates/paged-canvas/src/virtual_set.rs`: reading a table's read-only counts by address.
  - `crates/paged-canvas-wasm/src/dispatch.rs` and `crates/paged-cli/src/engine.rs` thread the
    new fields onto the replies.
  - No protocol change: every field is new and optional, and no message kind or operation
    changes. The fields arrive in `v0.71.12`, on protocol 71.

## Context

A host follows the document by the replies that commit a change to it. The plugin SDK reports
five of them to bundles as one change event (plugin-sdk ADR 327): `mutationApplied`,
`undoApplied`, `redoApplied`, `gestureCommitted` and `scriptResult`. Two of those replies
said less than the consumers following them needed, and two reads were missing:

1. **A resize by the handles carried no reflow.** `MutationApplied.reflow` (protocol 38)
   reports a frame's new content box when a `resizeFrame` changes it, and never for a move or a
   transform: pagination must react to a resize and must not react to a transform. Plugins that split a table across a frame chain
   (the spreadsheet and the data plugins) re-split on it. A frame resized by its handles is
   committed as a gesture, and `GestureCommitted` had no such field, so they did not re-split
   after the most common resize there is.
2. **A script reply said nothing about what it changed.** `ScriptResult` carried the output and
   the error. A host could not tell a read-only script from a writing one, nor which pages it
   changed, so the SDK reported every script as a change to every page (`pageIds: []`,
   `pagesUnknown: true`) and every consumer re-read everything after a script that only read.
3. **No page read said where a page sits in its spread.** Only an item's geometry carried
   `pageOrigin`, so a host could place a page's chrome only through an item on it. The editor
   draws selection chrome for a pasteboard item in its spread's coordinates; on a spread whose
   pages hold nothing it had nothing to read the page positions from.
4. **No host door reported a table's header and footer bands.** `tableRowCount` is the total.
   A plugin that refreshes a placed table in place (the spreadsheet plugin, on reopen) must keep
   a header band the host added with `insertHeaderRow`, and without the band sizes it cannot
   tell band rows from body rows. Separately, reading `tableRowCount` by address
   (`RequestGet`, `paged.get('table:…')`) answered "no such path": the table read resolved names
   through the advertised list only, and the read-only counts are deliberately not on it.

## Decision

**`GestureCommitted.reflow`.** A gesture that resized ONE frame by its handles reports the
`FrameReflowInfo` that `MutationApplied.reflow` reports for the equivalent `resizeFrame`: the
frame's id and its committed content box, read back from the scene by the same lookup
(`frame_content_box`). A resize gesture writes the frame's bounds exactly as `resizeFrame` does,
so the two cannot disagree; a test commits both and compares the replies. It is absent for a
move, rotate, scale or shear (display geometry only), for a gesture that did not move, and for a
resize of several frames at once. That last one is a batch, and a batch of `resizeFrame`s reports
no reflow as a mutation either: the field has one frame's shape, and naming one of several would
be a guess. A list form is a later addition if a consumer needs it.

**`ScriptResult.writes` and `ScriptResult.page_ids`.**
- `writes` is the number of writes the run committed to the document itself: each one step the
  engine logged (`paged.set`, an insert, a `paged.undo`, a batch). It is `0` for a read-only
  script. It is counted from the engine's applied sequence over the script's own segments, so an
  edit a user made while the script waited on the host's objects is not the script's. The writes
  the host made answering such a script are not counted either; each answered as a
  `MutationApplied` of its own, so a consumer has already seen them.
- `page_ids` are the pages those writes changed, in document order. The engine already knows the
  pages each build changed ([ADR 027](027-incremental-flow-invalidation.md), plan step 7): a text
  edit of one story narrows to the pages laid out afresh, any other write is every page. The model collects the union of those sets
  while a script segment runs (`begin_page_change_tracking` / `end_page_change_tracking`), and
  the reply names the pages that still exist. This is the knowledge the engine has, at the
  precision it has it: a frame write names every page, as `MutationApplied.page_ids` does for the
  same write. It is never a guess. It can be empty while `writes` is not: a write that changes
  nothing a page draws (a label, text in a story with no face to lay it out in) still changed the
  document, which `writes` says. A reply without `page_ids` (no document, an older worker) means
  the pages are unknown.

**`PageSummary.page_origin` and `PageSummary.spread_id`.** `page_origin` is the page's
top-left in its spread's coordinates, the build's `spread_origin`: the value
`ElementGeometryItem.page_origin` carries for an item on that page, so the two reads agree by
construction (a test reads both). Page 2 of a two-page spread whose first page starts at the
spread's origin is at `[width of page 1, 0]`. `spread_id` is the spread's id as
`SpreadSummary.self_id` spells it.

**`tableHeaderRowCount` and `tableFooterRowCount`.** Two read-only `PropertyPath`s beside
`tableRowCount`, read from the model's `Table.header_row_count` / `footer_row_count` (IDML
`HeaderRowCount` / `FooterRowCount`). They are listed by `RequestElementProperties` on a table
and read by `RequestGet` / `paged.get` on a `table:` address. They are read-only, for the same
reason as the two counts beside them: the bands change through the structure operations
(`insertHeaderRow` / `removeHeaderRow` and their footer twins), which mint and remove the rows
along with the count. A count setter would have to pick one of InDesign's two meanings (Table
Options adds or removes rows; Convert Rows re-designates rows that exist), and no caller needs
it. So they join the catalog's unadvertised list with that reason, as the two table counts did.
The table read now resolves a name against every path, not only the advertised ones, so
`tableRowCount` and `tableColumnCount` read by address as their catalog entry always said.

**No protocol bump.** Every addition is a new optional field or a new read-only
`PropertyPath`, which governance rule 1
([`reference/protocol-governance.md`](../reference/protocol-governance.md)) says never bumps:
`PROTOCOL_VERSION` stays 71 and the fields ship in the `v0.71.12` patch release. An older host
reads every reply as before, and a newer host talking to an older engine simply sees the fields
absent. So a consumer detects them by presence: an absent `reflow`, `writes`, `pageIds`,
`pageOrigin` or `spreadId` means an engine that does not send it, and the consumer keeps the
fallback it has today. A consumer that needs them (and wants to drop its fallback) requires
`@paged-media/canvas-wasm >= 0.71.12` by package version, not by protocol number. The two
band-count paths answer "no such readable path" on an older engine, which is the same signal.

## Consequences

- The plugin SDK re-syncs its vendored wire types and catalog from `@paged-media/canvas-wasm`
  0.71.12 and keys off presence: it passes `reflow` through on `gestureCommitted` when the reply
  carries it; for a script it sends no change event when `writes` is `0`, takes `pageIds` from
  the reply when it is there, and keeps `pageIds: []` / `pagesUnknown: true` when `writes` or
  `pageIds` is absent (an older engine).
- The spreadsheet and data plugins re-split a chained table on a gesture's reflow as on a
  mutation's. The spreadsheet plugin's in-place refresh keeps the header and footer bands the
  host added, reading their sizes.
- The editor places a spread's pages from `PageSummary.page_origin` when the engine sends it,
  and derives them as it does today when it does not.
- A consumer treats an absent field as "this engine does not say", which is what every older
  worker sends; nothing negotiates a protocol for it.

## Tests

`crates/paged-canvas-wasm/tests/committed_change_reports.rs`, through the worker dispatch the wasm canvas runs:
- a handle resize of a frame threaded into a second frame reports the reflow a `resizeFrame` to
  the same bounds reports; a handle move reports none;
- a read-only script reports `writes: 0` and no page; two text edits of the story on page 2
  report `writes: 2` and `["p2"]`; a frame write reports every page;
- page 2 of a two-page spread reports `pageOrigin` x equal to page 1's width, and the same origin
  and spread as the geometry of a frame on it;
- a table read from IDML with `HeaderRowCount="2"` / `FooterRowCount="1"` reads 2 and 1; a table
  with none reads 1 after `insertHeaderRow`; the inspector lists both counts; a `set` of one is
  refused.
