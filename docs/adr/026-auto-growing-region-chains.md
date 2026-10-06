# ADR 026 — Auto-growing region chains: pages grow in core, at composition time

- **Status:** ACCEPTED 2026-10-02 (was PROPOSED 2026-10-01); implemented in
  `crates/paged-scene/src/grow.rs`, `crates/paged-renderer/src/pipeline/mod.rs`
  (`build_document`) and the wire operation `setFlowGrowRule` (protocol 64). This is a
  forward-looking **ADR-as-memo**.
- **Supersedes:** nothing. **Extends:** [020](https://github.com/paged-media/plugin-web/blob/main/docs/adr/020-paged-web-native-engine-defer-frame-threading.md)
  and [021](021-paged-native-document-model-idml-as-format.md). Both introduced the `FlowId` /
  region-chain seam; this ADR gives that seam its missing behaviour, growing the chain when
  content oversets.
- **Companions:** [027](027-incremental-flow-invalidation.md) (what an edit must re-lay once
  pages can appear), [028](028-pagination-rules-are-engine-owned.md) (where a page ends),
  ADR 029 (the first consumer).
- **Applies to:** `core` (`paged-flow`, `paged-composition`, `paged-renderer`, `paged-canvas`,
  the wire), `plugin-sdk` (vendored wire), the editor (pins), and plugin-doc as the consumer.

## The problem

A Word document is paginated by nature. Its page count is an **output** of layout. paged
cannot express that today. Verified on core `main` (2026-10-01):

- **No page generation.** Text flows only through the frame chain the document authored.
  Overflow becomes overset, and `paged-flow` says so in its own docs: "today's IDML story
  emitter … discards overflow and only emits an `OversetTextDropped` diagnostic"
  (`crates/paged-flow/src/lib.rs:29-31`). No smart-text-reflow logic exists. The only way to
  add a page is the explicit `insertPage` mutation.
- **The seam exists but is unused.** `run_flow` returns `Overset::Remains(cursor)`. Its doc
  comment says that is where the composition "can grow the chain, report the overset honestly,
  or continue the flow" (`crates/paged-flow/src/lib.rs:209-216`). Nothing grows it yet.
- **The consumer is blocked.** plugin-doc pours a whole DOCX into **one** text frame on the
  current page (`plugin-doc: packages/doc-bundle/src/place.ts:70-120`). Standalone open is
  explicitly deferred (`plugin-doc: packages/doc-bundle/src/activate.ts:70-75`). The plugin's own design promises "an
  auto-generated page/region chain" (`plugin-doc: docs/concept.md`), and the engine
  cannot yet supply one.

## Options weighed

| | Option | Verdict |
|---|---|---|
| A | **The plugin paginates through mutations.** Count the pages needed at import and issue `insertPage` + `insertTextFrame` + `linkFrames`, the way `sheet-lower/paginate.rs` does. | Rejected for editing. It works once, at import. After that, every keystroke that grows the text needs someone to notice the overset and add a page. Each page is a mutation, and each mutation is a whole-document rebuild (~14 s in wasm on the 134-page annual, as measured at the time). The result is a loop across the plugin boundary, and the added pages become authored content that pollutes undo. |
| B | **The host editor paginates.** The editor watches overset diagnostics and issues page mutations. | Rejected. Same rebuild loop as A, and the editor is the wrong place for it: the CLI, Boa scripts and the headless export would all paginate differently. |
| **C** | **Core grows the chain at composition time.** When a flow ends with `Overset::Remains` on a chain marked *auto-grow*, the composition adds a page from the chain's rule and keeps flowing. It removes generated pages that end up empty. | **Chosen.** |

## The decision

1. **Page growth is a property of a region chain, evaluated by the composition.** A chain
   carries an optional **grow rule**: which master (template) a new page is cloned from,
   which region on it continues the flow, where pages are inserted (end of the chain's
   section), and whether empty generated pages are removed (default yes).
2. **Generated pages are derived, not authored.** They do not exist in the saved model. A
   `.paged` file stores the rule, and opening the file regenerates the pages. Undo never
   records a generated page. Undoing the edit that caused one makes it disappear on the next
   layout. This is the [007](007-carry-through-rendering-honesty.md) principle applied to
   pages: the saved file says what was authored.
3. **Generated pages have stable identities.** A generated page's id is derived from (chain
   id, ordinal within the chain), not from its absolute document index. Adding a page early in
   a document must not renumber the identity of every page after it ([027](027-incremental-flow-invalidation.md) depends on this).
4. **It lands on the composition model, not the IDML scene.** `paged-composition` already
   projects a flow into a `paged_flow::RegionChain` (`Composition::flow_chain`). Growth is a
   composition-level step around `run_flow`. The IDML path reaches it through the existing
   `paged_scene::Document::flow_chain` adapter. IDML export **materialises** generated pages
   as ordinary pages, as InDesign's own smart-text-reflow export does.
