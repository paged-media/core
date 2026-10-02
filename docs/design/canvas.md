# IDML Canvas — Technical Concept

May 2026. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

*Status note (2026-10-02): the worker side of this concept is `crates/paged-canvas` with its binding layer `crates/paged-canvas-wasm`; the main-thread side (UI shell, input pipeline, overlay) is in the editor repository. The sections on incremental computation with salsa (§3.6, §7) and on incremental line breaking (§4.2) were not adopted as written; [ADR 107](../adr/107-whole-document-build.md) and [ADR 027](../adr/027-incremental-flow-invalidation.md) record what was built.*

---

## 1. Executive summary

This concept describes the architecture of the IDML web canvas — the rendering, navigation, and interaction surface that powers all upcoming functionality from a passive document viewer through a property inspector to a full visual editor. It is the single client-side component through which every user-facing capability of the IDML stack is delivered.

The canvas is conceived as a long-lived investment. Its design horizon is the editor; everything it does in earlier phases must lay foundations for the editor's hardest cases — incremental editing of multi-hundred-page documents with table of contents, footnotes, cross-references, and threaded text frames, at any zoom level, with sub-frame latency on user input.

The architecture rests on three commitments. First, **strict separation of concerns** between the document model, the render pipeline, and the UI shell, communicating only through typed message channels. Second, a **four-tier incremental pipeline** (content, layout, resolution, output) where each tier consumes upstream facts and produces facts for the next, with dependency-tracked recomputation so that mutations affect only the minimum necessary subset of derived state. Third, **content-space addressing** throughout the editor surface, so that selection, hit-testing, and mutation are independent of zoom or display resolution.

The renderer backend is already in place as Vello-on-WebGPU compiled through `paged-sdk`. This document concerns everything above it: how documents are modeled in JavaScript, how layout is made incremental, how cross-page artifacts are resolved, how the viewport is presented and navigated, how editor mutations propagate, and how the whole system stays fast at scale.

---

## 2. Goals and non-goals

### 2.1 Goals

The canvas must provide a smooth, WYSIWYG rendering of IDML documents up to several hundred pages, with continuous navigation between overview and close-inspection zoom levels. Frame-time budgets are 16 ms for the input-to-pixel path during pan and zoom, and 100 ms or better for first paint after a navigation jump. Memory must stay bounded regardless of document length through tiered caching with explicit budgets.

The canvas must correctly handle every IDML feature that produces cross-page or cross-frame relationships: threaded text through frame chains, footnotes that co-decide page breaks with body content, computed stories (table of contents, indexes), inline cross-references and page references, running headers driven by document state, and any IDML field type whose value is derived from layout results.

The canvas must be designed for editability from day one. Every architectural decision is evaluated against the editor case as well as the viewer case. The user must be able to insert, delete, and reformat content with sub-frame visible latency on the active paragraph and bounded latency for downstream propagation. Editor operations must address content (story IDs and offsets), never pixels, so that they remain correct at any zoom.

The canvas must serve as the foundation for the inspector. Hit-testing must return both presentation-level objects (frame, image, group) and content-level positions (story ID and offset within story) from a single document-space coordinate.

### 2.2 Non-goals

*Status note (2026-10-02): `paged-parse` no longer exists in this repository; the IDML reader moved to the plugin-publish repository ([ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md)).*

This concept does not define the backend renderer itself. The renderer is the existing Rust pipeline through `paged-parse`, `paged-scene`, `paged-text`, `paged-compose`, and `paged-gpu`, with Vello as the GPU rasterizer. The concept defines the contract between that renderer and everything above it.

This concept does not define the IDML feature roadmap of the renderer (e.g., when drop shadows, gradients, OpenType features, or CJK composition land). Those are tracked in the original design spec. The canvas architecture must accommodate features as they arrive without structural change.

This concept does not define the inspector or editor user interfaces. Those are downstream projects that consume this canvas. The concept defines the API surface they will consume.

This concept does not define collaboration, undo/redo persistence formats, or asset management. Those are concerns of the surrounding application.

### 2.3 Explicit performance targets

| Operation | Target | Hard ceiling |
|---|---|---|
| Pan, zoom: input-to-pixel | 8 ms | 16 ms |
| Page jump animation | 200 ms total | 300 ms |
| First paint of new document, 100 pages | 800 ms | 1500 ms |
| First paint of new document, 500 pages | 2000 ms | 4000 ms |
| Single-character keystroke to visible glyph | 16 ms | 32 ms |
| Full TOC update after heading edit | 200 ms | 500 ms |
| Frame resize: visible reflow | 33 ms | 66 ms |
| Memory ceiling, 500-page document at idle | 400 MB | 700 MB |

All targets are measured on a 2022-class developer laptop at 1× device pixel ratio. Mobile and high-DPR targets relax these by 50%.

---

## 3. Architectural principles

### 3.1 Three-thread separation

The system runs on three threads of execution with explicit boundaries between them.

**Main thread** owns the React UI tree, the camera transform, the input pipeline, the lightweight document model (page index, story list, structural relationships), the overlay DOM layer, and the message channel to the worker. It never touches WebGPU. It performs no glyph layout, no path rasterization, no heavy computation. Its work per frame is bounded by input handling and DOM reconciliation.

**Worker thread** owns the `OffscreenCanvas`, the WebGPU device and context, the WASM module, the rendering pipeline, the LOD cache, the spatial indexes, and the layout and resolution machinery. It receives camera updates and content mutations from the main thread and produces rendered frames into the canvas. The worker has its own render loop driven by a frame scheduler that prioritizes viewport-visible work.

**GPU** runs the WGSL shaders submitted by Vello. The worker submits compute and render passes; GPU work is opaque to the rest of the system.

The boundary between main thread and worker is a typed message channel for content operations (load, mutate, query) and a `SharedArrayBuffer` for the camera transform (read by the worker at the start of every frame, written by the main thread on every input event, no postMessage round-trip). Camera updates are the highest-frequency communication and must be lock-free; content operations are lower-frequency and may use structured cloning.

### 3.2 Four-tier incremental pipeline

*Status note (2026-10-02): this section predates the implementation; the tiers are not separately cached with dependency-tracked recomputation. Layout is one build over the whole document, made cheaper by caches the caller passes in: see [ADR 107](../adr/107-whole-document-build.md) and [ADR 027](../adr/027-incremental-flow-invalidation.md).*

The document is processed through four tiers, each consuming inputs and producing outputs that downstream tiers depend on.

**Tier 1 — Content.** The document model in its canonical form: stories with structural markup, frame definitions, anchor table, computed-story registry, master-page references, style definitions. This tier is the source of truth for everything the user can edit. Mutations enter the system here.

**Tier 2 — Per-story layout.** For each story, Knuth-Plass paragraph composition through its frame chain, producing positioned glyphs, line break decisions, frame fill states, and page-break decisions including footnote placement. Composition is incremental: a checkpoint cache stores stable resume points, and an edit at offset X resumes from the last checkpoint ≤ X.

**Tier 3 — Resolution.** A linear pass over the laid-out document that assigns page numbers, footnote markers, figure numbers, section numbers, and resolves all field references by anchor ID lookup. Produces a numbering map and a diff of fields whose resolved text changed since the previous resolution pass.

**Tier 4 — Per-page output.** Display lists keyed by page ID and the (layout, numbering) generation that produced them. Display lists are sliced from the laid-out content by page bounding box. The LOD cache sits below this tier and produces three resolution tiers of rendered output (snapshot, mid-resolution bitmap, live Vello tiles) on demand from the display lists.

