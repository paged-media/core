# The composition format (`document.pgd`) — design spine

2026-07-19. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

The core-owned **composition** part of
the [`.paged` container](paged-file-format.md) — one of the two new formats Paged authors (the
other is the publishing format). It formalizes the composition model sketched in an
internal design note into an actual on-disk format. It **descends from today's
`paged-scene` arrangement concepts** (spreads/pages/frames/masters/styles), cleaned per an
internal notebook of misshapen concepts and decoupled from IDML content. Load-bearing
*shape* only — the exact fields evolve behind it ([ADR 021](../adr/021-paged-native-document-model-idml-as-format.md) "get the architecture right; evolve the
details").

*Status note (2026-10-02): this document is partly implemented. `crates/paged-composition/src/lib.rs` implements the arrangement kernel (surfaces, flows, pages, the region/group/layer tree, the position kinds, JSON serialization) and `crates/paged-flow/src/lib.rs` implements the flow protocol of §5. Templates, instances and slots (§7) and shared-resource handles (§8) are not built. The composition is a projection derived from the document model (`Document::to_composition` in `crates/paged-scene/src/lib.rs`), not the model the renderer builds from. Its container path is `paged/core/composition/document.pgd`; `CanvasModel::refresh_composition_part` (`crates/paged-canvas/src/model.rs`) writes the part, and no wire operation calls that method. The sections below carry a note where the code differs.*

## 0. What the composition is — and is *not*

The composition is **the arrangement**: how content is placed, ordered, flowed, constrained, and
projected onto surfaces. It is the one thing no content part can own.

It **holds:** surfaces, pages/spreads, **regions** (the placement unit), region-chains (flow),
positioning constraints, layers/z-order/blend, part↔region bindings, template/instances,
shared-resource references, and surface/condition visibility.

It **does NOT hold** content: no text runs, no cell values, no vector geometry, no pixels, no
formulas. Those live in the content parts (`content.pgp`, web, sheet, data, draw, image). The
composition references them by typed handle + selector. This boundary is the format's central
invariant — it is what keeps the composition small, keeps content engines independent, and makes the
C1 test ("hand-build a composition with a native part, no IDML, render it") pass.

## 1. Serialization — JSON, diffable, inspectable

*Status note (2026-10-02): this section predates the implementation; see `Composition` in `crates/paged-composition/src/lib.rs`, which carries `format`, `version`, `capabilities`, `surfaces`, `flows`, `pages` and `nodes`. `resources`, `templates` and a page's `master` are not implemented, and a flow names its part and its selector in two fields.*

`document.pgd` is a **JSON** document (aligning with [`paged-file-format.md`](paged-file-format.md) §2's open/inspectable/diffable
principle and the `spec`-role convention). No bespoke binary. A composition is a small tree of typed
nodes with stable ids; it is greppable, version-controllable, and repairable by hand. (A derived
binary fast-load cache is possible later, never the source of truth — [`paged-file-format.md`](paged-file-format.md) §9.)

```jsonc
{
  "format": "paged-composition",
  "version": 1,                       // producer.version
  "capabilities": ["flow.regionChain@1", "surface.print@1"],
  "resources": { "colors": "…", "styles": "…", "fonts": "…" }, // handles into paged/core/resources
  "surfaces": [ { "id": "print", "kind": "print" }, { "id": "screen", "kind": "screen" } ],
  "flows":    [ { "id": "f1", "part": "publishing:story/s7" } ],   // FlowId → a part's content
  "pages":    [ { "id": "p1", "spread": "sp1", "size": [...], "master": { "template": "t1", "overrides": {…} } } ],
  "nodes":    [ /* the region/group tree — see §3 */ ],
  "templates":[ { "id": "t1", "root": "…" } ]
}
```

## 2. The node model — one generic typed tree

*Status note (2026-10-02): this section predates the implementation; the `Node` enum in `crates/paged-composition/src/lib.rs` has `Region`, `Group` and `Layer`. `Guide` and `Grid` are not implemented, and a `Group` carries an id and children only.*

Nodes form a tree (`{ id, kind, props, children }`), stable-id'd. `kind` is an open enum; the
composition's kinds are all *arrangement* kinds (never content kinds):

- **`Region`** — the placement unit (§3). Binds a content part into a geometry.
- **`Group`** — grouping + a shared transform/clip.
- **`Layer`** — a stacking + visibility/lock/print band (replaces IDML's `<Layer>`).
- **`Guide` / `Grid`** — layout scaffolding for constraints.

Geometric primitives that are *pure composition decoration with no content part* (a plain ruled line,
a background swatch rectangle) are also `Region`s — with a content binding of `none` and a fill/stroke
from the resource graph. Anything with real content (text, a table, a drawing) binds a part.

## 3. `Region` — the placement unit

*Status note (2026-10-02): this section predates the implementation; `Region` in `crates/paged-composition/src/lib.rs` carries `id`, `bind`, `position`, `geometry`, `layer`, `flow` and `visibleOn`. Its geometry is a rectangular content box (`RegionGeometry` in `crates/paged-flow/src/lib.rs`: width, height, columns, column gap). `transform`, `clip`, `blend`, `opacity`, `overrides` and non-rectangular outlines are not implemented, and the container manifest records no dependency between parts.*

A region places (a selector into) a content part into a geometry on a surface. Its props:

- **`bind`** — `{ part: "<part-ref>", selector: "<engine-specific>" } | "none"`. E.g.
  `{ part: "publishing", selector: "story/s7" }`, `{ part: "media.paged.web/o3", selector: "flow:main" }`,
  `{ part: "media.paged.sheet/o1", selector: "Sheet1!A1:D20" }`. The **selector is opaque to the
  composition** — the content engine interprets it. `none` = a decoration-only region.
- **`position`** — a **positioning constraint** (§4), not raw bounds.
- **`geometry`** — the region's shape (a rect, or a `Reference<vector>` outline for a non-rect frame),
  content-box insets, columns.
- **`layer`** — which `Layer` (z-band); plus a local z within it.
- **`transform` / `clip` / `blend` / `opacity`** — the compositing controls core applies when it
  draws the region's `SceneLayer` (the compositor proper).
- **`flow`** — optional `FlowId`: this region is a link in a region-chain (§5).
- **`visibleOn`** — a set of surfaces/conditions (§6); default: all surfaces.
- **`overrides`** — view-local presentation overrides applied to the bound content in *this* region
  (the "frame-local override", kept minimal).

A region never contains content; it *windows* a part. Two regions may bind the *same* part with
different selectors/geometry (the same dataset placed twice with two chart views — [`paged-file-format.md`](paged-file-format.md)
§5's definition-vs-placement, generalized).

**Binding-location rule:** a `bind` is *placement* (arrangement — belongs here). A
*content↔content* dependency — a sheet table whose rows derive from a data part — is *content
semantics* and lives in the **content part** (the sheet part records `{ source:"data/ds1", … }`), NOT
in the composition. The composition only places the region; the **container manifest records the
inter-part dependency** (`deriveFrom` across parts) for staleness + open-order. Rule
of thumb: a *position* (`Anchor`) is composition; a *derivation* is part-internal.

## 4. Positioning — one constraint system

*Status note (2026-10-02): this section predates the implementation; the six kinds exist as the `Position` enum in `crates/paged-composition/src/lib.rs`. Only `PageRelative` is resolved to geometry, in `crates/paged-composition-render/src/lib.rs`; the layout pass that resolves the other kinds is not built.*

`position` is one of a single constraint family — never a separate model per case:

- `PageRelative { page, at }` · `SpreadRelative { spread, at }` · `FrameRelative { region, edge, offset }`
- `GridCell { grid, row, col, span }`
- `Anchor { part, at: <selector-position> }` — anchored-to-content (a figure anchored mid-story):
  the region's origin is a *position inside another part's content*. This is the unification —
  the anchored object is one constraint kind, resolved by the same layout engine, not a special model.
- `ViewportRelative { edge, offset }` — for screen surfaces.

One layout pass resolves all constraint kinds into concrete geometry per surface.

## 5. Flow — region-chains (the FlowId protocol, content-agnostic)

*Status note (2026-10-02): implemented in `crates/paged-flow/src/lib.rs` (`FlowId`, `RegionChain`, `FlowContent`, `run_flow`, `Overset`). `crates/paged-renderer/src/flow.rs` implements the protocol for shaped text; the renderer's story emitter does not run through `run_flow` and shares the `region_overflows` rule with it. Growing a chain when content is overset is recorded in [ADR 026](../adr/026-auto-growing-region-chains.md).*

A **flow** (`{ id, part }`) names a content sequence in a part that fragments across an **ordered set
of regions** (each region tagged with that `flow` id). The composition owns the *chain + order +
overset*; the content engine owns *the content and how it fragments* (composition owns the
views/region-chain; the part owns the story/content).

- **Content-agnostic:** the same protocol threads an IDML/publishing story, an HTML flow, or a long
  table across frames — it is exactly [ADR 020](https://github.com/paged-media/plugin-web/blob/main/docs/adr/020-paged-web-native-engine-defer-frame-threading.md)'s `FlowId` and the paged.web fragmentation work,
  generalized. The engine is asked "fragment `part:selector` across these region geometries, in this
  order" and returns per-region content + overset.
- **Overset** (content past the last region) is a first-class composition state, surfaced (not hidden).
- This is the format's **first executable seam**: flows + regions are the
  smallest slice that proves composition-owns-arrangement / part-owns-content.

## 6. Surfaces & conditions

*Status note (2026-10-02): this section predates the implementation; `Surface`, `SurfaceKind` (`Print`, `Screen`) and a region's `visibleOn` exist as data in `crates/paged-composition/src/lib.rs`. No code reads `visibleOn`, and props do not vary by surface.*

- **`surfaces`** — a `SurfaceSet` (`print`, `screen`, …). Exactly one is the **print projection** —
  the deterministic, CMYK-exact surface the fidelity gate + PDF/IDML export target. Multi-surface never
  perturbs it.
- A region's **`visibleOn`** restricts it to surfaces/conditions; a `screen`-only interactive web
  region is simply absent from the print projection. Props may also vary by surface.
- **`printFallback` is not a new field:** a screen-only region plus a *print-only* region
  (`visibleOn:["print"]`, e.g. a static image) over the same geometry gives a print fallback with the
  existing `visibleOn` mechanism — no new primitive.

## 7. Templates, instances & slots

*Status note (2026-10-02): not implemented; `crates/paged-composition/src/lib.rs` has no template, instance or slot types.*

- A **template** (`{ id, slots?, root }`) is a composition subtree (a page template, a repeating
  component, the master-page chrome) that may declare named **slots** (content parameters).
- A template `Region` may `bind` a **slot** (`{ slot: "hero" }`) instead of a concrete part-selector —
  a named placeholder for *content*.
- An **instance** references a template + **fills its slots** (`slots: { hero: {part,selector}, … }`)
  + an optional **override `DiffLayer`** (tracked property/child overrides for arrangement). Slots
  supply per-instance *content*; overrides supply per-instance *arrangement* divergence.
- Master pages, repeating product pages (a 40-page catalog — same layout, different product per
  page via slot-fills), and component instances are the *same* primitive. Editing a template repaints
  all instances except overridden nodes; "detach" materializes the diff. Replaces IDML's
  special-purpose master-spread + `OverriddenPageItemProps`. (Nested templates — a component instance
  inside a page template — are the same primitive recursively; unproven past one level.)

## 8. Shared resources — typed graph handles

*Status note (2026-10-02): not implemented; `Composition` in `crates/paged-composition/src/lib.rs` has no `resources` field, and no code reads or writes `paged/core/resources/`.*

`resources` are handles into `paged/core/resources/` — the **ColorGraph** and **StyleGraph** (typed
references + relationships/inheritance) shared across *all* parts, so a swatch or paragraph style is
one entity, not a per-format duplicate. A region's fill is a `Reference<Color>` into the graph; a
flow's base style is a `Reference<ParagraphStyle>`. This is where "one document, consistent styling"
across an IDML story and a web frame is won.

## 9. IDML ↔ composition (the adapter boundary)

*Status note (2026-10-02): this section predates the implementation; the import direction exists in part: `Document::to_composition` in `crates/paged-scene/src/lib.rs` maps pages and each story's frame chain (a flow plus text-frame regions with their content-box geometry); rectangles, groups, layers, master spreads, anchored objects and resource references are not mapped. The export direction is not built: the IDML package is written from the document model by `plugin-publish: crates/idml-export` ([ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md)).*

The composition is where the IDML *arrangement* maps in and out; IDML *content* maps to the publishing
part (separate spec).

**Import (`IDML → composition` + `IDML → publishing`):**

| IDML arrangement | Composition |
|---|---|
| `<Spread>` / `<Page>` | `pages` (+ `spread`) with size + master instance |
| `<TextFrame>` (+ `NextTextFrame` chain) | `Region`s over a `flow` bound to the publishing story |
| `<Rectangle>/<Oval>/<Polygon>/<GraphicLine>` | `Region`s (decoration or bound to a graphic part) |
| `<Group>` | `Group` node |
| anchored object | a `Region` with an `Anchor` constraint (§4) |
| `<MasterSpread>` + overrides | a `template` + instance `DiffLayer` (§7) |
| `<Layer>` | `Layer` node (§2) |
| swatch/style refs | `Reference<Color>`/`Reference<Style>` into the resource graph (§8) |
| geometry, transforms, columns | region `geometry`/`transform`/columns |

(The story/table/character content itself goes to the publishing part; provenance carry-through keeps
what neither model owns.)

**Export (derived IDML projection, `composition + parts → IDML`):** to keep `.paged` a valid IDML
package, the composition + publishing part *regenerate* the canonical IDML parts:
regions over a flow → threaded `<TextFrame>`s + a `<Story>`; templates → master spreads; non-print
parts → baked `<Image>`/PDF page-items (the derived flatten). Lossless for IDML-origin arrangement;
best-effort + diagnostic for composition features IDML can't hold (a `screen` surface, a live binding).

## 10. Relationship to the render pipeline (unchanged below)

*Status note (2026-10-02): this section predates the implementation; `crates/paged-composition-render/src/lib.rs` renders one page of a composition through a `RegionRenderer` (single page, `PageRelative` positions). The production pipeline does not go through it; it builds display lists from the document model ([ADR 107](../adr/107-whole-document-build.md)).*

The composition **drives the compositor**: for a target surface, resolve constraints → geometry; ask
each region's engine to render its `part:selector` into that geometry → a `SceneLayer` (per region, per
flow-fragment); the compositor arranges them by layer/z/clip/transform/blend (§3). PDF/print export =
the compositor over the print-projection SceneLayers. This is exactly today's `paged-compose` +
`SceneLayer` path ([ADR 013](../adr/013-in-frame-scenelayer.md)) — the composition format just makes the *arrangement* it composites
explicit and Paged-owned, instead of implicit in the IDML scene.

## 11. Versioning & capabilities

*Status note (2026-10-02): this section predates the implementation; `version` and `capabilities` are fields of `Composition` in `crates/paged-composition/src/lib.rs`. No code checks them when the part is read, and `minEngine` and the render-cache fallback are not implemented.*

`document.pgd` carries `version` (its `producer.version`) and declares the `capabilities` it uses
(`flow.regionChain@N`, `surface.screen@N`, `positioning.grid@N`, …) + contributes to the document's
`minEngine`. An older engine that lacks a declared capability renders the affected regions from the
**derived render-cache** and marks them view-only, preserving the composition on save. The capability vocabulary is defined once in the capability catalog ([ADR 019](../adr/019-capability-catalog-one-contract.md)).

## 12. Open questions

- **Selector grammar** — the composition holds selectors opaquely, but a *shared minimal* selector
  convention (`story/<id>`, `flow:<name>`, `<A1:range>`) helps tooling; how much is standardized vs
  per-engine.
- **Geometry for non-rect regions** — inline path vs `Reference<vector-part>`; how a clip/frame shape
  that is itself authored content (a draw path used as a text-wrap frame) is referenced.
- **Constraint solving order + cycles** — an `Anchor` into a part whose layout depends on the region
  size (a frame that grows to fit): the resolve order / fixpoint. (Ties to the flow re-resolve loop the
  paged.web fragmentation already does.)
- **Undo/collaboration identity** on composition nodes (out of scope for v1; stable ids are the enabler).
- **How much of the print-projection regeneration is deterministic** enough for the fidelity gate to
  run on the *derived* IDML (vs on the composition directly).

---

**Bottom line.** `document.pgd` is a small JSON tree of **arrangement** nodes — surfaces, pages,
**regions** that bind (a selector into) a content part into a positioned, layered, clipped geometry,
**region-chains** that flow a part's content across regions (the content-agnostic `FlowId`), a single
positioning-constraint system, template/instances, and typed shared-resource references. It holds *no
content* — content lives in parts, referenced by handle. It descends from `paged-scene`'s arrangement
concepts, cleaned, and drives the existing compositor unchanged. It is the
core-owned "native model," and the first code seam over it is the region-chain/flow protocol.
