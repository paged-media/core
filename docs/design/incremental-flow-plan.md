# ADR 027 implementation plan: measured first

- **Status:** PLAN (2026-10-01). Measurements and an ordered plan for
  [ADR 027](../adr/027-incremental-flow-invalidation.md) (incremental flow invalidation).
- **Measured on:** commit `6dea692` (wire v64). The timing code behind §1 and §2 was never
  committed; §6 names the harness that is.
- **Reads with:** [026](../adr/026-auto-growing-region-chains.md) (growth),
  [028](../adr/028-pagination-rules-are-engine-owned.md) (keeps, break-before, span/split),
  ADR 029 (an unpublished proposal on opening DOCX files; its acceptance metric),
  ADR 030 (an unpublished proposal on the wasm build; the font-hash fix that came before this).

*Status note (2026-10-02): the plan was carried out. Steps 0 to 7 are on `main`; §6 lists each
step with its commit, its measured result and what is still not done. §0 to §5 are kept as
written on 2026-10-01 and describe the code at `6dea692`.*

## 0. Headline

*Status note (2026-10-02): this section predates the implementation; see §6 for what was built
and what a keystroke cost after it, and [ADR 027](../adr/027-incremental-flow-invalidation.md)
for the decision.*

One typed character costs a **full rebuild**, and the rebuild grows with the document:

| Document | Pages | Native, per keystroke | wasm (node, `-Oz`), per keystroke |
|---|---|---|---|
| `reflow` (ADR 026 fixture, growing) | 3 | 1.2 ms | 2.6 ms |
| `docx-pagination` (ADR 029 fixture, growing) | 5 | 2.4 ms | 3.7 ms |
| Long DOCX-shaped story (growing, keepNext) | 232 | **170 ms** | **235 ms** |
| The same, a 60-paragraph paste that adds a page | 232 → 233 | 586 ms (pasted at the end) / 1 493 ms (pasted on page 3) | 1 881 ms (pasted on page 3) |
| The 134-page annual (`after-312-appendix-b.paged`, real fonts, CMYK profile) | 134 | **149 ms** | **178 ms** |

The editor's own budget is 32 ms per rebuild (AC-E-1, quoted in the `LayoutCacheStats`
doc comment). The two large documents are 5–7× over it in wasm.

**Line breaking is not the cost.** The paragraph layout cache hits 4 278 of 4 280 lookups per
keystroke on the long story, and the misses cost 0.1 ms. The cost is re-emitting every
unchanged paragraph and the passes that walk every page. In the order of the plan below:

1. Every rebuild **re-materialises the generated pages** (`Document::with_generated_pages`).
   That is 49 ms of the long story's 170 ms, and 950 ms at 2 000 pages, because it rebuilds
   every index once per generated page. That is O(pages × (spreads + paragraphs)).
2. The edited story **re-emits from its first paragraph**, and does it **twice** whenever it
   holds a keep option, because the ADR 028 keeps fixpoint restarts from no forced breaks on
   every build. That is 48 + 43 ms on the long story.
3. On the annual the per-story emit cache **never hits**. A `ContinueNumbersAcrossStories`
   list turns it off for the whole document, the post-layout pass (running headers) runs a
   second full build with every cache off, and 134 stories are uncacheable because they emit a
   diagnostic. The edited story itself costs 0.2 ms of the 84 ms spent emitting stories.
4. **Fixed per-build passes**: the CMYK ICC transform is rebuilt on every inner pass (about
   38 ms per keystroke on the annual), the footnote pools are measured over every page once
   per story (16 ms), and the text z-slot and transparency-group passes walk every page
   (10–15 ms).
5. **Two cliffs** sit on top. The layout cache clears **all** of its entries at 10 000. The
   long story holds about 8 250, so a paste tips it over and pays 4 125 misses (886 ms). And
   a growing story caps at **257 frames**, so a story longer than that oversets, and the grow
   loop pads the document to 2 000 pages, 1 744 of them empty.

A simulation of ADR 027's per-story invalidation on the annual cuts the edit from 159 to
124 ms with **0 of 134 page digests different** from a full rebuild. Also caching the
post-layout pass "naively", without a read set, drops it to 90 ms but leaves **98 pages wrong**.
That second number is why decision 1 (record what emission reads) and decision 4 (the digest
gate) come first.

## 1. Method

- **Harness.** A throwaway `paged-canvas` example drove `CanvasModel` exactly as the worker
  does: `CanvasModel::load` with the whole corpus font directory (`corpus/fonts`) registered
  (`font_registry_from_paths`) and Inter as the default face, then
  `apply_mutation(InsertText)` of one character at an advancing caret ("typing"). Each edit is
  a new string, so the edited paragraph is a real cache miss. Every number is a median over 10
  edits after one warm-up edit, unless stated otherwise. Grow rules were set through the
  wire op `SetFlowGrowRule` (v64), with `copy_frame_options` for the DOCX shapes.
