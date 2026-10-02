# ADR 027 — Incremental flow invalidation: a complete key, an early stop, and a digest that proves it

- **Status:** ACCEPTED 2026-10-02 (was PROPOSED 2026-10-01); implemented in part (decisions 3
  to 7; decisions 1 and 2 are not built as written, see the amendment) in
  `crates/paged-canvas/src/model.rs` and `crates/paged-renderer/src/pipeline/`, in the steps
  recorded in the [implementation plan](../design/incremental-flow-plan.md). This is a
  forward-looking **ADR-as-memo**.
- **Extends:** [026](026-auto-growing-region-chains.md), which makes the page set an output of
  layout and so makes this ADR necessary rather than optional.
- **Gated by:** ADR 030. Measure the wasm build first.
- **Applies to:** `core` (`paged-canvas` model, `paged-renderer` pipeline, `paged-mutate`).

## The problem

An edit costs a whole-document rebuild: ~16 ms native, ~14 s in wasm on the 134-page annual
(`crates/paged-canvas/src/model.rs:1316` comment). Most of that cost is waste:

- A **per-paragraph layout cache** already persists across rebuilds (`layout_cache`,
  `crates/paged-canvas/src/model.rs:1255`). Unchanged paragraphs skip line breaking.
- A **per-story emit cache** exists (`body_story_emit_cache`,
  `crates/paged-canvas/src/model.rs:1297`). Its key
  (`crates/paged-renderer/src/pipeline/deltas.rs:55`) hashes the chain's frame geometry, the wrap
  shapes on its pages and the **absolute page index**. It does **not** include the story's
  content, styles or the variables it reads.
- That key would return stale output for any text edit, so every mutation entry point clears
  the whole cache: `crates/paged-canvas/src/model.rs:2181, 4050, 4110, 4162`. Correct, but a
  one-character edit re-emits every story on every page.

Flow raises the real question: can an edit change the whole document? Today, through core's
verified dependencies:

- **Its own story, all of it.** Reflow can move every downstream line across every page of
  the chain.
- **Other stories, only through:** shared definitions (style, swatch, font, colour settings),
  frame geometry (already in the key), post-layout values (running headers, page-number
  cross-references, re-emitted by `crates/paged-renderer/src/pipeline/links.rs`), cross-story
  list numbering (already disables
  the cache document-wide, `crates/paged-renderer/src/pipeline/build_engine.rs:2391`), and
  structural ops.
- **Wrap shapes are collected from frame geometry before any story is laid out**
  (`collect_wrap_rects_per_page`, `crates/paged-renderer/src/pipeline/build_engine.rs:2008`).
  So one story's layout never feeds
  another's, and one pass is enough. [026](026-auto-growing-region-chains.md) ends this: a
  grown page brings master content.

DOCX makes per-story caching insufficient on its own. A Word document is roughly **one** long
story, so the edited story *is* the document.

## The decision

1. **Record the inputs instead of maintaining a hand-written key.** Story emission records
   what it reads: the story's content revision, the ids of the styles, swatches and fonts it
   resolves, its chain's region identities and geometry, the wrap shapes on those regions, and
   the variables it resolves. The cache key is the hash of that read set.
   - A hand-maintained hash list is how the current key went stale the first time a new input
     appeared. That is the reason for the wholesale `clear()`.
   - Revisions come from `paged_mutate::apply`, which bumps a per-story content revision for
     every story an op touches, and a per-definition revision for styles, swatches and colour
     settings.
2. **Key on identity, never on absolute index.** Regions and pages are keyed by stable id
   (generated pages per [026](026-auto-growing-region-chains.md) §3). Page numbers are
   **variables** the emission reads, so a renumbering re-emits only the frames that display a
   number.
3. **Stop early inside a story.** After an edit at a given position, re-lay from the frame
   containing that position. When a frame starts **and** ends at the same text position as in
   the previous build, with keep rules applied ([028](028-pagination-rules-are-engine-owned.md)),
   every later frame of the chain is unchanged and is reused. Typing on page 3 of an 80-page
   DOCX re-lays one or two pages.
4. **Prove it against the full build.** A debug and CI mode runs the incremental build *and*
   a full rebuild after every op and compares `DisplayList::digest()` per page (the keystone
   that shipped with `paged-sdk`). It runs in the editor E2E suite and the showcase/annual
   drivers, and any difference fails the build. Incremental correctness is then tested, not
   assumed.
5. **One commit path.** The four copies of clear-then-rebuild fold into one
   `commit_and_rebuild(invalidation)`, and the two near-identical `PipelineOptions`
   constructions (`build_for_export`, `rebuild_after_mutation`) fold into one
   `pipeline_options(purpose)`. Invalidation then has one place to live.
6. **Structural ops still rebuild everything.** Inserting or deleting authored pages or
   frames, changing a master, applying a master to a page, and loading a document keep the
   full rebuild. Correctness first; this ADR only narrows the common case.
7. **Dirty pages drive redraw.** The set of pages whose emission changed replaces the
   coarse `story_pages` hint (`crates/paged-canvas/src/model.rs:1260`) for GPU tile invalidation.
   Visible pages are
   composed and presented first.

## Options weighed

| Option | Verdict |
|---|---|
| Keep the full rebuild, only make wasm faster | Necessary (ADR 030) but not enough. A full rebuild still scales with the document, and DOCX will be long. |
| Add the content revision to today's key by hand | The minimal patch. Rejected as the end state because it repeats the failure that caused the wholesale clear. Acceptable as step one if the digest gate (decision 4) lands first. |
| **Record what emission reads + early stop + digest gate** | **Chosen.** |

## Consequences

- Text edits stop clearing the emit caches. Style edits invalidate only the stories that read
  the style.
