# ADR 128 — Pages a merge can make: page handles, copied stories, margins

- **Status:** Accepted 2026-10-06.
- **Scope:** `crates/paged-canvas/src/batch_handles.rs`, `crates/paged-canvas/src/model.rs`
  (the mixed batch lane, `page_insert_context`), `crates/paged-mutate/src/apply/duplicate_page.rs`,
  `crates/paged-mutate/src/apply/duplicate_nodes.rs`, `crates/paged-mutate/src/apply/batch_page.rs`,
  the inverse-only `Operation::RemovePageClone` (protocol 69, no new wire kind)

## Context

A Data Merge places records on as many pages as the records need. Run against the engine with a
template that InDesign wrote, the data-publishing plugin found four gaps:

1. **A page minted in a batch could not be named.** `bindCreated` named elements only, so a batch
   that added pages could not place anything on them. A merge that added pages took two undo steps:
   the pages, then the content.
2. **`duplicatePage` was not a copy.** The clone's frames named the source's stories, so editing the
   copy edited the original. The page's margins were keyed by the source page's id and did not
   follow the clone. Ids were minted from the spreads alone and could repeat a hyperlink's id.
3. **`insertPage` made pages with no margins.**
4. **`duplicateElements` refused a story holding a hyperlink source.** Every Data Merge placeholder
   is a `<HyperlinkTextSource>`, so no template frame could be duplicated.

## Decision

- **A page is a handle.** After an `insertPage` or `duplicatePage` child, `bindCreated` binds the
  page that child minted. `$h:<name>` resolves in a page position (`pageId`, `afterPageId`, `page`,
  `atPage`) and nowhere else. An element handle in a page position fails the batch, and so does a
  page handle anywhere else. A page is not an element, so it is not listed in the reply's `minted`.
  The translated lane learns ids before apply and cannot know a page's, so a handle batch that
  creates a page takes the sequential lane. That lane reads the minted page off the scene after the
  child. A page-addressed insert there reads the page's origin off the scene, because the build is
  deferred to the batch's end. Pages and their content are one undo step.
- **A duplicated page owns copies.** Each distinct story on the page is copied under a fresh
  `Story/u<n>`. Frames threaded to each other on the page stay threaded in the copy, and a thread
  that leaves the page is cut. Margins, labels, image metadata, pasted-in children and opacity
  masks are re-keyed to the clone's ids. Ids come from the document's whole shared `u<hex>` line.
  A story holding a table, an anchored object or a footnote is refused, by the rule
  `duplicateElements` already applies: those carry ids of their own. The inverse is
  `RemovePageClone`, which removes the spread, the copied stories and their hyperlinks. Redo
  restores the captured clone with the same ids. A capture written before this change still
  decodes.
- **A new page takes its master's margins**, else those of the page it follows. This follows
  InDesign, where a page's `<MarginPreference>` follows its master's until the page overrides it.
- **A copied story owns its hyperlink sources.** Each distinct source in the copy gets a fresh id.
  Each designmap `<Hyperlink>` that owned the original is copied onto it with the same destination
  and a unique name. These ids are derived from the copy's story id (`<story>_hs<k>`,
  `<story>_hl<k>`), not drawn from the `u<hex>` line. The story id is unique and stable across
  undo and redo, so the derived ids are too. They also cannot collide with ids the canvas minted
  ahead of apply for later children of the same batch. `duplicateElements` and `duplicatePage`
  share this code.

## Not decided here

- **The paragraph mark between adjacent placeholders.** An InDesign template
  `<<name>>¶<<subtitle>>` read back as `<<name>><<¶subtitle>>`. This is a defect in the IDML
  reader, which lives in the adapter repository and is fixed there. The engine test is ignored
  until the workspace's `idml-import` pin includes that fix.
- **Overset in a host without fonts.** The engine reports overset from its own layout, headless as
  well. A host that loads a document with no font gets no layout, so `overset: false` then means
  "not measured". Supplying a font is the host's job (`loadDocumentDirect`'s font, or font
  registration).

## Evidence

`crates/paged-canvas/tests/data_merge_pages.rs` runs against an InDesign Data Merge template
(`tests/fixtures/data-merge-empty-field-lines.idml`):

- pages and their content in one batch and one undo step, for an inserted and a duplicated page;
- handle kinds refused in each other's positions;
- a duplicated page with its own story copies, fresh hyperlinks, its margins, and an undo and redo
  that take and restore all of it;
- master and neighbour margins for an inserted page;
- `duplicateElements` of a story holding placeholders;
- overset with and without a font;
- the merge writer's exact batch sequence: merge, re-merge, then four undos back to the template.

Every case except the last two fails without its fix. The last two passed before this change and
pin behaviour.