5. **One wire addition, one protocol bump.** The grow rule is new authored state, so it
   needs a setter: a `setFlowGrowRule` op plus the matching property path. That is a protocol
   bump (63 → 64), following [006](006-protocol-coupled-versioning.md) and the bump chain:
   core tag → plugin-sdk re-syncs the vendored wire → editor moves its pins. The read side
   (pages collection, page layout) needs no shape change, because generated pages are pages.
6. **Reachable from every surface.** The wire, Boa `paged.*`, the CLI and the capability
   matrix get it in the same change. The read-only viewer SDK renders generated pages with no
   mutation code. Verify this on the light consumer alone: a `--workspace` build unifies features and never compiles the light path.

## Consequences

- **The page count becomes an output of layout.** Everything that read "page N" as a stable
  input now depends on layout: page-number variables, running headers, cross-references, the
  pages collection, and the editor's page strip. The first three already re-emit after layout
  (`crates/paged-renderer/src/pipeline/links.rs:45-53`). The editor must treat a page-set change as
  ordinary output, not as a structural event.
- **The cache key must stop using absolute page indices.** The body-story emit signature
  hashes the absolute page index (`crates/paged-renderer/src/pipeline/deltas.rs:55`). With growth,
  one added page would miss every cache entry after it. Fixed in [027](027-incremental-flow-invalidation.md).
- **Growth can loop.** A grown page brings master content: wrap shapes, and running headers
  that change the text. A bounded loop (re-flow until the page set stops changing, capped,
  with a diagnostic at the cap) is required. Today's dependencies go one way, so the loop is
  new and must be tested.
- **Selection, caret and comments survive a regrown page** only because ids are stable
  (decision 3).

## What this ADR does NOT decide

- Where a page ends inside a story (keep rules, widows/orphans, breaks): [028](028-pagination-rules-are-engine-owned.md).
- How Word sections map to grow rules: ADR 029.
- Whether IDML documents can opt into growth in the editor UI (an InDesign-style "Smart Text
  Reflow" preference). The mechanism allows it; the product decision is separate.
- Growing a chain *sideways* (adding frames on an existing page rather than whole pages).

## 2026-10-01 addendum: what InDesign does, measured, and the first implementation

**Asked InDesign** (`tools/indesign-export/reflow-probe.sh` on the paged-gen
`reflow` fixture: two pages, one threaded frame each, 33 lines a frame, 80
one-line paragraphs). Smart Text Reflow is an **idle task**: it never runs
while a script runs and never on open, so the probe edits in one script and
measures in a second. Answers:

| Question | InDesign 2025 |
|---|---|
| Does a lone unthreaded frame grow? | No. Only a threaded chain (limit-to-primary off) |
| Where do pages go? | After the page of the chain's last frame (end of story) |
| Which master? | The master of that last page |
| Frame geometry? | The new page's **margin box**: neither the master frame nor the last frame (both probed separately) |
| Frame options? | **Defaults**: page 3's first line sits 2.43 pt higher (ascent offset, not the chain's `LeadingOffset`) |
| Shrinking? | Every reflow-chain page that ends up empty is deleted, **authored pages included** |

The last row differs from §2: InDesign deletes authored pages, while this
engine keeps them (generated pages are derived and are the only ones
removed). For DOCX the two coincide, since only generated pages follow the
first.

**Implemented (core d384aaf):** `FlowGrowRule` on `Story` (rule-only, serde
default; never imported from IDML, which matches InDesign not reflowing on
open). `paged_scene::Document::with_generated_pages` materialises N pages per
growing story into a copy, with stable ids `<story>_grow<k>_{page,frame,spread}`
(first written as `~grow~`; changed in core `190a5c2` because `~` is not valid in
an XML name, and an IDML export writes these ids as `Self` attributes and part names).
`pipeline::build_document` wraps the old build: it doubles a story's count
while it oversets, drops trailing generated frames with no line, and carries
counts between builds (`PipelineOptions::grow_hint`). Documents without
rules take the old single build. `paged-inspect --grow-story`.

**Verified against InDesign:** `crates/paged-renderer/tests/reflow_pipeline.rs` (one page added, 14
lines on page 3, paragraph 67 opens it, margin-box frame, stable ids, a fitting
story generates nothing, the hint settles and shrinks). The fidelity gate
renders the fixture with growth against InDesign's REFLOWED PDF: all three
pages mean ΔE ≈ 0.85, the generated page included.

**Found on the way:** `LeadingOffset` first baselines took the minimum
offset or 0.8 × point size instead of the first line's leading, so every such
frame sat 4 pt high (at 10/12 pt). Fixed in the same change (InDesign: the
leading, never less than the minimum).