- **Documents.**
  - (a) `reflow` and `docx-pagination` from `paged-gen`, with ADR 026/029's grow rules.
  - (b) A long story: `docx-pagination` with 2 000 Word-like paragraphs (3 sentences, about
    5 lines each) inserted into section 1. It grows to 232 Letter pages. Its paragraph P054
    carries `keepNext`, as in the fixture.
  - (c) The finished annual from the editor's showcase chain
    (`editor: apps/canvas/showcase/checkpoints.build6/after-312-appendix-b.paged`): 134 pages,
    1 061 stories, the driver's 13 corpus faces, and `default_cmyk.icc` as the CMYK profile.
- **Phases.** `std::time::Instant` laps at the section boundaries of `pipeline::build_document`
  (grow loop), `build_document_fixed` (the post-layout re-run), `build_document_inner` (setup,
  auto-size, page walk, master overlay, frame pass, wrap collection, master stories, text on
  path, the body-story loop split into planning, cache lookup and splice, emit pass 0, keep and
  footnote re-emit passes, rollback and capture, then the post passes) and inside
  `emit_paragraph_into_chain`. Counters inside `paged_text::cache::layout_runs_cached`
  separated key hashing, miss layout and hit lookup.
  - A per-phase median is taken per phase, so the rows do not sum exactly to the wall time.
  - macOS `sample` agreed with the laps on `rebuild_indexes` (24 % of samples against 29 % by
    lap). Inside paragraph emission it is not usable: `<deduplicated_symbol>` and inlining put
    13 % on `layout_runs`, while the counters measured 0.1 ms per edit. Only the lap numbers
    are quoted below.
- **wasm.** `paged-canvas-wasm` was built from clean `origin/main` sources (no timing code)
  for `wasm32-unknown-unknown` with the release profile, `wasm-bindgen --target nodejs`, then
  `wasm-opt -Oz` (binaryen 126), as `.github/workflows/publish-wasm.yml` does. The build had no `gpu` feature,
  since node has no WebGPU, and rebuild does not touch the GPU.
  - Driven in node 24 through `handleMessage` (`registerFont` for the driver's 13 faces,
    `setFlowGrowRule`, `mutate insertText`) and `loadDocumentDirect`.
  - "wall" is the `handleMessage` round trip. "engine" is the worker's own
    `cacheStats.rebuildMs` (Date-based, so whole milliseconds).
  - There is no per-phase split in wasm: `Instant` is not available on `wasm32`. The phase
    shares below are native shares. On the two large documents the wasm totals are 1.2–1.4× the native ones (2.2× and 1.5× on the two small fixtures).
- **Not measured.** GPU repaint after the rebuild. ADR 030's addendum says it is now most of a
  write's end-to-end time in Chrome, and it is outside `apply_mutation`. Rasterisation is
  likewise not part of the edit path measured here.

## 2. Measurements

### 2.1 The long story (232 pages), one character typed on page 3, native

Wall 170.4 ms. Engine `build_ms` 164.3 ms. The other ~6 ms is op apply, `compute_story_pages`
and dropping the previous `BuiltDocument`.

| Phase | ms | Note |
|---|---|---|
| `with_generated_pages` | **49.4** | Clones the document, appends 230 pages, and calls `rebuild_indexes()` once per page. Each call walks every spread and every paragraph and lowercases style names (the heading-anchor table). |
| Emit pass 0 (both section stories) | **48.0** | The edited story re-emits all 2 120 paragraphs. |
| Keep re-emit pass | **43.2** | P054 `keepNext`: the fixpoint restarts from an empty `forced_breaks` and needs a second full emit on every build. |
| Text z-slot relocation and transparency-group fit | 13.7 | Every page, after the story loop. |
| Story capture (segments and cache delta copy) | 6.3 | Copies the whole story's commands into a cache entry that the next text edit throws away. |
| Rollback between passes | 2.5 | |
| Everything else in `build_document_inner` | < 1 | Frame pass, wrap collection, page walk, master overlay, auto-size. |

Inside paragraph emission (4 280 calls per edit = 2 140 paragraphs × 2 passes, 89 ms in all,
about 21 µs a paragraph):

| Part | ms | Share |
|---|---|---|
| Attribute cascade, font bytes, runs, substitutions | 10.7 | 12 % |
| Per-line wrap widths (`build_perline_wrap_widths`) | 7.9 | 9 % |
| Layout cache: key hashing 4.0, hit lookup and clone 4.2, misses 0.1 | 8.8 | 10 % |
| Line placement, glyph emission, decorations (the rest) | ~62 | 69 % |

A typed character at the **end** of the story costs the same as one on page 3 (168 against
170 ms): there is no early stop.

**No-op rebuild** (nothing changed, all caches warm): 79.8 ms. 48.0 of that is
`with_generated_pages`, 10.3 is splicing the two cached story deltas back in, and 15.5 is the
post passes. This is the floor of today's structure, before any story is re-emitted.

### 2.2 The annual (134 pages, 1 061 stories), one character typed on page 3, native

Wall 149.0 ms. The edited story `Story/u38` costs **0.20 ms** of the 83.5 ms spent emitting
stories. Everything else is re-emitting stories the edit did not touch.

