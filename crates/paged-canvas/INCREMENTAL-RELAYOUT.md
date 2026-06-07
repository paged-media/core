<!--
  Design spike for the renderer roadmap. Part of paged (https://paged.media),
  dual-licensed MPL-2.0 OR PMEL like the rest of this crate. This is a DESIGN
  DOCUMENT, not an implementation — W1.24 (audit B17–B19) is the perf lane;
  the incremental-relayout work it scopes is deliberately NOT built here.
-->

# Incremental relayout — design spike (W1.24, audit B17–B19)

**Status: paper only. No implementation in this change.** This is the
staged plan for replacing the full-document relayout that `CanvasModel`
runs after *every* mutation (`rebuild_after_mutation` → `build_document`
→ a fresh `BuiltDocument`). It records what the pipeline can invalidate
today, what would have to be cached/keyed to do better, the dependency
edges that make naïve per-story caching unsafe, and a staged path with
effort estimates.

## 5-line abstract

The canvas relays out the whole document on every keystroke; the W1.24
bench lane measures this (`build_document/text` ≈ 58 ms,
`build_document/tables` ≈ 490 ms on the dev host; the model-cached
`rebuild` round-trip ≈ 36 ms / 380 ms). Three emit-delta caches
(layout, master-text, body-story) already make the *common* gesture
cheap, but a text edit blows the body-story cache and pays full
re-emission. The path forward is **story-level invalidation with a
post-layout fixup pass**: key each story's emission on a content+geometry
signature, re-emit only dirtied stories, then re-run the cheap global
passes (numbering, running headers, cross-references) that depend on
final page positions. Estimated 3 staged increments, ~6–9 eng-days, with
the table-cell and threaded-overflow cases as the correctness long poles.

## 1. What the pipeline can invalidate today

`build_document` is monolithic — it walks spreads → pages → frames →
stories and emits one `DisplayList` per page from scratch. There is **no
invalidation granularity at the pipeline level**: it always rebuilds
everything. What *does* exist is three persistent caches threaded in via
`PipelineOptions`, owned by `CanvasModel` and reused across rebuilds:

| cache | crate / type | key | granularity | what it saves |
|-------|--------------|-----|-------------|---------------|
| **layout** | `paged_text::LayoutCache` | blake3 over paragraph content + style + measure inputs (`cache.rs`) | **paragraph** | re-running Knuth–Plass for unchanged paragraphs |
| **master-text** | `MasterTextEmitDelta` | `(master_frame_self_id, page_idx)` | **per-master-page** | re-emitting footers/running headers (~161 ms/multi-spread) |
| **body-story** | `BodyStoryEmissionDelta` | `(story_self_id, body_story_signature)` | **per-story** | the largest single cost — body-story emission (~613 ms ceiling) |

The `body_story_signature` (pipeline `mod.rs`) already hashes the frame
chain's identity + bounds + transforms and the wrap rects on chain pages.
So the *machinery* for story-level keying exists. The gap is that a
**text edit blows the body-story cache wholesale** (see `apply_mutation`
in `model.rs`: `self.body_story_emit_cache.borrow_mut().clear()`), because
the signature hashes geometry, not content — a content change wouldn't
bump it, so the only safe move today is a full clear. Incrementality =
making that clear *selective*.

Generation counters exist for the consumer side: each `BuiltPage` carries
`layout_generation` + `numbering_generation`, which the canvas combines
with the page id to invalidate its GPU scene cache. These are output
signals, not pipeline inputs — but they show the page is already the unit
the *downstream* cache keys on.

## 2. State that would need caching / keying

To re-emit only dirtied stories, the rebuild must persist (per story):

1. **A content signature** — hash of the story's `Content` runs + applied
   styles, NOT just geometry. This is the missing half of
   `body_story_signature`. Folding content into the signature lets the
   body-story cache survive an edit to a *different* story.
2. **The emitted `BodyStoryEmissionDelta` per page the chain touches** —
   already cached; today keyed by absolute page index (the spike must
   keep the page-index-in-key invariant noted in the existing comment, or
   an insert-page mid-chain splices into the wrong page).
3. **The story→pages map** (`story_pages` / `compute_story_pages`) —
   already maintained on the model after every rebuild. This is the dirty
   set: edit story S ⇒ re-emit S's pages, reuse the rest.
4. **Frame-chain fit state** — whether a story overflowed its declared
   chain (`dropped_overflow_lines`). An edit that grows a story can push
   lines onto a *later* frame on a *later* page that the pre-edit dirty
   set didn't include. The cache must record the chain's last frame's
   fill so "did this edit change which pages the chain occupies?" is
   answerable without re-emitting every story.

## 3. Dependency edges (why naïve per-story caching is unsafe)

These are the reasons "edit story S ⇒ re-emit only S" is wrong without a
fixup pass. Each is a real edge in this codebase:

- **Threading chains.** A story spans a frame chain across pages. Adding
  text reflows lines *forward* through the chain — a one-character insert
  on page 3 can push a line onto page 4. So the dirty page set is not
  static; it must be recomputed from the *post*-relayout fit. (`frame_chain`
  in `paged-scene`; overflow accounting in the pipeline.)
- **Text wrap / anchored objects.** A frame's `wrap_rects` shape line
  breaking for *other* stories whose text flows past it (already in
  `body_story_signature`). Moving or resizing an anchored object reflows
  the host story AND any story it wraps. Anchored-object positions
  (`AnchorPosition`, `resolve.rs`) are resolved *after* layout, so they
  are a post-pass input, not a pre-pass one.
- **Running headers + page numbering.** `resolve()` computes the
  `NumberingMap` and `running_headers` from the *built* document — they
  depend on final page positions. Re-emitting one story can change a
  page's first/last heading, which changes a running header two pages
  later. These must re-run globally (they are cheap — they walk the built
  tree, they don't re-lay-out).
- **Cross-references / TOC / "continued on" jump lines.** `field_diff`
  (`FieldChange`) and `TocEntry` resolve page numbers post-layout. A
  reflow that moves an anchor's page invalidates every xref/TOC entry
  pointing at it. Same treatment as numbering: cheap global fixup pass.
- **Table cells.** A table is a story; a cell is a sub-story. Cell text
  edits (W1.13) reflow the cell, which can change the row height, which
  reflows the *table's* host story, which can reflow the frame chain.
  Table layout is the deepest dependency chain and the hardest to scope —
  treat the whole containing story as dirty (no sub-cell incrementality
  in the first cut).

The safe shape is therefore: **re-emit dirtied stories (+ forward-reflow
closure through their chains) → re-run the global post-layout passes**.
The post-layout passes are O(built tree), already factored into
`resolve()`, and far cheaper than emission.

## 4. Staged implementation path + effort

Each stage is independently shippable and measurable against the W1.24
bench lane (`rebuild/text`, `rebuild/tables`).

### Stage A — content-aware body-story signature (~2 days)
Fold story content (runs + applied styles) into `body_story_signature`,
so an edit to story S leaves story T's signature unchanged. Replace the
wholesale `body_story_emit_cache.clear()` on text edits with a *selective*
invalidation keyed by the affected story id (already captured —
`story_id_for_mutation`). **Risk:** the signature must hash exactly what
emission consumes, or a stale hit ships wrong pixels. Gate with the
existing `emit_cache_undo` + determinism suites + a new "edit story A
doesn't re-emit story B" bench/assert. **Win:** multi-story documents stop
paying for untouched stories on every keystroke.

### Stage B — forward-reflow closure + dirty page recompute (~2–3 days)
After re-emitting a dirtied story, recompute its chain's occupied pages
from the post-relayout fit and union them into the dirty set; only then
re-run page assembly for the dirty pages. **Risk:** the threaded-overflow
case (insert grows the chain onto a new page) and the insert/delete-page
index-shift invariant. This is where the existing `insertPage`-mid-chain
panic lives — keep the page index in the cache key. Gate with the
editor-suite page-ops harness.

### Stage C — split the global post-layout passes out (~2–4 days)
Make `resolve()` (numbering, running headers, xrefs, TOC) a standalone
pass that runs over the *partially* rebuilt document, so Stages A+B can
skip emission for clean stories but still get correct page numbers /
headers / xrefs everywhere. **Risk:** these passes currently assume a
fully fresh `BuiltDocument`; they must become idempotent over a document
where most pages are reused and a few were re-emitted. **Win:** this is
what makes A+B *correct* rather than just fast-but-wrong on numbered docs.

### Explicitly out of scope (later)
- Sub-cell table incrementality (treat the whole table story as dirty).
- Salsa-style demand-driven memoisation of Tiers 2–4 (the `model.rs`
  doc comment's eventual "Phase 3" vision) — a much larger rewrite that
  Stages A–C are the pragmatic, shippable down-payment on.

**Total: ~6–9 eng-days** for A–C, gated at each stage by the W1.24 bench
lane (regression visibility) + the determinism / emit-cache-undo / page-ops
suites (correctness). The benches added in W1.24 are the instrument that
makes "did this stage actually win, and did it break a cache?" a number
instead of a guess.

---

*W1.24 (Full-Green campaign, audit B17–B19). Companion to the bench lane
(`crates/paged-canvas/benches/pipeline.rs`) and the `RebuildStats`
instrumentation (`model.rs`). MPL-2.0 OR PMEL.*