**Not yet:** the wire setter (`setFlowGrowRule`, protocol 64), the canvas
model carrying `grow_hint`, IDML export materialising generated pages, Word
section → master mapping (ADR 029).

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. Growth is built and reachable: a story with a grow
rule gains generated pages while it oversets and loses the empty ones, in every build that
goes through `pipeline::build_document`. Three of the four "Not yet" items are done, and the
implementation differs from the text above in the places listed below.

**1. Built since the addendum.** This supersedes its "Not yet" paragraph, except for the Word
section mapping, which belongs to plugin-doc. It also supersedes, in "The problem", "No page
generation" and "The consumer is blocked"; "The seam exists but is unused" still describes
`run_flow` (item 3).

- The wire setter: `crates/paged-wire/src/lib.rs:1493-1507` (`SetFlowGrowRule { story_id, grow,
  max_pages, copy_frame_options }`), `crates/paged-canvas/src/channel.rs:503-510` (protocol 64),
  `crates/paged-canvas/src/model.rs:3780-3790` (lowered to the operation),
  `crates/paged-mutate/src/apply/flow.rs:24-56` (the inverse is the prior rule).
- The canvas model carries the counts between builds:
  `crates/paged-canvas/src/model.rs:1269-1272`, `:8548`.
- An IDML export writes generated pages as real pages:
  `crates/paged-canvas/src/model.rs:4436-4443`; test
  `crates/paged-renderer/tests/reflow_pipeline.rs:223`.
- Scripts: `paged.setFlowGrowRule` (`crates/paged-script/src/lib.rs:3130-3156`), listed in the
  catalog (`crates/paged-introspect/catalog.json:634`, `:1366`).
- The consumer sends the rule when it opens a document:
  `plugin-doc: packages/doc-bundle/src/open.ts:73-74` (plugin-doc at `76e1d06`).

**2. The rule has two fields, not four choices.** This supersedes, in decision 1, "which
master (template) a new page is cloned from, which region on it continues the flow, where
pages are inserted (end of the chain's section), and whether empty generated pages are
removed (default yes)". `FlowGrowRule` holds `max_pages` and `copy_frame_options`
(`crates/paged-model/src/lib.rs:5929-5943`). Master, frame geometry, insertion point and the
removal of empty pages are fixed behaviour, taken from the InDesign measurement in the
addendum (`crates/paged-scene/src/grow.rs:17-29`).

**3. Growth runs on the scene document and in the renderer's build, not in the composition
model.** This supersedes, in decision 4, "It lands on the composition model, not the IDML
scene" and "Growth is a composition-level step around `run_flow`".

- `crates/paged-scene/src/grow.rs:88` — `Document::with_generated_pages` returns a copy of the
  document with the pages added; `:49-72` — the id functions.
- `crates/paged-renderer/src/pipeline/mod.rs:1189-1303` — `build_document` loops over
  `build_document_fixed`, reading the `OversetTextDropped` diagnostic of each pass.
- `crates/paged-composition/src` and `crates/paged-flow/src` contain no growth code, and
  `run_flow` is called only from tests.

**4. The loop grows by estimate first.** This supersedes, in the addendum, "it doubles a
story's count while it oversets". A story that oversets grows by the number of frames its
dropped lines need, and doubles only when a pass gives no estimate or after four estimated
passes (`crates/paged-renderer/src/pipeline/mod.rs:1262-1270`, `:1305-1315`).

**5. The loop is capped; no diagnostic names the cap.** Consequences asks for a loop "capped,
with a diagnostic at the cap". The caps are `MAX_GROW_PASSES = 24` build passes and, per story,
the rule's `max_pages` or `DEFAULT_MAX_GENERATED_PAGES = 2000`
(`crates/paged-renderer/src/pipeline/mod.rs:1182-1187`, `:1296`). No diagnostic code refers to
growth (`crates/paged-renderer/src/diagnostics.rs`); the build returned at a cap is the last
pass, with whatever overset diagnostic that pass reported.

**6. Decisions 5 and 6, as built.** No property path was added for the rule; the catalog's
settable paths contain none, and the rule is set only through the operation. In
`crates/paged-cli/src` no command names the rule. The viewer SDK builds through the same
`pipeline::build_document` (`crates/paged-sdk/src/build.rs:37`).

**7. The cache key still uses the absolute page index.** Consequences says of the body-story
emit signature "Fixed in 027". The page index is still hashed into it
(`crates/paged-renderer/src/pipeline/deltas.rs:84-91`); see the amendment to
[ADR 027](027-incremental-flow-invalidation.md).

**8. The timing in option A.** The "~14 s in wasm" predates the measurements in the
[implementation plan of ADR 027](../design/incremental-flow-plan.md). Its §0 gives 149 ms
native and 178 ms in wasm for one typed character on that document, measured at `6dea692`
before the steps of that plan.