| Phase | ms | Note |
|---|---|---|
| First inner build | 73.2 | |
| Second inner build (post-layout: running headers, page references) | **69.8** | `build_document_fixed` re-runs the whole build with the master-text and body-story caches **off** (`crates/paged-renderer/src/pipeline/build_engine.rs`: `if post.is_some() { None }`). |
| of which: emit (1 029 stories × 2 builds) | 56.1 | **Zero cache hits.** The annual declares a `ContinueNumbersAcrossStories` list, so `cross_story_numbering.is_some()` disables the cache document-wide. |
| of which: build setup | **38.3** (2 passes) | The CMYK ICC transform is built in every `build_document_inner`. Without the profile the edit is 103.8 ms. |
| of which: footnote measure and keep check | 16.0 | The lap covers the keep check and `measure_footnote_pools`. The code measures the pools over **all** pages for every story once any page holds a footnote: O(stories × pages). |
| of which: z-slot and group-fit passes | 9.9 | |
| of which: span/split planning per story | 6.5 | `plan_span_columns` resolves every paragraph's attributes in every story, every build, to find out that most stories have no spans. |
| of which: master stories, auto-size, post-layout context | 5.1 + 2.7 + 2.3 | |

### 2.3 Spikes and cliffs found on the way

- **Growth doubles on a hinted rebuild.** A paste that adds one page to the 232-page story
  runs three full builds:
  - the hinted count, which oversets;
  - double the count (about 460 pages, which fits);
  - the trimmed count.

  That costs 586 ms native when pasted at the end. With the paste on page 3, 1 493 ms native
  and 1 881 ms in wasm (wasm also pasted on page 3).