- The digest gate becomes a required CI lane. It doubles build time in that lane only.
- The early stop needs the previous build's per-frame start and end positions kept in the
  model. That is a small memory cost per frame.
- [026](026-auto-growing-region-chains.md)'s growth loop and this ADR interact: the loop
  repeats until the page set *and* the per-frame break positions stop changing.

## What this ADR does NOT decide

- Background or off-main-thread layout of pages that aren't visible (a later optimisation;
  decision 7 only orders the work).
- Incremental *structural* ops (decision 6 keeps them full).

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The steps taken, with their commits and measured
results, are in [`../design/incremental-flow-plan.md`](../design/incremental-flow-plan.md), §6.
Decisions 3 to 7 are built, with the differences noted below. Decisions 1 and 2 are not built
as written: there is no recorded read set and no revision counter, and a text edit is kept
away from other stories' cache entries by the commit path instead.

**Decision 4, the digest gate: built.**

- `crates/paged-canvas/src/model.rs:8917-8938` — `digest_gate_check` builds the scene cold and
  compares; `set_digest_gate` turns the per-rebuild check on for one model. `:1744` — the
  environment variable `PAGED_DIGEST_GATE=1` turns it on for every model. `:8909-8912` — a
  difference panics.
- `crates/paged-canvas/src/model.rs:9687-9726` — `compare_builds` compares the page ids, each
  page's `DisplayList::digest()`, each page's `story_layout` and the diagnostics. The decision
  names only the digest.
- `crates/paged-canvas/tests/digest_gate.rs:15-28` — the test that scripts edits and checks
  after every operation.

In this repository the gate is that integration test; no file under `.github/workflows` names
it, so it runs with the workspace tests (`.github/workflows/ci.yml:83`) and is not a lane of
its own. This qualifies, in
Consequences, "The digest gate becomes a required CI lane". Whether the editor's suites run it
(decision 4, "It runs in the editor E2E suite and the showcase/annual drivers") is outside this
repository.

**Decision 5, one commit path: built.**

- `crates/paged-canvas/src/model.rs:8681-8777` — `commit_and_rebuild(invalidation)`;
  `:9661-9675` — `Invalidation` is `Everything` or `Text { story, edit }`.
- `crates/paged-canvas/src/model.rs:8527` — `pipeline_options`; `:9677-9685` —
  `PipelinePurpose` is `Live` or `Export`.

This supersedes, in "The problem", "every mutation entry point clears the whole cache:
`crates/paged-canvas/src/model.rs:2181, 4050, 4110, 4162`".

**Decisions 1 and 2, the key: not built as written.**

- `crates/paged-renderer/src/pipeline/deltas.rs:55-110` — the body-story key is still
  hand-written: the story id and `body_story_signature` over the chain's frames, the wrap
  shapes on its pages and the absolute page index (`:84-91`), mixed at the call site with the
  list-numbering ledger and, for a story that prints page numbers or variables, a
  page-numbering key (`crates/paged-renderer/src/pipeline/build_engine.rs:2751-2797`). No part
  of it is recorded from what the emission read.
- No revision counter exists: the word "revision" does not occur in `crates/paged-mutate/src`.
- `crates/paged-canvas/src/model.rs:8720-8745` — what a text edit does instead: it drops the
  body-story entries of the edited story only (both emit caches whole, when the story has no
  frame chain of its own or its frame is anchored in another story). `:2254`, `:4162-4170`, `:4217-4226` — text operations and their undo and redo
  take this path.
- `crates/paged-canvas/src/model.rs:4137`, `:8712-8719` — every other operation drops the
  master-text and body-story caches whole.

This supersedes, in Consequences, "Style edits invalidate only the stories that read the
style": a style edit is one of the other operations. "Text edits stop clearing the emit
caches" holds for the text operations.

**Decision 3, the early stop: built, at paragraph granularity.** This supersedes "When a
frame starts **and** ends at the same text position as in the previous build": the test is
made at paragraph boundaries and compares the emitter's flow state.

- `crates/paged-renderer/src/pipeline/resume.rs:15-39` — an edited story resumes at the first
  changed paragraph and stops at the first later paragraph boundary whose flow state equals
  the previous emission's; the output before and after is spliced from the previous emission.
- `crates/paged-renderer/src/pipeline/resume.rs:140-145` — only stories whose emission is "a
  pure function of the flow state" are recorded; the others re-emit from their first
  paragraph.

**Decision 6, structural operations: holds.** They go through `Invalidation::Everything`, and
only a build whose commits were text edits of one story narrows anything
(`crates/paged-canvas/src/model.rs:1379-1392`).

**Decision 7, dirty pages: built for text edits of one story.**

- `crates/paged-canvas/src/model.rs:8867-8904` — after such a build the changed pages are the
  pages laid out afresh, the new pages, and the pages of the story's auto-sizing frames; after
  any other build every page is reported.
- `crates/paged-canvas/src/model.rs:8795-8812`, `crates/paged-renderer/src/pipeline/mod.rs:417-424`
  — in that case the previous build's pages are lent to the pipeline, and a page nothing was
  laid out on afresh is moved in instead of being finished again.
- `crates/paged-canvas-wasm/src/dispatch.rs:135-153` — the GPU scene cache invalidates the
  narrowed pages when the model has them and otherwise falls back to the pages of the story.
  The `story_pages` hint is therefore not replaced; it is the fallback.

**The timing in "The problem".** The plan's §0 measured one typed character on the 134-page
document at 149 ms native and 178 ms in wasm at `6dea692`, before its steps. The "~14 s in
wasm" above, and the comment it cites (now at `crates/paged-canvas/src/model.rs:1330`), predate
that measurement.