Each tier is independently testable. Each tier has explicit cache invalidation semantics. Each tier produces facts that downstream tiers consume by ID, not by reference, so that intermediate results can be persisted and recovered.

### 3.3 Content-space addressing

Every user-visible operation is expressed in content space, never pixel space. A text selection is `{story_id, start_offset, end_offset}`. A frame manipulation is a transformation on geometry in document points (1/72 inch). A drag-and-drop is a new position in document units. The cursor's screen position is *derived* from the current layout for display purposes; the cursor's *identity* is a content offset.

This principle has two consequences. First, the editor surface is correct at any zoom — including extreme zoom-out where individual glyphs are sub-pixel — because operations don't depend on pixel-precise input. Second, operations are stable across re-layouts: if the user has selected paragraph 3 of story X and an upstream edit changes pagination, the selection still points to paragraph 3 of story X; it doesn't drift because its position on screen changed.

### 3.4 Stable identity

Every object the user can refer to has a stable ID independent of its layout position. Stories have IDs (IDML's `Self` attributes provide these directly). Anchors within stories have IDs (also from IDML where present; synthesized stably for content the user creates). Frames, pages, layers, styles all have IDs. The dependency graph references everything by ID.

Layout positions, page numbers, and rendered output are derived from stable IDs but are themselves regenerable. Cache keys include both the ID and a generation counter so that stale cache entries can be detected without invalidation cascades through unrelated state.

### 3.5 Progressive fidelity

The canvas never blocks user-visible work on full-resolution rendering. Every page is always renderable at some quality level immediately, with refinement happening in the background. The LOD cache guarantees this: the snapshot tier is never evicted, so any page can be drawn at low resolution in microseconds; higher tiers are computed on demand and fall back to the next tier down while computing.

The same principle applies to layout: when the user types, the active paragraph re-composes synchronously and renders immediately; downstream incremental composition queues asynchronously and the user sees the local effect of their edit before the document settles globally.

### 3.6 Deterministic invalidation

*Status note (2026-10-02): this section predates the implementation; salsa was not adopted and no manifest in this repository names it as a dependency. See [ADR 107](../adr/107-whole-document-build.md).*

Cache entries are keyed by inputs, not by clock time or sequence number. If two mutation sequences produce the same intermediate state, they hit the same cache entries. This makes the system testable: snapshot the input state, replay a mutation, assert the derived state matches.

This principle is what motivates the recommendation to model the dependency graph using `salsa` or an equivalent demand-driven memoization system (Section 7). Hand-rolled invalidation is the largest single source of subtle bugs in editor implementations; using a library-supported query system removes most of the surface area for those bugs.

---

## 4. The four-tier pipeline in detail

### 4.1 Tier 1 — Content

The content tier is the canonical representation of the document. It is what gets persisted (round-trippable to IDML), what mutations operate on, and what every other tier ultimately depends on.

**Stories** are sequences of structured text content. A story is a list of paragraphs; each paragraph is a list of runs; each run carries character style attributes and a payload that is one of: text, an inline field, an inline anchor target, or an embedded object (inline image, anchored frame). Stories have IDs, are independent of presentation, and are the unit of authorship. Threaded text and computed stories are both expressed as stories — the difference is only in how the content arises.

**Frames** are presentation surfaces with geometric extent. A frame has an ID, a bounding shape (rectangle, polygon, or path), a transform, and a position on a specific page. Frames are organized into **chains**: an ordered list of frames that together hold one story. Most stories occupy a single frame; threaded text stories occupy multiple frames, often across multiple pages. The frame chain is the binding between the story (content) and the spread (presentation).

The `frame-for-story index` in the current `paged-scene` is the inverse mapping: given a story ID, which frame chain holds it. This index is already present and is the foundation for every cross-frame operation.

**Anchors** are named positions within stories that other content can reference. Every paragraph with a paragraph style marked as a "heading style" automatically produces an anchor. Every footnote marker produces an anchor (the body-side anchor of the footnote). Cross-reference targets, index entries, and bookmarks produce anchors. The anchor table maps anchor ID to `{story_id, paragraph_index, run_index, character_offset_within_run}`. Anchor positions are content-stable: they move with the content they're attached to, not with the layout.

**Computed stories** are stories whose content is a function of anchors and document state. The most important examples are tables of contents, indexes, and lists of figures. A computed story has a *definition* (e.g., "collect every heading at paragraph styles 'Heading 1' and 'Heading 2'") and a *materialization function* that produces text runs from the current anchor table and numbering map. The materialization is deterministic: given the same inputs, it produces the same output. Computed stories flow through frame chains exactly like regular stories.

**Fields** are placeholders within text runs that get resolved during the resolution pass. The field types the canvas must support:

- `PageRef(anchor_id)` — resolves to the page number of the anchor's containing line.
- `TextRef(anchor_id)` — resolves to the text content of the anchor's containing paragraph (used by TOCs to quote heading text, and by cross-references to repeat target text).
- `AutoNumber(scope, format)` — resolves to a counter value (footnote markers, figure numbers, equation numbers, list numbers).
- `Variable(name)` — resolves to a document-level or section-level variable (chapter title from running heading, document date, file name).
- `RunningHeader(style)` — resolves to the most recent text in the document using the named paragraph style (e.g., chapter title in the page header).

Fields have a stable identity (an ID) and a target (typically an anchor ID or a variable name). Resolution produces a text value for each field; the value is consumed by tier 4 when generating the display list.

**Master pages** are presentation templates applied to ranges of pages. A master defines frames, repeating content (page numbers, running headers, decorative elements), and structural rules. Most master content involves fields that resolve per-page (e.g., the page number field on a master resolves to the page number it's currently on). The canvas treats master content as if it were stories with per-page resolution context.

**Style definitions** (paragraph styles, character styles, object styles) live in tier 1 as named definitions. Runs reference styles by name; the layout tier resolves the cascade. Style edits propagate to every run that uses the style, invalidating only the affected stories.

### 4.2 Tier 2 — Per-story layout

*Status note (2026-10-02): this section predates the implementation; the composer is not incremental (a paragraph is laid out whole). What was built is a per-paragraph layout cache (`crates/paged-text/src/cache.rs`) and, per story, a resume and an early stop at paragraph boundaries (`crates/paged-renderer/src/pipeline/resume.rs`): see [ADR 027](../adr/027-incremental-flow-invalidation.md).*

The layout tier consumes a story and its frame chain and produces a laid-out story: a sequence of paragraphs, each with a sequence of lines, each with a list of positioned glyph clusters and the frame they belong to. Layout also produces frame-level facts: which frames are full, which are partially filled, where the story ends.

**Composition algorithm.** Layout uses Knuth-Plass optimal paragraph composition as already implemented in `paged-text`. The composer is extended in two ways for the canvas.

First, **frame chain awareness**. The composer processes paragraphs sequentially. As it fills a frame, it checks the frame's remaining height; when a line would overflow, it considers the page break and the next frame in the chain. Page break decisions are co-decided with footnote placement (see below) and with `keep-with-next` and `widow-orphan` constraints.

Second, **checkpoint caching**. The composer saves layout state at every paragraph boundary that meets stability criteria:

- No active widow/orphan constraint crossing the boundary
- No `keep-with-next` between this paragraph and the next
- No spanning column constraint crossing the boundary
- The paragraph's break decisions are not being optimized jointly with subsequent paragraphs (Knuth-Plass operates on paragraph scope, so this is almost always true)

At a checkpoint, the composer records: the current position in the frame chain (frame index, vertical offset within frame), the current page number generation, the current footnote-counter generation, and any cross-paragraph state (e.g., open vertical-justification slack). An edit at story offset X invalidates the cached lines from offset X onward; composition resumes from the latest checkpoint ≤ X and runs forward.

**Short-circuit detection.** As the resumed composition runs forward, it compares its output against the cached tail. As soon as it produces a sequence of line break decisions identical to the cached tail at a checkpoint, it stops and re-uses the cached lines. In typical editing, an insertion of a single character changes line breaks within a paragraph and possibly the next; composition short-circuits after at most 2–3 paragraphs.

**Footnote handling.** Footnotes are stories whose body-side anchor sits within another story. When the composer crosses a footnote anchor, it composes the footnote story into its frame chain (typically a per-page footnote slot at the bottom of the page) up to the line that contains the anchor. It then checks: can the line containing the anchor fit on the current page, given that the page's footnote slot now contains this footnote's first line plus all footnotes anchored above? If yes, the line is placed. If no, the line — and the footnote, and any preceding lines bound by `keep-with-next` — push to the next page, and the footnote story's frame chain is updated.

This co-decision is the critical correctness property for footnotes. It cannot be expressed as two independent passes: the body's page-break decision depends on the footnote's height, and the footnote's frame placement depends on which page the anchor lands on.

Long footnotes that span pages (rare but legal in IDML) introduce a forward dependency: the body composer needs to know how much of the footnote fits on each page before placing the body anchor. The implementation strategy is to compose footnotes lazily and cache the result: first call composes as much as fits on the current page, subsequent calls extend.

**Frame chain reflow.** When a story's content lengthens or shortens, frame fills shift forward. If new content overflows the last frame in the chain, the story is marked "overset" (the IDML term) and the editor surfaces a warning. If content shortens enough to empty later frames in the chain, those frames remain present but empty (the user controls frame existence, not the composer).

**Output schema.** Per laid-out story:

- A list of `LaidOutParagraph` objects, each with style attributes and a list of `LaidOutLine` objects
- Each `LaidOutLine` has a frame ID, a position within the frame, a list of `PositionedCluster` objects (glyph cluster + position + style), and the source `{paragraph_index, character_range}` it represents
- Per frame: which character range of the story it contains, total fill height, list of vertical positions of inline-anchored objects
- Per page (derived): which frames are present, where they are, list of footnote frames and their content

This schema is the contract with tier 3 and tier 4.

**Performance budget.** A clean compose of a 100-paragraph story on a 2022-class laptop: target 50 ms. A resumed compose from a checkpoint, short-circuiting within 3 paragraphs: target 2 ms. These budgets are what make typing latency achievable.

### 4.3 Tier 3 — Resolution

*Status note (2026-10-02): this section predates the implementation; `crates/paged-canvas/src/resolve.rs` holds a resolution pass with a numbering map and a field diff that assigns page numbers. Running headers and text-anchor destinations are resolved by one second run of the document build (`build_document_fixed` in `crates/paged-renderer/src/pipeline/mod.rs`): see [ADR 107](../adr/107-whole-document-build.md).*

The resolution tier consumes the laid-out document and produces resolved field values.

**Numbering pass.** A single linear walk over pages assigns:

- Page numbers (respecting section breaks, restart points, and numbering format like Roman vs. Arabic)
- Footnote markers (per-page restart by default, configurable to per-document or per-section)
- Figure, table, equation numbers (per the autonumber configuration in styles)
- Section and chapter counters

The walk produces a **numbering map**: a dictionary keyed by anchor ID, with values containing all the numeric facts about that anchor's position (page number, line number if requested, chapter number, section number, figure-style identifier if applicable).

**Field resolution.** A second linear walk over all stories looks at every field and computes its resolved text:

- `PageRef(anchor_id)` → numbering_map[anchor_id].page_number, formatted per the field's style
- `TextRef(anchor_id)` → the paragraph text from the anchor's containing paragraph in the content tier
- `AutoNumber(scope, format)` → the scope's counter value at the anchor's position
- `Variable(name)` → looked up in the variable table (document- or section-scoped)
- `RunningHeader(style)` → for each page, the most recent paragraph with the named style; resolved per-page, not per-field

Each field's previously resolved value is compared to its new value; the resolution pass outputs a **field diff**: the list of `(field_id, story_id, old_text, new_text)` for every field whose text changed.

**Feedback to tier 2.** The field diff is the feedback signal that drives convergence. Any story containing a field in the diff is content-dirty (its text has changed) and re-enters tier 2. The most common case is a TOC whose entries' page numbers changed; the TOC is one story, it re-flows quickly, and tier 3 runs again.

**Convergence guarantees.** The feedback loop terminates in practice because each iteration either (a) doesn't change anything (steady state), (b) shifts content by a small amount that doesn't push the TOC into another page, or (c) shifts the TOC's own page count, which is the only case where a third iteration may be needed. The system caps at 4 iterations and raises a warning if reached — this almost always indicates a structural problem in the document itself (e.g., a TOC whose growth pushes itself onto more pages whose page numbers then change in a way that grows the TOC further).

**Performance budget.** The resolution pass is linear in document size and does no glyph work. Target: 20 ms on a 500-page document with 1000 fields. This is achievable because all data is in flat arrays at this point.

### 4.4 Tier 4 — Per-page output

The output tier produces the data that the GPU renderer consumes.

**Display list per page.** For each page, a `DisplayList` is sliced from the laid-out content. The slice includes:

- All frames whose bounding box intersects the page bounding box
- For each such frame, the positioned clusters that belong to that page (frames spanning pages are content-flowed but visually clipped per-page)
- Footnote frames on the page
- Master-page content for the page (resolved per-page for fields like page number)
- Frame chrome (borders, fills, stroke if any)

The display list is keyed by `(page_id, layout_generation, numbering_generation)`. The layout generation increments whenever the laid-out content for any frame on the page changes; the numbering generation increments whenever any resolved field on the page changes.

**LOD cache.** Three resolution tiers, each with explicit memory budget and eviction policy.

*Snapshot tier* (target: 256–512 px page width). A thumbnail of every page in the document, packed into a texture atlas. Used by the page navigator, minimap, and the canvas at overview zoom. Generated lazily on first request, cached indefinitely (never evicted). Memory: ~64 KB per page, ~32 MB for a 500-page document. Regenerated for a page only when that page's layout or numbering generation changes.

*Mid-resolution tier* (target: 1024–2048 px page width). A bitmap per page, sufficient for page-fit and near-page-fit zoom. LRU eviction with a budget of ~200 pages cached at any time. Memory: ~4 MB per page, capped at ~800 MB; in practice the working set is ~50 pages at ~200 MB. Regenerated on layout or numbering generation change.

*Live tiles tier* (variable resolution, per zoom). At close zoom, the renderer produces tiles directly from the display list using Vello. Tiles are keyed by `(page_id, zoom_band, tile_x, tile_y, layout_generation, numbering_generation)`. Only tiles intersecting the viewport plus a one-tile margin are produced. LRU eviction with a memory ceiling enforced per zoom band.

**Tier transitions.** The renderer always has *something* to draw because the snapshot tier is never evicted. When the user requests a zoom level for which higher-tier data is not yet ready, the canvas draws the snapshot upsampled (or the mid-res if available) and refines as higher tiers arrive. Transitions are sample-blended over 1–2 frames so the user perceives sharpening, not popping.

**Cache invalidation.** When tier 2 reports that a story's layout changed, the canvas computes the set of affected pages from the frame chain. All three LOD tiers mark those pages stale. The mid-res and snapshot tiers regenerate in the background at low priority; the live tile tier regenerates on demand for tiles in the viewport. Pages outside the viewport are not re-rendered until they enter it.

**Performance budget.** Display list assembly for a single page: target 1 ms. Snapshot rendering: target 20 ms per page (acceptable because it runs in the background). Mid-res rendering: target 50 ms per page. Live tile rendering: target 8 ms per tile, with viewport repaint of all visible tiles completing within one frame budget.

---

## 5. Worker–main thread contract

*Status note (2026-10-02): this section predates the implementation; see [ADR 115](../adr/115-worker-boundary-transports.md) for the worker boundary as built. The message kinds are `MainToWorkerKind` and `WorkerToMainKind` in `crates/paged-canvas/src/channel.rs`.*

### 5.1 Camera state via SharedArrayBuffer

The camera transform is a 2D affine: `{scale, translate_x, translate_y}` plus a timestamp. It lives in a `SharedArrayBuffer` of fixed layout, accessed by both threads.

The main thread writes the camera on every input event using `Atomics.store` for each field, then bumps a generation counter using `Atomics.add` to mark the write complete. The worker reads the generation counter at the start of every frame using `Atomics.load`, and if it differs from the last frame's generation, reads the camera fields and uses the new value. No locks, no `postMessage`, no copy.

There is a benign race: the worker can read a half-written camera transform between the main thread's writes to individual fields and the generation bump. In practice this manifests as a single-frame visual glitch at most; the next frame reads the consistent value. For the camera specifically, this is acceptable. For anything where torn reads would cause incorrect behavior (selection state, mutation commits), the typed message channel is used.

### 5.2 Typed message channel

The message channel handles everything that is not pure camera. Messages are typed and versioned.

**Main → worker:**

- `LoadDocument(bytes)` — initial document load, returns a `DocumentHandle` with page count, story count, and structural metadata
- `Mutate(operation)` — content mutation (see Section 6)
- `RequestPage(page_id, lod_tier)` — explicit request to ensure a page is rendered at a given tier (typically driven by the navigator opening or by an upcoming zoom transition)
- `HitTest(page_id, doc_point, hit_filter)` — synchronous-style query for what is at a given document point on a given page
- `SelectionGeometry(story_id, range)` — request the geometry (sequence of rects) for rendering a selection in the overlay
- `CaretGeometry(story_id, offset)` — request the cursor position and line height
- `Subscribe(channel_id)` — register interest in worker-side events
- `Unsubscribe(channel_id)`

**Worker → main:**

- `DocumentLoaded(handle)` — initial load complete
- `PagesDirty([page_ids])` — these pages now have new display lists; if they're visible, re-render
- `StoryDirty(story_id, change_set)` — a story's content changed (used by inspector to update content views)
- `LayoutProgressed(progress)` — incremental layout has advanced; used to drive a progress indicator during long initial layouts
- `Warning(kind, details)` — convergence cap reached, overset text, missing fonts, etc.
- `Error(kind, details)` — unrecoverable problem (corrupted document, WASM panic recovery state)

All messages use structured cloning by default. Large payloads (initial document bytes) are transferred where possible.

### 5.3 Backpressure

The main thread can issue many `HitTest` and `Geometry` queries quickly (e.g., during a drag). The worker maintains a small queue and coalesces or drops obsolete queries — if a new `HitTest` arrives while two older ones are queued for the same page, the older ones are discarded since the result would no longer match the user's pointer position. Mutations are never coalesced; they are processed in order and acknowledged with a generation number.

### 5.4 Determinism and replay

*Status note (2026-10-02): this section predates the implementation; undo and redo are built on pre-captured inverse operations, not on replay from a snapshot: see [ADR 110](../adr/110-one-undo-timeline.md).*

Every mutation has a sequence number. The worker's state is a deterministic function of `(initial document, ordered mutation sequence, font and asset resolution results)`. This enables several capabilities:

- **Undo/redo** at the canvas level by replaying from a snapshot
- **Test reproducibility** by recording mutation traces and asserting derived state
- **Recovery from worker crash**: the main thread can spawn a new worker, replay the mutation sequence, and resume
- **Collaboration** (out of scope here but unblocked by this property) via shared mutation logs

---

## 6. Editor mutations and the propagation cascade

*Status note (2026-10-02): this section predates the implementation; see [ADR 116](../adr/116-mutations-lowered-onto-operations.md) (wire mutations and the operations they are lowered onto), [ADR 117](../adr/117-story-local-text-offsets.md) (text addresses) and [ADR 107](../adr/107-whole-document-build.md) (a mutation is followed by a whole-document build). No crate in this repository implements the synchronous main-thread composer of §6.2 and §6.3.*

### 6.1 Mutation API

Mutations are content-space operations. A mutation is a single message that the worker applies atomically:

- `InsertText(story_id, offset, text)` — insert characters at offset
- `DeleteRange(story_id, start, end)` — delete a range
- `ApplyStyle(story_id, range, style_attributes)` — set or unset style attributes on a range
- `InsertField(story_id, offset, field)` — insert a field placeholder
- `MoveFrame(frame_id, new_transform)` — change a frame's position or transform
- `ResizeFrame(frame_id, new_geometry)` — change a frame's bounding shape
- `LinkFrames(frame_a, frame_b)` — extend a frame chain
- `UnlinkFrames(frame_chain_id, after_frame)` — split a frame chain
- `InsertPage(after_page_id, master_id)` — add a page using a master
- `DeletePage(page_id)` — remove a page (frames go with it; stories using those frames reflow)
- `InsertFrame(page_id, geometry, content_kind)` — add a new frame
- `DeleteFrame(frame_id)` — remove a frame (story content reflows to remaining chain)

Each mutation has a unique ID and a sequence number. The worker acknowledges with `MutationApplied(seq, change_set)` where `change_set` describes what derived state went stale (which stories, which pages, which fields).

### 6.2 The propagation cascade (worked examples)

**Example A: User types one character into a body paragraph in chapter 5.**

1. *Main thread (synchronous, this frame).* The character is captured by the keystroke handler. The main thread already knows the active paragraph's style attributes and metrics (from a cached layout query for the current selection). It runs a synchronous Knuth-Plass pass over the active paragraph only, using a lightweight composer running in the main thread (a stripped subset of `paged-text`). It patches the active page's display list locally and submits a redraw of the active tile. The user sees the character within 16 ms.

2. *Main → worker.* Mutation message dispatched: `InsertText(body_story_id, offset, "X")`.

3. *Worker (asynchronous).* Tier 2 resumes composition from the checkpoint before the edit. K-P short-circuits within 1–2 paragraphs because line breaks converge. No frame-level changes; the chapter 5 frame chain is unaffected downstream. Tier 3 sees no changes to anchor text or page positions, so the numbering map is unchanged; field diff is empty (or contains only fields whose `TextRef` target is the edited paragraph). Tier 4 marks the active page dirty and re-generates its display list and the mid-res cache entry.

4. *Worker → main.* `PagesDirty([active_page_id])` — but the main thread already redrew this page in step 1, so it's already correct. The mid-res tier swap happens silently when the worker finishes.

Total visible latency: one frame. Background work: a few milliseconds.

**Example B: User inserts a half-page figure mid-chapter 2 of a 200-page document.**

1. *Main thread.* The user drags a figure from a library into the canvas. On drop, the main thread issues `InsertFrame(page_id, geometry, image)` plus `InsertField(...)` for the figure's auto-numbered caption.

2. *Worker.* Tier 2 sees that chapter 2's body story now has a new anchored frame in the middle. The composer resumes from the checkpoint before the insertion, accommodates the figure's anchor (which displaces text below it), and runs forward. Line breaks shift; line counts per page change. Composition continues forward until either (a) it finds a checkpoint where the laid-out tail matches the cache, or (b) it reaches the end of the story. For a half-page figure in a 200-page chapter, composition typically settles within the next several pages but every subsequent chapter's *pagination* is shifted — chapter 3 now starts one or two pages later, chapter 4 likewise, and so on.

3. *Worker (tier 3).* The numbering pass walks pages. Page numbers for everything from chapter 3 onward shift by 1–2. The figure's auto-number resolves (it's "Figure 2.7" or whatever the counter produces). The numbering map has ~150 entries changed.

4. *Worker (tier 3, field resolution).* Every field whose target shifted gets a new value. The TOC contains entries for every heading from chapter 3 onward; their page numbers all changed. Every cross-reference whose target moved gets updated. Running headers on every page from chapter 3 onward refer to potentially-changed chapter titles (unchanged in this case, since chapter content wasn't edited, only paginated differently). Field diff has ~50–100 entries.

5. *Worker (back to tier 2).* The TOC story is content-dirty: many fields within it have new resolved text. The TOC re-flows. Its length is typically stable (number text width is similar), but if it's near a page boundary it might cross. Re-flow takes a few milliseconds.

6. *Worker (tier 3 again).* Numbering pass runs again; field diff is now small (only fields on the TOC's pages if they changed). Usually converges here.

7. *Worker (tier 4).* All affected pages have their display lists invalidated. The visible pages re-render at high priority; the rest queue at low priority. Background mid-res cache regeneration spreads over the next 1–2 seconds.

Total visible latency for the inserted figure to appear in place: target 100 ms. Total time for the entire downstream cascade (TOC updated, page numbers settled in cache, all background tiers refreshed): target 1–2 seconds, none of it blocking the user.

**Example C: User edits a heading in chapter 3 that the TOC references.**

This is example A combined with a tier-3 feedback iteration. The heading paragraph re-composes locally. The anchor for that heading carries the new text. The TOC has a `TextRef(ch3_heading)` field whose resolved text just changed. Field diff fires; the TOC story re-enters tier 2 and re-flows. Pages with the TOC are marked dirty and re-render.

Latency for the heading change to appear: one frame. Latency for the TOC to reflect the new text: 100–200 ms in the background.

**Example D: User resizes a frame in a threaded chain.**

The frame's geometry changes. Tier 2 sees that the frame's capacity changed; composition of the affected story resumes from the start of the frame (or from a checkpoint before, if the change in capacity affects line breaks across the boundary). Story content reflows through the chain — content earlier in the chain stays in place (it's bounded by earlier frames' geometry which is unchanged), content from the resized frame onward shifts forward or backward. If content was overset and the resize created more room, previously hidden content appears. Tier 3 runs because page positions of downstream content may have changed. Field cascade as needed.

The user sees the immediate frame the resize affects update within one frame (synchronous local re-compose on the main thread); downstream pages settle in the background.

### 6.3 Latency budget allocation

For typing-class operations, the budget is one frame for visible feedback. The synchronous main-thread composer is the mechanism. Its scope is intentionally narrow: it composes one paragraph at a time, with no field resolution, no cross-story dependencies, no master page logic. Its output is a single dirty rectangle on the active page, blitted directly to the canvas.

For mutation-class operations (anything that changes frame geometry, anything that shifts pagination), the budget is one frame for the *local* visible effect (the dragged frame snaps to its new position) and "soon" for downstream settling. The user can continue editing while downstream work proceeds; they only block if they try to interact with content whose layout hasn't settled, which is rare.

For navigation-class operations (page jump, zoom transition), the budget is the animation duration (200 ms). The LOD cache absorbs almost all of the work: the navigator displays snapshots instantly, mid-res tiles arrive within the animation, live tiles arrive after settling.

---

## 7. The dependency graph and salsa

*Status note (2026-10-02): this section predates the implementation; salsa was not adopted. See [ADR 107](../adr/107-whole-document-build.md) (layout is a whole-document build) and [ADR 027](../adr/027-incremental-flow-invalidation.md) (the incremental invalidation built on top of it).*

### 7.1 Why a query system

The four-tier pipeline is naturally expressed as a graph of derived values:

```
content (inputs)
  → laid-out stories (per story)
    → frame placements (per frame chain)
      → page contents (per page)
        → numbering map (single global value with per-page entries)
          → resolved field values (per field)
            → final display list (per page)
              → LOD cache entries (per page per tier)
```

Each node depends on a specific subset of upstream nodes. A mutation changes inputs; the system needs to recompute the minimum subset of dependent nodes.

The naive implementation is invalidation flags: each upstream node has a list of dependents; mutating it marks them stale; reading a stale node recomputes. This is correct but fragile. The dependent lists must be maintained as the graph evolves (new pages, new fields, new computed stories add nodes). Cross-cutting concerns (style changes affecting many stories) require either explicit dependent registration or coarse invalidation. Bugs are hard to find because they manifest as stale-data correctness issues, not crashes.

The principled alternative is **demand-driven memoization**: every derived value is a function of inputs that is automatically tracked. The framework intercepts reads, records which inputs were read by which computation, and on subsequent reads either returns the memoized result (if no inputs changed) or recomputes (if any did). The `salsa` crate (used by rust-analyzer) is the mature implementation of this for Rust.

### 7.2 Recommended adoption pattern

The canvas's worker-side data model is modeled as a salsa database. Inputs are: the parsed IDML container, font and asset resolution results, and the mutation log. Queries are everything else.

Specifically:

**Input queries** (set by mutation handlers, never computed):
- `story_content(story_id) -> StoryContent`
- `frame_geometry(frame_id) -> Geometry`
- `frame_chain(chain_id) -> FrameChain`
- `style_definition(style_id) -> Style`
- `master_page(master_id) -> Master`
- `computed_story_definition(story_id) -> ComputedStoryDef`
- `font_bytes(font_id) -> FontBytes`

**Derived queries** (computed lazily, memoized):
- `laid_out_story(story_id) -> LaidOutStory` — depends on story content, frame chain, font, styles
- `frame_placements(chain_id) -> Vec<FramePlacement>` — depends on laid_out_story
- `page_content(page_id) -> Vec<FrameOnPage>` — depends on all frame placements with frames on this page, plus master content
- `numbering_map() -> NumberingMap` — depends on every page_content
- `resolved_field(field_id) -> String` — depends on the field's target lookup in numbering_map or content
- `computed_story_content(story_id) -> StoryContent` — depends on numbering_map and the anchors it references; this creates the feedback loop
- `display_list(page_id) -> DisplayList` — depends on page_content and resolved_fields visible on that page
- `snapshot_bitmap(page_id) -> Bitmap` — depends on display_list
- `mid_res_bitmap(page_id) -> Bitmap` — depends on display_list
- `tile(page_id, zoom_band, tx, ty) -> Tile` — depends on display_list

When the user types, the mutation handler calls `set_story_content(story_id, new_content)`. Salsa marks every query that depended on that input as potentially stale. The next time the renderer asks for `display_list(active_page_id)`, salsa walks the dependency chain: laid-out story (re-runs because its input changed), frame placements (re-runs because its dependency changed; but if the result is the same it short-circuits), page content (re-runs only if frame placements differ), numbering map (re-runs only if page content differs), and so on. The "re-runs only if its input differs from cached" check is salsa's killer feature: it lets large swaths of the graph skip re-computation when an upstream change doesn't actually propagate.

Computed stories (TOC, index) are queries that read both inputs and other queries (specifically numbering_map and anchor content). This is allowed in salsa and creates a natural fixpoint computation: the system iterates until queries stop returning new values. Salsa handles cycles by returning the previous value on the second iteration, which is exactly the convergence semantics needed.

### 7.3 What salsa does not do

Salsa memoizes Rust function calls. It does not interact with the GPU; tile bitmaps are produced by the renderer outside salsa and cached in their own LRU structure with salsa-derived keys. It does not handle long-running incremental computation within a single query — the Knuth-Plass composer with checkpoints is implemented inside the `laid_out_story` query and salsa sees only its final result. It does not handle the main thread; the camera transform is not a salsa input because it's not in the salsa database.

### 7.4 Migration path

The salsa adoption is gradual. Initial implementation can use straightforward hand-written invalidation; salsa can be retrofitted once the dependency graph is stable. The architecture should be designed so this retrofit is mechanical: every derived value is already a pure function of identified inputs, named with an ID, and stored in a flat container. Replacing the container with a salsa database is then a matter of annotation.

---

## 8. The viewport and navigation

### 8.1 Camera model

The camera is a single 2D affine transform from document space (point units, origin at document top-left) to viewport space (pixels). Three parameters: `scale` (pixels per point), `tx`, `ty` (viewport position of document origin).

All viewport operations derive from the camera:

- **Pan**: increment `tx`, `ty`
- **Zoom**: multiply `scale`, adjust `tx`, `ty` to keep the cursor point fixed in document space
- **Fit to page**: compute the scale that fits the page bounding box plus margin into the viewport; center the page
- **Fit to selection**: same with selection bounding box
- **Page jump**: animate from current camera to fit-to-page target over 200 ms with cubic ease-out

The camera is the single source of truth. The worker reads it via `SharedArrayBuffer` on every frame. The overlay layer reads it via the same buffer and applies it as a CSS matrix transform.

### 8.2 Input pipeline

Input handling is on the main thread, using `PointerEvent` exclusively. No legacy mouse events.

- **Wheel** → zoom-to-cursor. The point under the cursor in document space remains fixed.
- **Pinch (touch / trackpad)** → same as wheel.
- **Pointer drag with no modifier on empty canvas** → pan
- **Pointer drag with modifier or on selectable content** → selection / drag depending on context
- **Keyboard**: `Page Up`/`Page Down` for page-at-a-time navigation; `Ctrl/Cmd+G` for go-to-page; arrow keys with modifier for fine pan; `Ctrl/Cmd+0` for fit-to-page; `Ctrl/Cmd+1` for 100% zoom; `Ctrl/Cmd++/-` for zoom in/out.

Input events are processed synchronously and update the camera (via `SharedArrayBuffer`) and the overlay (via CSS transform) within the same frame. The worker reads the new camera on its next frame.

### 8.3 Inertia and animation

Pan continues with inertia after the pointer is released (momentum scrolling). Zoom transitions are instantaneous from input but page jumps animate. Animation runs on the main thread driving the camera; the worker re-renders each frame at the current camera.

### 8.4 Multi-page layouts

The document is presented as a vertical (or horizontal, configurable) flow of spreads with configurable inter-page spacing. The renderer treats this as one large document-space canvas; each page occupies its own rectangle in document space. Spreads (facing pages) are positioned side by side at their natural facing-pages geometry.

At overview zoom (e.g., showing 20+ pages at once), the snapshot tier is what's visible. At page-fit zoom, the mid-res tier. At close zoom, live tiles. Transitions happen continuously as the user zooms.

### 8.5 Page navigator

A separate UI panel (sibling to the canvas) shows a vertical scroll of page thumbnails. The thumbnails are taken directly from the snapshot atlas — no separate rendering. Clicking a thumbnail issues a page-jump animation. The navigator's scroll position is independent of the canvas's scroll position so the user can browse pages while keeping their work in view.

### 8.6 Minimap

For documents where the navigator's vertical extent isn't enough to be useful (a 500-page document at 100 px per thumbnail is 50,000 px tall), a minimap view shows the entire document as a grid of micro-thumbnails. Same atlas backing.

---

## 9. The overlay layer

### 9.1 Purpose

The overlay layer is HTML/SVG positioned above the canvas. It draws everything that is *not* document content:

- Selection chrome: transform handles on selected frames, marching ants around selected text ranges
- Text caret (blinking)
- Hover halos on hoverable objects
- Snap guides during drag
- Rulers and measurements
- Marquee (rubber-band) selection rectangle
- Inspector pull-outs and tooltips
- Frame chain visualization (lines connecting linked frames)
- Anchor marker visualization (pins on anchored objects)
- Comments, annotations (future)

### 9.2 Implementation

A single full-viewport `<div>` overlays the canvas with `pointer-events: auto` or `none` per child element. The overlay div has `transform: matrix(...)` applied with the camera, so its content is positioned in document space.

Individual chrome elements are rendered as SVG or HTML elements. Sizing is in CSS pixels, *not* document units — handles stay 12 px wide whether the document is at 5% or 800% zoom. To achieve this with a single CSS transform, each chrome element applies an inverse-scale at its own level: the overlay's transform is the camera, individual handles use `transform: scale(1/camera_scale)`. This keeps positioning in document space while making sizes constant in screen space.

Why HTML/SVG instead of a canvas-rendered overlay? Three reasons. First, accessibility — overlay elements are real DOM nodes that screen readers can find. Second, ease of styling and theming — overlay chrome inherits the editor's design tokens directly. Third, ease of interaction — overlay elements have native event handling, focus, drag-and-drop.

The cost is that overlay rendering is on the main thread. This is acceptable because overlay rendering is bounded — selection chrome is a few SVG elements, handles are a dozen, even rulers are bounded by viewport size. The main thread budget for overlay work is ~2 ms per frame.

### 9.3 Hit-testing integration

*Status note (2026-10-02): this section predates the implementation; the hit tester iterates candidate elements in the renderer's paint order and no R-tree is used: see [ADR 114](../adr/114-interaction-lives-in-the-engine.md), which also records selection geometry, gestures and snapping as built.*

Pointer events fire on overlay elements when they hit chrome (handle, caret, marquee). Pointer events fall through to the canvas div when they hit document content. The canvas div then issues a `HitTest` to the worker, which returns the content-level hit (story ID + offset, or frame ID, or page background).

The worker's hit-tester uses the spatial indexes built during layout: an R-tree of frames per page, and per-frame R-trees of glyph clusters and embedded objects. Hit-test of a point in a known page is sub-millisecond.

### 9.4 Selection rendering

Text selection rendering is a notable case. A selection is a content range `{story_id, start, end}`. Rendering it requires the geometry — a sequence of rectangles, one per visible line in the selection, in document space. The main thread requests this geometry from the worker; the worker computes from the laid-out story and returns a list of rects. The overlay renders them as semi-transparent rectangles.

The geometry is invalidated when the story's layout changes. The main thread re-requests on `StoryDirty` for the relevant story.

For very long selections (the user selects the entire document), the geometry could be huge. The worker returns only the rects intersecting the current viewport plus a margin, and re-computes as the user scrolls. This streaming approach is consistent with the "nothing in the viewport waits on anything off-viewport" principle.

---

## 10. IDML feature coverage roadmap

*Status note (2026-10-02): this roadmap predates the implementation; see `../status.md` for what ships.*

The canvas accommodates IDML features as the renderer adds them. This section is the explicit list of what the canvas must support and the order it must support it. Detailed renderer-side scoping is in the original design spec.

### Phase 1 — Viewer foundation (must support)

Pages, spreads, master pages. Frames (rectangle, text frame, graphic frame). Text content with character and paragraph styles, alignment, basic spacing. Threaded text through frame chains within a single page. Basic colors (CMYK and RGB to linear RGB). Strokes and fills on geometric primitives. Multi-page documents up to 100 pages.

### Phase 2 — Cross-page content

Threaded text across pages. Footnotes (per-page restart, single-line in this phase). Page numbers in masters via `Variable(page_number)`. Running headers driven by paragraph style. Cross-references and `PageRef` fields. Computed stories (TOC) without index. Up to 500 pages.

### Phase 3 — Editor foundation

Mutation API for text and styles. Single-character typing latency target. Style propagation. Text selection and caret. Frame manipulation: move, resize, rotate. Frame chain manipulation: link, unlink.

### Phase 4 — Advanced typography

OpenType features (kerning, ligatures, alternates). Hyphenation. Justification with Knuth-Plass refinement. Drop caps. Nested character styles. Tab leaders.

### Phase 5 — Advanced cross-page

Indexes (computed stories with index marker collection). Long footnotes that span pages. Tables (the largest single piece of work in this list — tables are themselves a sub-pipeline). Anchored frames with custom positioning rules. Conditional text.

### Phase 6 — Effects and color

ICC color management end-to-end. Transparency, blend modes, drop shadows, feathering, glow, gradients. Spot colors. Image effects.

### Phase 7 — Internationalization

CJK composition. Right-to-left text. Vertical text. Bidirectional algorithm. International page-numbering formats.

The canvas architecture does not change between these phases. Each phase adds new content types, new field kinds, new style attributes, or new tier-2 composition rules — all of which fit into the existing pipeline structure.

---

## 11. Acceptance criteria

This section is the gating list. Each criterion has a definition-of-done that can be objectively measured.

### 11.1 Phase 1 — Viewer foundation acceptance

**AC-V-1: Initial load performance.** A 100-page IDML document parses and produces a first viewable frame within 1.5 seconds on the reference hardware. Measurement: cold load with cleared cache, instrumented timer from `LoadDocument` call to first canvas paint.

**AC-V-2: Pan and zoom performance.** Continuous pan and zoom maintain ≥ 55 fps on the reference hardware while showing arbitrary regions of a 100-page document. Measurement: instrumented frame timer over a scripted pan-zoom sequence of 30 seconds.

**AC-V-3: Memory bound.** A 500-page document at idle (viewport showing 1–2 pages) uses < 400 MB of process memory. Measurement: Chrome DevTools memory snapshot.

**AC-V-4: LOD coverage.** Every page in the document is always renderable at some quality level instantly (snapshot tier never evicted). Measurement: random navigation to 50 pages in a 500-page document; every navigation produces a visible page within 50 ms.

**AC-V-5: Rendering correctness.** Pixel-faithful rendering matches the existing renderer's `paged-diff` criteria (mean ΔE ≤ 1.0, p99 ΔE ≤ 2.5, SSIM ≥ 0.99) on the seed corpus. Measurement: automated test using `paged-fidelity`.

**AC-V-6: Spatial index correctness.** Hit-testing any document-space point returns the correct frame and (for text content) the correct story and offset on a corpus of 50 test documents with known geometry. Measurement: automated test.

**AC-V-7: Threading isolation.** The main thread performs no WebGPU calls and no glyph layout. Measurement: instrumented main-thread profile during a 30-second pan-zoom session shows zero GPU calls and zero `paged-text` invocations.

**AC-V-8: Camera latency.** Camera updates propagate from the main thread to the worker's render loop within one frame (16 ms). Measurement: instrumented latency from input event to canvas paint.

### 11.2 Phase 2 — Cross-page content acceptance

**AC-X-1: Threaded text correctness.** A document with a story chained across 10 frames on 10 pages renders the story content continuously, with no duplicated or skipped text. Measurement: text extraction from rendered output matches the source story content character-for-character.

**AC-X-2: Footnote co-decision.** A document with footnotes where a footnote pushes the body line containing its anchor to the next page renders correctly: the line and its footnote both appear on the next page; the previous page's footnote slot contains only the previously-anchored footnotes. Measurement: 10-document test set with known footnote-pushing cases.

**AC-X-3: TOC correctness.** A document with a TOC generated by the IDML feature renders the TOC with correct text and correct page numbers, matching the InDesign-rendered reference. Measurement: TOC text and page numbers extracted from rendered output match reference.

**AC-X-4: Cross-reference correctness.** Every `PageRef` and `TextRef` field in a test document resolves to the correct page or text. Measurement: 10-document test set, every field's resolved value compared against reference.

**AC-X-5: Resolution convergence.** The resolution feedback loop converges within 3 iterations on all test documents. Measurement: instrumented iteration counter, asserted ≤ 3 on a 50-document corpus.

**AC-X-6: 500-page performance.** A 500-page document renders, navigates, and resolves all fields with all Phase 2 features active in under 4 seconds first-paint and < 700 MB memory at idle. Measurement: automated performance test.

### 11.3 Phase 3 — Editor foundation acceptance

**AC-E-1: Single-character typing latency.** Typing a character in a body paragraph of a 500-page document produces a visible glyph within 32 ms (one frame at 30 fps, half a frame at 60 fps). Measurement: instrumented latency from keystroke event to canvas paint.

**AC-E-2: Incremental composition.** Typing a character causes Knuth-Plass to re-compose at most 3 paragraphs. Measurement: instrumented composition counter on a typing test.

**AC-E-3: TOC update latency.** Editing a heading whose text appears in the TOC causes the TOC entry to update within 500 ms. Measurement: instrumented latency from mutation to display list invalidation for the TOC page.

**AC-E-4: Selection geometry.** Selecting text ranges of various sizes (single character, single line, multi-paragraph, full-page, multi-page) renders selection chrome correctly. Measurement: visual regression test on 20 selection scenarios.

**AC-E-5: Caret positioning.** The caret is positioned correctly within text at every zoom level from 5% to 800%, including positions where the cursor sits between sub-pixel glyph clusters. Measurement: visual regression test.

**AC-E-6: Frame manipulation.** Moving, resizing, and rotating frames produces correct geometry in the document model and correct visual feedback. Measurement: automated test asserting post-mutation document geometry.

**AC-E-7: Mutation determinism.** Replaying a recorded mutation log against an initial document produces byte-identical derived state across runs. Measurement: snapshot hash comparison.

**AC-E-8: Undo correctness.** Undoing a sequence of mutations and re-applying them produces the same final state as the original sequence. Measurement: automated test.

**AC-E-9: Zoom-independence of operations.** Performing identical mutations at zoom levels of 5%, 100%, and 800% produces identical document state. Measurement: automated test with three zoom-level runs against the same mutation sequence.

### 11.4 Cross-cutting acceptance

**AC-C-1: API stability.** The worker API surface is documented, versioned, and stable for all consumers (viewer, inspector, future editor). Breaking changes require explicit version bump and migration notes. Measurement: API documentation exists, automated test enforces backward compatibility within a major version.

**AC-C-2: Test coverage.** Unit tests cover ≥ 80% of canvas-specific code (excluding the underlying renderer crates). Integration tests cover every acceptance criterion above. Measurement: coverage reports, test count.

**AC-C-3: Performance regression gate.** CI runs the performance test suite against a reference document corpus and fails the build if any criterion regresses by > 10%. Measurement: CI configuration.

**AC-C-4: Memory leak gate.** A 30-minute scripted session of editing, navigation, and zooming produces no growth in process memory beyond the LOD cache budget. Measurement: long-running test with memory snapshots at start and end.

**AC-C-5: Crash recovery.** A simulated worker crash mid-session is recovered transparently by spawning a new worker and replaying the mutation log; the user's session continues without data loss. Measurement: automated test.

---

## 12. Risks and mitigations

### 12.1 Incremental Knuth-Plass is the long pole

The biggest technical risk is incremental composition. Knuth-Plass is naturally a whole-paragraph (and sometimes multi-paragraph, with `keep-with-next`) global optimization; making it incremental with correct checkpointing requires careful analysis of where stable boundaries exist. Implementation may discover that certain document patterns (heavy widow-orphan constraints, dense `keep-with-next` chains) defeat checkpointing and force whole-story recomposition.

*Mitigation.* Build a benchmark suite of real-world documents early. Profile incremental composition against the full re-compose baseline. If certain patterns can't be checkpointed, fall back to whole-story re-compose for those stories — single-character latency degrades but only for stories with the pathological pattern, which are typically structural pages (cover, TOC) not body content.

### 12.2 Resolution convergence

The feedback loop between content and resolution could in principle fail to converge. A TOC that grows enough to push pages onto more pages whose numbers then grow the TOC further is a real (if rare) scenario. The system caps at 4 iterations to prevent infinite loops, but the cap fires as a warning the user must address.

*Mitigation.* The convergence cap surfaces to the inspector with diagnostic information ("the TOC's length is unstable; consider fixing its page count manually"). For automated environments, the cap is a hard error. In practice, documents that exhibit this pattern are also problematic in InDesign and require manual TOC updates.

### 12.3 Worker termination and recovery

A WASM panic, a GPU device loss, or an OOM crash in the worker terminates rendering. The main thread must recover gracefully.

*Mitigation.* All worker state is deterministic from the mutation log. Spawning a new worker and replaying produces the same state. The main thread maintains the mutation log; the worker is stateless except for caches. Recovery is automatic for transient errors and surfaces to the user only for persistent errors (e.g., document is too large for available memory).

### 12.4 Browser API maturity

WebGPU is mature but recent. `OffscreenCanvas` support is widespread but has edge cases on Firefox and Safari. `SharedArrayBuffer` requires cross-origin isolation headers.

*Mitigation.* The architecture is browser-agnostic in principle. Concrete browser compatibility is gated by the deployment environment. For environments where `SharedArrayBuffer` is not available, the camera uses `postMessage` with structured cloning; latency degrades but functionality is preserved. For browsers without `OffscreenCanvas`, the worker can be eliminated and rendering moves to the main thread; performance degrades but functionality is preserved.

### 12.5 Memory growth from caches

Three LOD tiers with explicit budgets are the design defense, but real-world usage may produce pathological cache patterns (e.g., a long scroll through a 1000-page document loading the mid-res tier for every page).

*Mitigation.* Budgets are configurable. Eviction is observable in the developer HUD. Long-running session tests catch memory growth in CI. Worst case: aggressive eviction degrades visual quality (snapshots used where mid-res would be ideal) but never crashes.

### 12.6 Complexity creep

The architecture has many moving parts. As IDML features land in later phases (effects, tables, internationalization), the temptation will be to add new tiers or new fast paths. Each addition risks the integrity of the four-tier model.

*Mitigation.* The four-tier model is the architecture contract. New IDML features add new node types in tier 1, new composition rules in tier 2, new field kinds in tier 3, or new display commands in tier 4 — but never new tiers and never cross-tier fast paths. Every PR is reviewed against this contract.

---

## 13. Open questions

*Status note (2026-10-02): this section predates the implementation; OQ-1 and OQ-3 presuppose salsa, which was not adopted ([ADR 107](../adr/107-whole-document-build.md)). OQ-6 was decided: the host registers fonts and colour profiles, see [ADR 109](../adr/109-engine-does-no-io.md).*

These are decisions deferred to implementation but flagged here for explicit resolution before the relevant phase begins.

**OQ-1: Salsa adoption timing.** Should salsa be introduced in Phase 2 (when the dependency graph first becomes non-trivial) or Phase 3 (when editor mutations make incrementality critical)? Argument for earlier: building on salsa from the start avoids a retrofit. Argument for later: Phase 1 doesn't need it and the API may evolve in ways that make the retrofit cheap.

**OQ-2: Footnote span policy.** Long footnotes that span pages are rare but legal. Are they supported in Phase 2, or deferred to Phase 5?

**OQ-3: Master page edits.** Editing a master page propagates to every page using that master. Is the propagation modeled as a salsa input change (clean) or as a bulk invalidation (faster but less principled)?

**OQ-4: Synchronous main-thread composer.** The fast-path for single-character typing requires a stripped composer on the main thread. How much of `paged-text` does this composer share? Options: link `paged-text` into both the main thread WASM and the worker WASM (simple but doubles wasm size), or extract a minimal "single paragraph composer" crate (cleaner but adds maintenance burden).

**OQ-5: Tile sizing strategy.** Live tile size affects cache granularity and GPU draw efficiency. Fixed-size tiles (e.g., 256×256) are simple. Page-aligned tiles (one tile per page) are simpler still but waste memory for partially-visible pages. Pick during Phase 1.

**OQ-6: Asset resolution.** Fonts, embedded images, and ICC profiles need an async resolver. Is the resolver injected by the host application, or does the canvas own it?

**OQ-7: Selection persistence across mutations.** When a mutation deletes content overlapping a selection, what happens to the selection? Options: collapse to the deletion point, preserve the unaffected range, clear the selection. InDesign behavior is to preserve the unaffected range; this is the proposed default.

---

## 14. Glossary

**Anchor.** A named position within a story that other content can reference. Headings, footnote markers, and cross-reference targets all produce anchors.

**Camera.** The 2D affine transform from document space to viewport space.

**Checkpoint.** A saved layout state at a paragraph boundary that allows incremental composition to resume without re-running from the start of the story.

**Computed story.** A story whose content is derived from other content (e.g., TOC, index).

**Display list.** A per-page sequence of render commands consumed by the GPU.

**Document space.** The coordinate system in which document content is positioned. Units are PostScript points (1/72 inch).

**Field.** A placeholder within a text run that gets resolved during the resolution pass (page reference, text reference, variable, autonumber).

**Frame.** A presentation surface with geometric extent that holds part of a story.

**Frame chain.** An ordered list of frames that together hold one story. Threaded text occupies a chain.

**Knuth-Plass.** The optimal paragraph composition algorithm used by `paged-text`.

**LOD cache.** The three-tier cache (snapshot, mid-resolution bitmap, live tiles) that drives the viewport.

**Numbering map.** The dictionary keyed by anchor ID containing all numeric facts about that anchor's position.

**Overlay layer.** The HTML/SVG layer above the canvas that draws non-content chrome (handles, caret, guides).

**Resolution pass.** The tier-3 pass that produces the numbering map and resolves fields.

**Salsa.** The Rust demand-driven memoization framework recommended for the dependency graph.

**Snapshot.** A low-resolution rendered thumbnail of a page, never evicted.

**Story.** A unit of authored content; a sequence of paragraphs and runs.

**Tier.** One of the four levels of the pipeline (content, layout, resolution, output).

**Viewport space.** The coordinate system of the visible canvas, in CSS pixels.

---

*End of document.*