- **The layout cache clears everything at 10 000 entries** (`LayoutCache::insert`, "no LRU
  yet"). The long story holds about 4 entries per paragraph (8 245 for 2 120 paragraphs) and
  adds about 2 per keystroke. The page-3 paste tipped it over mid-build: 4 125 misses, 886 ms
  of re-layout, and `len` back at 2 454. Typing alone reaches the cliff after about 900
  keystrokes.
- **A growing story caps at 257 frames.** `paged_scene`'s `MAX_FRAME_CHAIN = 256` is a cycle
  guard on `NextTextFrame`, and it truncates a generated chain. Verified:
  `with_generated_pages({s0: 2000}).frame_chain(s0).len() == 257`.
  - With 2 400 paragraphs (about 280 pages of text) the story oversets after its 257th page.
  - The grow loop then keeps doubling to `DEFAULT_MAX_GENERATED_PAGES`, producing 2 003 pages,
    1 744 of them empty.
  - Every keystroke then costs 1.09 s native, 950 ms of it in `with_generated_pages`.
  - This is a correctness bug for any DOCX over roughly 250 pages, independent of ADR 027.
- **The digest itself is not free.** `DisplayList::digest()` costs 1.3–2.3 ms a page on the
  large documents (306 ms for the annual's 134 pages, 305 ms for 233 DOCX pages). That is acceptable for a CI
  lane, not for a hot path.

### 2.4 Simulating decision 1 on the annual, with a digest diff

Two env-gated hacks were run (not committed):
- drop only the edited story's body-story entries instead of `clear()`;
- allow the cache despite cross-story numbering.

Each edit was applied to two models loaded from the same bytes: one with the hacks, one
without. Then all 134 page digests were compared.

| Variant | Per keystroke | Pages whose digest differs from the full rebuild |
|---|---|---|
| Today (the reference model in the same run) | 159 ms | — |
| Per-story invalidation, cache on despite cross-story numbering | **124 ms** (−22 %) | **0** on every edit |
| … and the cache also on in the post-layout pass | 90 ms (−43 %) | **98** on every edit |

What the simulation shows:
- Even with invalidation, 135 stories per build still re-emit. 134 of them are uncacheable
  because they produced a diagnostic, and the cache stores no diagnostics.
- The 0 holds for this edit and this document only. Cross-story numbering is correct here only
  because the edited story is not in a continued list. The real key must carry the ledger
  state (see step 5).
- The third row is the stale-key failure ADR 027 describes. Post-layout variables are not in
  the hand key, so cached stories keep pre-header text. A digest gate catches it on the first
  edit.

## 3. Where ADR 027 cuts, against the code as it is now

*Status note (2026-10-02): "the code as it is now" is `6dea692`. This section predates the
implementation; see §6.*

| ADR 027 decision | Fit with `6dea692` | What changes |
|---|---|---|
| 1. Record a read set, not a hand key | Fits, and is needed more than the ADR said. The hand key (`crates/paged-renderer/src/pipeline/deltas.rs::body_story_signature`) covers chain ids and bounds, the wrap shapes on chain pages, and the absolute page index. It misses content, styles, variables, the cross-story numbering ledger and frame options (insets, columns, first-baseline offset). Four `clear()` sites paper over it: `apply_mutation` text lane, `apply_operation`, `undo`, `redo`. | Revisions per story and per definition from `paged_mutate::apply`. Emission records what it resolves. |
| 2. Identity, not absolute index | Generated pages already have stable ids (`<story>_grow<k>_page`, ADR 026 §3). Both emit caches still key on the absolute index: `master_text_emit_cache` is `(frame, page_idx)`, the body key hashes `page`, and deltas are `per_page: Vec<(usize, …)>`. | Key and splice by `PageId`. Page numbers become variables. |
| 3. Early stop inside a story | Not possible yet. A story emits in one `for paragraph in &story.paragraphs` loop with emitter state (`frame_idx`, `y_cursor`, numbering, footnote captures, `forced_breaks`) that is never snapshotted per frame. The keeps fixpoint and the footnote fixpoint both re-emit the **whole** story. | Per-frame resume points: paragraph index, line, emitter state, per-frame command ranges (already tracked as `frame_cmd_ranges`). |
| 4. Digest gate | `DisplayList::digest()` exists and is used by `paged-sdk`'s equivalence test and three `paged-canvas` tests. No incremental-vs-full lane exists. | New lane (step 0). |
| 5. One commit path | Confirmed: four clear-then-rebuild copies, and two near-identical `PipelineOptions` constructions (`build_for_export`, `rebuild_after_mutation`). | `commit_and_rebuild(invalidation)`, `pipeline_options(purpose)`. |
| 6. Structural ops rebuild everything | Fits. | — |
| 7. Dirty pages drive redraw | Today a text edit reports **every** page: `MutationOutcome.page_ids` is all pages, and the wasm `CacheEffect::InvalidatePages` uses all pages of the story's chain, which for a DOCX is the whole document. | Report the pages whose emission changed. |

**Every global pass a local edit pays for today.** These are measured costs per keystroke,
native:

| # | Pass | Long story | Annual |
|---|---|---|---|
| G1 | `with_generated_pages`: clone + `rebuild_indexes()` per generated page | 49 ms (950 ms at 2 000 pages) | — |
| G2 | Edited story re-emitted from paragraph 0, no early stop | 48 ms | 0.2 ms |
| G3 | Keeps fixpoint restarts from empty `forced_breaks` on every build | 43 ms | ~0 |
| G4 | Unedited stories re-emitted: cache off under cross-story numbering, uncacheable when they emit a diagnostic | — | ~80 ms (both builds) |
| G5 | Post-layout second build with every cache off | — | 70 ms |
| G6 | CMYK ICC transform rebuilt per inner build | — | 38 ms |
| G7 | Footnote-pool measure over all pages, once per story | — | 16 ms |
| G8 | Z-slot relocation and transparency-group fit over all pages | 14 ms | 10 ms |
| G9 | Span/split planning resolves every paragraph of every story | 0.7 ms | 6.5 ms |
| G10 | Whole-story delta capture, and splice of cached stories (O(commands)) | 6 + 10 ms | 2 ms |
| G11 | Grow loop doubles on overset, then trims (two extra full builds) | +2 builds on a page-adding edit | — |
| G12 | Layout cache wholesale clear at 10 000 entries | 886 ms spike | — |

Also global, but not a cost in these documents:
- `expand_column_chain` measures the whole story for balanced columns.
- `auto_size::fit_all` measures every auto-sized frame.
- TOC stories rebuild from `page_labels` on every build.
- `build_post_layout_ctx` walks every line.

## 4. The plan

*Status note (2026-10-02): this section predates the implementation; §6 records, per step, what
was built and where it departs from the plan.*

Each step lands on its own, keeps output byte-identical, and is tested by the digest gate from
step 0. The "expected win" figures are measured components from §2, not forecasts of
interaction effects.

### Step 0: the 027 digest gate, plus two correctness fixes

- **Gate.** Add `PAGED_DIGEST_GATE=1` (env or a `CanvasOptions` debug flag).
  - After every committed op the model also runs a cold build: fresh caches, no `grow_hint`,
    no seeded fixpoints.
  - It compares page count, page ids and `DisplayList::digest()` per page, and also
    `story_layout` and diagnostics, which the digest does not cover.
  - A difference fails with the first differing page and the op that caused it.
  - Wire it into the editor E2E suite and the showcase/annual driver as ADR 027 §4 says, plus
    a native nextest lane that replays a typing script over `reflow`, `docx-pagination`,
    `keeps`, `span-columns`, `footnotes`, the long story and a numbered-list fixture.
  - Cost: two builds plus 1.3–2.3 ms a page per op, in the lane only.
- **Fix the 257-frame cap for generated chains** (G1's 2 000-page case). Generated frames are
  threaded by construction, so `frame_chain` can follow them without the cycle guard, or the
  guard can count authored links only. Oracle: a 300-page story fits, with no overset and no
  empty generated page.
- **Fix `with_generated_pages` to index once.**
  - Append all spreads, then call `rebuild_indexes()` once, maintaining `text_frame_index`
    incrementally for the thread links.
  - Move the heading-anchor table out of the per-page path. It depends only on stories.
  - Expected: most of G1's 49 ms (the sample put 96 % of `with_generated_pages` in
    `rebuild_indexes`) and nearly all of the 950 ms at 2 000 pages.
  - Oracle: identical digests (pure refactor).

### Step 1: one commit path (ADR 027 §5)

- Fold the four clear-then-rebuild copies into `commit_and_rebuild(Invalidation)` and the two
  option builders into `pipeline_options(Purpose)`.
- No behaviour change. It is the single place every later step edits.
- Oracle: digest gate green, existing `paged-canvas` tests.

### Step 2: the cheap per-build constants (G6, G7, G9, G11, G12)

- **Cache the ICC transform** on the model, keyed by profile bytes, intent and BPC. Pass it in
  through `PipelineOptions` as `FontTable` already is. Expected: 38 ms of the annual's 149
  (−26 %), native; on wasm32 `paged-color` takes a different path, so measure there.
- **Footnote measure only the chain's own pages**, and only when this story captured a
  footnote. `frame_host_keys` already names them. Expected: about 16 ms on the annual.
- **Span/split planning**: decide "no span or split here" from the resolved paragraph styles
  once per build, not per paragraph per story. Expected: about 6 ms on the annual.
- **Grow by estimate, not by doubling.** After an overset, grow by
  `ceil(overset lines / lines per generated frame) + 1`, which the previous build knows.
  Doubling stays the fallback when no estimate exists. Expected: a page-adding edit takes
  1–2 builds of about 180 ms instead of 3 (measured 586 ms native at the end of the long
  story).
- **Bound the layout cache honestly.** Evict entries not touched in the last N builds
  (a generation counter), or LRU, instead of clearing everything. Expected: removes the
  886 ms (native) spike. Typing never hits the cliff.
- Oracle for all of these: the digest gate. Grow-by-estimate must land on the same page set
  as doubling-then-trimming, and the gate compares page ids.

### Step 3: seed the keeps fixpoint from the previous build (G3)

- Keep each story's last `forced_breaks` on the model, as `grow_hint` is kept, and start the
  next build from it. If the seed still satisfies every keep, the story needs one emit pass
  instead of two.
- Expected: 43 ms of the long story's 170 (−25 %).
- Risk: a seeded fixpoint could settle on a different stable set than the cold one, since
  keeps only add or move breaks earlier.
  - Mitigation: after a seeded pass, drop any seeded break the keep rules no longer ask for,
    and re-emit.
  - The gate's cold build is the oracle.

### Step 4: revisions and the read-set key (ADR 027 §1–2) (G4)

- **Revisions.**
  - `paged_mutate::apply` bumps a content revision per story it touches.
  - Styles, swatches, colour settings and fonts get definition revisions.
  - Text ops in `crate::mutate::apply` bump the story revision too.
- **Read set.** Emission records:
  - the story revision;
  - the ids of the paragraph and character styles, swatches and fonts it resolved;
  - the chain's frame ids **and their text-frame options** (insets, columns, first-baseline
    offset, vertical justification, auto-size fit);
  - the wrap shapes on chain pages;
  - the variables it read (page number, page count, chapter number, running headers, text
    variables);
  - for a story in a continued list, the numbering ledger value **in**.

  The key is the hash of that set. The entry also stores the ledger value **out**, so a hit
  can replay the ledger update.
- **Key by `PageId`**, and splice by page id, for both emit caches.
- **Store diagnostics in the delta**, so the 134 stories that emit a diagnostic (overset,
  substitution) become cacheable and replay their diagnostics.
- Then the text lane stops calling `clear()`. A text edit invalidates exactly the edited story
  by revision.
- Expected: the annual's measured simulation, 159 → 124 ms (−22 %).
- Oracle: the gate over the annual chapter scripts, plus a numbered-list fixture whose
  continued list spans the edited story.

### Step 5: the post-layout pass reuses what it can (G5)

- With variables in the read set, the second build is a cache **lookup** like the first. Only
  stories whose recorded variables changed value re-emit: running headers, page references,
  and anything reading a page number that moved.
- Expected: toward the simulation's 90 ms on the annual. That simulation was wrong on 98 pages
  only because the key had no variables, so it is not a promise.
- Also skip the second build when the first build's post-layout context equals the previous
  build's.
- Oracle: the gate, on the annual and the `variables` and `navigation` fixtures.

### Step 6: early stop inside a story (ADR 027 §3) (G2, G10)

- **Resume points.** For each frame of a story's last build, record:
  - the start position and end position (paragraph, line, and byte offset into the story);
  - the emitter state at the frame start: numbering counters, footnote reservation, forced
    breaks, span-plan region;
  - the frame's own command range, per page (from `frame_cmd_ranges`, plus `story_layout` and
    the footnote captures).
- **Resume.** An edit at byte offset `o` re-lays from the frame that contains `o`, with the
  emitter restored to that frame's start state.
- **Stop.** After each re-laid frame, compare its end position and emitter state, after the
  keep rules (ADR 028 §4 limits keeps to a one-region lookahead), with the previous build.
  When a frame starts **and** ends where it did, splice every later frame of the chain from
  the previous build and stop.
- **Per-frame deltas.** Today's whole-story delta becomes per-frame deltas, so the splice
  copies only the frames that are reused, and pages whose content did not change are not
  touched at all (step 7).
- Expected on the long story: emit drops from about 48 ms (one pass after step 3) to about
  0.4 ms a re-laid page × 1–2 pages. The 10 ms splice of the rest becomes zero once clean
  pages are kept (step 7). After steps 0–6 the long-story keystroke is bounded by the post
  passes and the remaining per-page bookkeeping, about 20–30 ms native from the no-op floor
  in §2.1 minus G1.
- Risks: see §5. The footnote reservation and keep breaks must be part of "ends where it
  did".

### Step 7: dirty pages drive everything after layout (ADR 027 §7) (G8)

- Keep the previous build's `BuiltPage` display lists, and rebuild only the pages that the
  re-laid frames or the invalidated stories touch.
- Run the z-slot relocation and the transparency-group fit on dirty pages only.
- Report those pages in `MutationOutcome.page_ids` and `CacheEffect::InvalidatePages`, so the
  GPU re-encodes what changed. Visible pages first.
- Expected: removes G8 (14 ms on the long story, 10 ms on the annual) and G10's splice
  (10 ms). It also stops the GPU re-encoding all 232 pages per keystroke, a cost not measured
  here.
- Oracle: the gate compares every page, including the ones not rebuilt, against the cold
  build.

### Acceptance (ADR 029 decision 6, and this plan)

1. **"Typing on page 3 re-lays only the pages that change".** Expose two counters per commit:
   the frames re-emitted and the pages rebuilt. On the 232-page story with grow rules, typing
   one character on page 3 re-emits ≤ 2 frames and rebuilds ≤ 2 pages, unless the edit moves
   a line across a page boundary. Then the count is the pages up to the first frame that
   starts and ends where it did.
2. **"With the 027 digest gate green".** The gate lane (step 0) passes over the typing scripts
   and the showcase/annual chapter drivers: no page differs from a cold build after any op.
3. **"Edit latency in wasm measured"**, with this document's numbers as the baseline. Record
   in ADR 029:
   - the per-keystroke wasm time (node harness, `-Oz`) on the long story: today **235 ms**;
   - the page-adding paste: today **1 881 ms**;
   - the annual keystroke: today **178 ms**;
   - plus `editor: apps/canvas/tests/showcase/rebuild-profile.spec.ts` (Chrome) for the
     end-to-end figure including repaint.

   The target is the editor's own 32 ms rebuild budget (AC-E-1) for a keystroke on both large
   documents.

## 5. Risks

- **Keeps.**
  - `crates/paged-renderer/src/pipeline/keeps.rs` turns rules into forced breaks from the **previous pass's** line placement, and
    evaluates a paragraph only at its first break (ADR 028 record). A seeded fixpoint (step 3)
    and an early stop (step 6) both depend on that being deterministic from the frame-start
    state.
  - Keep-with-next on the last paragraph of a frame looks one region ahead, so the stop
    condition must compare the frame *after* keeps are applied, not raw.
- **Growth.**
  - The grow loop and early stop must both converge: the loop repeats until the page set
    **and** the per-frame break positions stop changing (ADR 027 consequences).
  - Generated ids are stable, but `master_text_emit_cache` and the body key still use absolute
    indices until step 4, so step 6 must not land first.
  - Removing trailing empty generated pages must invalidate nothing before them.
- **Span/split and balanced columns.**
  - `plan_span_columns` measures the whole story at each spanned width.
  - `expand_column_chain` measures the whole story for `VerticalBalanceColumns`.
  - Both make a frame's regions depend on text after the edit. A split or balanced block is a
    unit: early stop may resume only at a frame that starts outside one, and must re-plan the
    block that contains the edit.
- **Footnotes.**
  - The reservation fixpoint is per frame (`reserved_64`). A footnote moving between frames
    changes both frames' text areas.
  - The stop condition must include each frame's reservation, and the footnote pool
    post-pass must run on dirty pages only (step 7).
  - Cross-frame splitting of an oversized footnote is still deferred (ADR 028). The gate will
    expose any interaction.
- **Cross-story dependencies.**
  - Text wrap is collected from frame geometry before any story is laid out
    (`collect_wrap_rects_per_page`), and auto-size fits feed those rects. A text edit in an
    auto-sized frame moves its bounds and so its neighbours' wrap: the fitted bounds must be
    in the neighbours' read set.
  - Anchored objects with wrap and floating Word drawings (ADR 029 open item) would make one
    story's layout an input to another's. They need their own decision before step 6 covers
    them.
- **Numbering.** A continued list makes every later story in the list depend on the ledger
  value in. Step 4 keys on it. A renumbering edit then re-emits the later stories in the
  list, which is correct.
- **Post-layout values.** Running headers, page-number cross-references, TOC page numbers and
  page count are outputs of layout that are read as inputs. Until they are recorded variables
  (step 5), any cache in the post-layout pass is wrong. §2.4 measured that: 98 pages.
- **Memory.** Per-frame resume points and per-frame deltas add one emitter-state snapshot per
  frame (small) and keep the previous build's display lists alive (one build's worth, about
  what is held today across a rebuild).
- **Gate cost.** Two builds plus 1.3–2.3 ms a page per op. It stays a debug/CI lane, as
  ADR 027 says. The digest's `Debug` formatting is its cost; a binary hash would be cheaper if
  the lane gets too slow.

## 6. Progress

Measured with the committed harness: `cargo run --release -p paged-canvas --example
edit_latency -- long|annual|docx|reflow|paste` (`crates/paged-canvas/examples/`), the
same method as §1, and a node driver of `paged-canvas-wasm` (no `gpu`, `wasm-bindgen
--target nodejs`, `wasm-opt -Oz`, binaryen 126, `handleMessage` with the driver's 13 faces).
The long story is `docx-pagination` + 2 000 paragraphs of 6 lines: **227** Letter pages
(§1's was 232). The machine was shared with other builds during every run (load average
20–70), so numbers carry roughly ±10 % noise; each row compares binaries run back to back.

Correctness net for every row: `crates/paged-canvas/tests/digest_gate.rs` (step 0) gates
every scripted op on reflow, docx-pagination, keeps, span-columns, footnotes, numbering,
variables, navigation and a growing DOCX-shaped story, and the annual with
`PAGED_DIGEST_GATE_ANNUAL=<.paged>`.

| Step | Commit | Long story, native | Annual, native | wasm (node) |
|---|---|---|---|---|
| baseline (`3b9694e` + the cycle-guard fix) | — | 194 ms | 154 ms (typing in `u020c4a`) | — |
| 0: digest gate; generated pages index once | `1be3e7f` | **146 ms** | 151 ms | — |
| 1: one commit path (`commit_and_rebuild`) | `9810367` | unchanged | unchanged | long 244 ms, annual 176 ms |
| 3: keeps fixpoint seeded from the last build | `493da64` | **90 ms** | unchanged | long **167 ms**, annual 159 ms |
| (step 2 constants, see the table further down: CMYK cache, footnote pages, span early-out) | `770bd88`…`c401ded` | — | — | — |
| 3 again, typing in **Story/u38** (the §2.2 edit) | `493da64` + step 2 | 90 ms | 110 ms | long 155 ms, annual 154 ms |
| 4: a text edit drops only its own story | `f81dfb4` | 90 ms | **70 ms** | — |
| 5: post-layout pass uses the caches | `0879078` | 90 ms | **41–44 ms** | long 155 ms, annual **59 ms** |
| 6: resume at the edit, stop where the flow rejoins | `0f6c008` | **~56 ms**, 1 frame laid out | unchanged (u38's frame is not recorded) | — |
| 6a: layout cache position-independent (the finding in the step 2 notes) | `aa29214` | unchanged; **page-3 paste 1 381 → 454 ms** | unchanged | — |
| 7: only changed pages reported, re-encoded and finished | `0fd3b67` | **19.4 ms**, 1 of 227 pages changed | **38 ms**, 3 of 134 pages changed | long **26 ms**, annual **53 ms**, page-3 paste **592 ms** |

Notes per step:
- **Step 0.** The gate compares page ids, every page's digest, `story_layout` and the
  diagnostics against a cold build (fresh layout cache, no emit caches, no grow hint).
  `PAGED_DIGEST_GATE=1` runs it inside every rebuild. It was checked to bite: leaving the
  body-story cache uncleared on a text edit fails keeps and docx-pagination on the first
  keystroke. `with_generated_pages` no longer re-indexes per page (−48 ms on the long story).
  Wired: the native nextest lane. **Not wired** (other repos): the editor E2E suite and the
  showcase driver; both can set `PAGED_DIGEST_GATE=1` on a native build or call
  `digest_gate_check()`.
- **Step 3.** The ADR 028 fixpoint is path-dependent (each pass carries earlier breaks
  over), so a seed is reused only when every paragraph with an active keep option ends
  before the edited paragraph; the seeded pass must also hold. Typing after P054 on the long
  story costs one emit pass instead of two. A Word document with `keepNext` on headings
  AFTER the edit gets no seed. The gate catches a seed without that condition (docx
  paste).
- **Step 4.** Per-story invalidation needed one more input than §4 listed: the page's
  path buffer. A delta's path ids are relative to the page's pool, including references to
  glyphs an EARLIER emission on the page interned, so a re-emitted story shifted every later
  story's ids on its pages. `PathBuffer` now keeps each path's intern key and a running
  fingerprint; a delta is spliced only into a pool with the print it was captured from, and
  replays its paths under their keys. The gate catches a splice without the check
  (numbering, first keystroke). The ledger IN is in the key (a continued list no longer turns
  the cache off), diagnostics replay, TOCs are never cached. Not done: keying deltas by
  `PageId` (still absolute indices in the key, so a grown page set misses).
- **Step 5.** The post-layout pass keeps its own body entries (salted key) and keys
  context-printing stories on the running-header index; masters that print no page context
  reuse the first pass's delta. Found on the way: master deltas dropped their `story_layout`
  lines (also on gesture rebuilds before). The gate catches the naive variant (variables and
  the annual).
- **Step 6.** Granularity is a paragraph boundary, not a frame (`emit_paragraph` lays a
  paragraph out whole; a stop can land inside a frame). `crates/paged-renderer/src/pipeline/resume.rs` keeps a
  `ParaMark` per paragraph (frame, baseline cursor, leading, list counter, last placed
  frame, overset flags, the frame's deepest baseline and range so far, the page's cut and
  pool print); the model passes an `EditSpan` per text edit. Recorded only for stories whose
  emission is a function of that state (no VJ, balanced/spanned columns, tables, anchored
  frames, footnotes, page context, cross-story lists, multi-column regions; no post-pass that
  added commands). The annual's Story/u38 sits in a multi-column frame and is not covered.
  The gate catches a stop that ignores the baseline cursor (typing a word at a time until the
  paragraph wraps). A paste is not helped by the stop: its paragraphs move by lines, so no
  later mark matches; 6a is what fixed it.
- **Step 7.** `BuiltDocument::fresh_pages` marks pages laid out afresh (master text only when
  its output print differs from the previous build's). For a build whose commits were text
  edits of one own-chain story the model narrows `MutationOutcome.page_ids` (and the
  worker's `InvalidatePages`) to those pages plus new pages plus the story's auto-sizing
  frames' pages, and lends the previous pages: an unchanged page is moved in and the
  footnote, z-slot and group-fit passes skip it. The gate checks every op that each page NOT
  reported kept its digest. Not done: skipping the splices into pages that end up adopted
  (they are still rebuilt in memory, then swapped for the previous page); the annual's
  remaining 38 ms is mostly those splices across two passes.

### Acceptance (§4) on `0fd3b67`

1. **Typing on page 3 re-emits ≤ 2 frames and rebuilds ≤ 2 pages**: met on the 227-page
   story: 1 frame laid out, 1 page changed and finished, 226 adopted (asserted in
   `digest_gate.rs`). "Rebuilds" here means finished and re-encoded; every page is still
   reassembled from cached deltas before the adoption.
2. **Digest gate green on every scripted op**: yes, all 10 lanes including the annual, at
   every commit of steps 0–7, plus the unreported-page check from step 7.
3. **wasm latency** (node, `-Oz`, the 13 faces): long story **235 → 26 ms** per keystroke,
   page-3 paste **1 881 → 592 ms**, annual **178 → 53 ms**. Under the 32 ms budget (AC-E-1)
   for the long story only; the annual's Story/u38 re-emits whole (multi-column) and every
   other story is spliced twice (two passes). The editor's `rebuild-profile.spec.ts`
   (Chrome, with repaint) was not run.


### Step 2 and the two cliffs (G6, G7, G9, G11, G12)

Measured with a throwaway `paged-canvas` example (not committed) driving `CanvasModel` as
§1 does: release, native, corpus fonts registered, typing on page 3. Each row compares the
commit with its parent, back to back, two runs where noted. Digest evidence for every row:
per-page `DisplayList::digest()` over every `corpus/generated` fixture plus the reflow,
docx-pagination, 2 000-paragraph and annual load / 3 keystrokes / 60-paragraph page-3 paste
scripts, identical to the parent (1 612–1 615 page digests), and `digest_gate` green with
`PAGED_DIGEST_GATE_ANNUAL` set from item 4 on.

| Item | Commit | Before | After |
|---|---|---|---|
| 257-frame cap on generated chains (bug) | `a4c9cdb` | 2 600 paragraphs: 2 003 pages (1 709 padding), 1 149 ms per keystroke | 294 pages, 202 ms |
| Layout cache: LRU instead of clear-all at 10 000 (G12) | `4963b68` | after crossing: 2 656 entries kept, 3 185 / 4 048 misses in the crossing build | 7 614 kept, 3 143 / 3 268 misses; wall time not measurably different; wasm high-water +12.7 MiB (1 254.6 → 1 267.3 MiB, +1 %) |
| CMYK transform kept on the model (G6) | `770bd88` | annual 143.4 / 142.9 ms; wasm 164.5 / 166.1 ms | **105.2 / 104.1 ms** (−38 ms); wasm 152.9 / 148.8 ms (≈ −14 ms, qcms is cheaper) |
| Footnote measure on the chain's own pages (G7) | `3831aa7` | annual 109.5 / 112.8 ms | **96.1 / 96.1 ms** (−13 to −17 ms) |
| Span/split planning returns early (G9) | `c401ded` | planner 817 µs per inner build; annual 87.9 / 87.6 ms | 157 µs; 87.2 / 86.3 ms (≈ −1.3 ms; the plan's 6.5 ms was measured on `6dea692`) |
| Grow by estimate, not doubling (G11); lowered `max_pages` holds (bug) | `d1fc71a` | paste at the end adding 3 pages: 3 builds, 315 / 325 ms; page-3 paste 671 / 684 ms; open+grow 1 391 / 1 393 ms | 2 builds, 241 / 258 ms; 601 / 630 ms; 1 060 / 1 083 ms |

Notes:
- **The 10 000-entry cliff was mostly not the clear.** `paged_text::cache` folds the
  paragraph's absolute `first_baseline` into the layout key (`layout_runs_cached`), so every
  paragraph that moves vertically misses. A paste on page 3 re-lays every paragraph below it
  whether or not the cache crosses its bound (2 143 misses on the 227-page story without a
  crossing), and so does a keystroke that changes its paragraph's line count (a 700-key
  typing run had its worst keystroke, 794–799 ms, at the same key with or without the LRU).
  Keying the layout on a baseline-relative origin (and translating the result) is the
  lever for G12 and for the page-3 paste; it is not in this step.
- **Grow by estimate** sizes the next pass from the summed line HEIGHTS of the dropped
  lines over what a generated frame took, not line counts: pasted text at auto leading
  undershot a count estimate by a frame. Doubling stays the fallback (no measure, or after
  four estimated passes). A body-story cache hit replays the measure (`f81dfb4` made overset
  stories cacheable).
- **Lowered cap.** The `grow_hint` count is clamped to the rule's current `max_pages` (else
  `DEFAULT_MAX_GENERATED_PAGES`); a rule with an explicit `max_pages` above the default keeps
  it. Found while the editor's panels for these rules were built (editor `a2b3e91`).
