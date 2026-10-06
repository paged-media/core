# ADR 021 — Paged-native document model; IDML becomes an import/export format

**2026-07-19 · decision record · status: ACCEPTED (ratified 2026-07-19); PLACEMENT CLAUSE AMENDED
2026-07-20 by [ADR-022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md).** The thesis (Paged-native model,
IDML demoted to import/export) stands; ADR-022 reverses this ADR's "IDML read/write stays core-resident
/ plugin-publish is NOT the home for IDML" clause now the three walls below are down (the self-ownership
work made the
native model authoritative + a new `host.nativeDocument` door + an adapter shared under the engine's
licence, see `LICENSE.md`). A forward-looking
ADR-as-memo (cf. [011](https://github.com/paged-media/plugin-web/blob/main/docs/adr/011-web-rendering-fork-defer-to-scenelayer.md)/[012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md)/[020](https://github.com/paged-media/plugin-web/blob/main/docs/adr/020-paged-web-native-engine-defer-frame-threading.md)): it keeps the ADR format but
argues a recommendation, paired with an internal reasoning memo on the document-model direction
("the memo" below). Records the disposition
of the question "should IDML stop being Paged's native document *model* and become an import/export
*format* over a Paged-native `.paged` model?" Ratified as a **direction** (not a scheduled task); the
first artifact is an internal document-model design ("the design doc" below),
then the executable sequence in Consequences.

**Sources:** the internal direction memo (the full reasoning);
`crates/paged-scene/src/lib.rs:44-72` (the `Document` = re-exported IDML AST — the model *is*
IDML); `crates/paged-mutate/src/operation.rs:66-171,182+` (the mutation vocabulary is IDML
nouns); `crates/paged-compose/src/{display_list.rs,scene_layer.rs}` (the format-neutral boundary
already exists, downstream at compose); the original renderer plan, §C1/§C2/§D (the parser↔scene-graph seam
discipline + the misshapen-concepts notebook + "Paged's 2026+ design"); an internal lessons notebook (the
notebook — 5 seeds, "new entries" empty); an internal proposal for a native HTML/CSS engine in the
core, §37 Q#7 + §41 (the
explicit format-neutral open question); **[ADR-020](https://github.com/paged-media/plugin-web/blob/main/docs/adr/020-paged-web-native-engine-defer-frame-threading.md)**
(the renderer-neutral `FlowId` seam "shared with IDML stories"); **[ADR-017](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/017-importer-exporter-door-shape.md)**
(the importer/exporter door — foreign formats, not the substrate); **[ADR-007](007-carry-through-rendering-honesty.md)**
(carry-through + honesty); the internal migration record (product-not-format
naming; embeddability); the plugin-draw concept paper, §7 ("documents degrade to their baked
IDML form"); `plugin-sdk: packages/plugin-api/src/{host.ts,editor.ts}` (the exporter door carries no
whole-document read).

## The decision

**Adopt as a direction: Paged gets its own document model, and IDML is demoted from the native model
to a first-class import/export adapter over it — evolutionarily, C1-seam-first, not a rewrite; IDML
read/write stays in the core; `plugin-publish` is NOT the home for IDML.**

> Paged's native document model becomes `.paged` (Paged-owned); IDML becomes a
> lossless-where-possible import/export adapter over it. The migration is **evolutionary** — the
> native model *descends from* `paged-scene` (Paged-owned types that *wrap*, not re-export, the parse
> AST; the C1 "hand-build a scene with no IDML" test enforced in CI), the mutation vocabulary
> migrates IDML-noun → native-noun incrementally, cleaning the lessons notebook's misshapen concepts as it
> goes. IDML read/write stays **core-resident** (embeddable). The round-trip promise is
> re-scoped and recorded in its own ADR. `plugin-publish` is for *foreign* formats, later.

This is not "move IDML out of core." IDML I/O stays. What moves is IDML's **status**: from *the
document model* (which limits what the platform can express) to *a format the platform reads and
writes* (which does not). It is the general form of ADR-020's renderer-neutral `FlowId` seam, and the
maturation of the renderer plan's C1/C2/D charter.

## Options weighed

| Option | Mechanism | Expresses rich content natively? | IDML round-trip | Embeddable IDML core | Risk |
|---|---|---|---|---|---|
| **X — status quo** | IDML object model stays the native model; plugins bake down to an IDML fallback + metadata. | **No** — web/sheet/data/charts are a rectangle-or-group + an opaque side-part. | Lossless (byte carry-through). | Yes. | The platform ceiling: every new capability grows the IDML shoe-horn; the model is the limiting factor. |
| **Y — IDML → `plugin-publish` plugin** | Relocate IDML/PDF I/O into a separate plugin. | No (no neutral model to lower from). | n/a. | **No** — IDML I/O leaves the core. | Three walls: no neutral model; the plugin API can't host whole-document export; breaks the "unreadable without plugins" guarantee. Rejected. |
| **Z — Paged-native model + IDML as adapter** *(recommended)* | A Paged-owned model grown out of `paged-scene`; IDML becomes `parse→native`/`native→IDML` adapters; `.paged` is the native container, IDML derived. | **Yes** — plugin content promotes to first-class as the model gains shape. | Lossless for IDML-origin; best-effort + honest diagnostics for native-only features (policy = its own ADR). | **Yes** — model + IDML I/O stay in the core. | Foundational change; must be evolutionary or it stalls (the real risk, mitigated by the guardrails). |

## Why Z, on the evidence

1. **The engine is already half-neutral.** The neutral boundary exists — but at `paged-compose`
   (display list + `SceneLayer`), not at IDML (`scene_layer.rs` is *designed* for non-IDML front-ends).
   The only IDML-shaped layer is the *document model* (`paged-scene`/`paged-mutate`). Z finishes a
   seam the architecture already has; it does not invent one.
2. **It was always the plan.** The renderer plan's §C1 required the scene graph to be *"designed for what
   comes next"* with a *"hand-build a scene, no IDML"* test; §C2's notebook was to be *"the spine of
   Paged's 2026+ design document."* The seam drifted (the parser leaked into `paged-scene`; the
   notebook lapsed). Z re-establishes the charter, it doesn't contradict it.
3. **The platform's growth is the trigger.** Five plugins each keep their real content in a `.paged`
   side-part because the IDML model can't hold it (sheets' workbook, web's HTML, data/charts). Five
   independent "the model can't express this" workarounds is the evidence that the model, not the
   plugins, is the ceiling (memo §4).
4. **It strengthens, not weakens, the guarantees.** "Documents degrade to their baked IDML" becomes
   "documents *are* natively rich; IDML export is the lossy interop" — a better story; and IDML stays
   core-resident and embeddable (memo §5).
5. **Continuity with ADR-020.** ADR-020 already ratified a renderer-neutral `FlowId` model *"shared
   with IDML stories"* as the *"single genuinely core-resident"* neutral concept. Z is that seam
   generalised from *flow* to the *whole document model*. Its first increment IS the ADR-020 `FlowId`.

The case against Z is honest and recorded: it puts the **lossless-IDML-round-trip promise** at risk
for native-authored content (memo §7), and a mishandled ("big-bang") execution is a project-killer.
Both are guardrail conditions, not reasons to reject the direction — but if a fully-lossless IDML
round-trip is a hard requirement, or there is no capacity for a long incremental
direction, the correct choice is **X + only the ADR-020 FlowId seam**, and this ADR should not be
ratified.

## Consequences

- **No code lands from this ADR.** It records a direction. The **next artifact is a design doc** —
  the "Paged document model" that the renderer plan's §C2/§D always pointed at, authored by the
  exemplar-first method, seeded from the lessons notebook + the five plugins' unmet needs. Design precedes
  code.
- **First concrete increment = ADR-020's `FlowId`/region/overset seam** (renderer-neutral, shared by
  IDML stories + web flows) — already blessed; it becomes the seed of the native model.
- **The C1 seam gets enforced in code + CI** (Paged-owned scene types wrapping the parse AST; the
  hand-build-without-IDML render test) so the model boundary cannot re-rot as it did.
- **The IDML round-trip policy is re-scoped in its own future ADR** — recommended: lossless for
  IDML-origin content (carry-through preserved via `paged-write`), best-effort + honest diagnostics
  (ADR-007) for native-only features. This is the single biggest product promise at stake.
- **`.paged` becomes the native-model container; IDML packages are a *derived* export**, not the
  substrate. Migration: `.paged` gains a native part → IDML parts become interop export → native part
  becomes source of truth.
- **Licensing invariant:** the native model **and** IDML I/O stay in the core
  (for licence terms see `LICENSE.md`; embeddability; the viewer stays the embeddable IDML read pipeline). Nothing IDML
  moves to a plugin.
- **`plugin-publish` is scoped to *foreign* publishing formats** (via the importer/exporter door,
  [ADR-017](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/017-importer-exporter-door-shape.md)) — and even that needs a new whole-document-read host
  door that does not exist today (`plugin-api` exporter carries no document; platform work, not a
  plugin reach-around). It is orthogonal to this decision.
- **This is a long-running direction, not a scheduled task.** It is ratified as an intent + a
  sequencing, to be executed incrementally alongside product work, each step green and the editor
  never broken — never as a stop-the-world rewrite.

## 2026-07-19 addendum — the mechanism refined to a *container of native parts*

Design-doc iteration (the internal document-model design, v0.1)
refined *how* this is realized, without changing the decision. The core idea is a **multi-surface
renderer and compositor**; the plugins are content-type **engines** (web=HTML/CSS, sheet=tabular,
draw=vector, data, image). So:

- **`.paged` is a *container* format, not a single document format.** It holds each content type in its
  **own native representation, in parallel** (IDML for print, HTML/CSS, tabular/SQLite, vector, …),
  plus **one core-owned *composition* part** that arranges them across surfaces (regions, region-chains
  = the `FlowId` flow protocol, positioning constraints, layers, template/instance, typed cross-part
  references, shared-resource graphs).
- **The "native document model" is therefore the *composition* model** — small and clean — not a
  re-invention of every content model. The lessons notebook's misshapen-concept reshapings live in the
  composition (arrangement/flow/positioning/templating/typed-refs), **not** in the content formats.
- **IDML becomes a *native part*, not a projected adapter** — so IDML-origin content round-trips
  **byte-perfect by storage**. This *strengthens* the decision (IDML ≠ the model) and largely
  **dissolves the round-trip-fidelity risk** (this ADR's §7 biggest concern): per-part native = lossless;
  loss appears only when the *whole composition* is flattened to a *single* foreign format (inherent +
  diagnosed).
- **The semantic spine (cross-content find/replace, tagged-PDF, accessibility, whole-doc single-format
  export) becomes a *derived* view**, computed by asking each engine to project its part into a shared
  vocabulary — never storage.
- **The print/DTP engine stays core-resident** (fidelity + embeddability, §5 above); the others are
  plugins.

Two further refinements from the same discussion:

- **Print storage resolved — a Paged-native *publishing* format, not raw IDML** (design doc §4/§5/§8a).
  Every content engine stores its *own* native format; the print engine is Paged's own (core-resident,
  fidelity path), so its native format is Paged's **publishing format** — **IDML `defines the publishing
  topics`** (the feature scope) and is the print engine's **import/export interchange**, not storage.
  Reason: IDML keeps evolving; storing it raw chains the substrate to Adobe's spec and bakes in the
  misshapen concepts. Consequence: the IDML round-trip is via a bounded Paged-owned converter +
  provenance carry-through (lossless for IDML-origin), not raw-byte storage — the ADR-021 §4 model,
  localized to the print part. (Other native parts — HTML, SQLite — remain byte-perfect by storage.)
  The two *new* formats Paged authors are the publishing format + the composition format
  ([`document.pgd`](../reference/composition-format.md)).

- **Get the foundations right up front — design-first, then incremental** (design doc "Design
  discipline" + migration §7 step 0). "Evolutionary" governs the *roll-out*, not the *target*: a
  container/format substrate is expensive to change once documents exist, so the **load-bearing
  decisions** (container structure, part↔composition boundary, format-adapter boundary, shared-resource
  model, **versioning/extensibility**) are designed and gotten right *before any code*; only the
  *details* (node kinds, style props) evolve. The immediate next work is therefore **completing the
  design** (worked exemplars + the container/composition/publishing-format specs + the versioning
  model), NOT starting the `FlowId` code. The ADR-021 decision is unchanged.

## Amendment — 2026-10-02

Checked against the code at `9f933f1` (plugin-publish at `6994ad1`). The thesis stands and is
partly executed: the model types
are Paged-owned, a saved document carries a native model part, and the flow and composition
vocabularies exist as crates. The placement clause was reversed by ADR 022, as the header says,
and the code follows ADR 022. The text above differs from the code in five places.

**1. The IDML import/export adapter is not in this repository.** This supersedes, in "The
decision", "`plugin-publish` is NOT the home for IDML" and "This is not "move IDML out of
core." IDML I/O stays."; in the options table, the rejection of option Y; and the Consequences
bullet "Licensing invariant".

- `Cargo.toml:71-82` — the IDML adapter (`idml-import`, `idml-export`) "now lives in the public
  paged-media/plugin-publish repo and is git-depended back into the engine"; a `[patch]` block
  points the adapter's dependencies on the model crates back at the local sources.
- `crates/paged-canvas/Cargo.toml:22`, `:35`; `crates/paged-renderer/Cargo.toml:45`, `:55`;
  `crates/paged-sdk/Cargo.toml:26` — the git dependencies, each pinned to one revision.
- `plugin-publish: crates/idml-import`, `plugin-publish: crates/idml-export` — where the code is.

Two other writers of IDML remain here for their own purposes: the fixture generator
(`crates/paged-gen/src/package.rs:74`) and the fallback skeleton in
`crates/paged-store/src/package.rs`.

**2. The model types are Paged-owned.** This supersedes, in Sources, "the `Document` =
re-exported IDML AST". Code in the direction of this ADR has landed since the Consequences
bullet "No code lands from this ADR" was written.

- `crates/paged-model/src/lib.rs:15-16` — "the Paged-owned data types and their pure value logic
  (geometry, IDML token maps), with **no XML/ZIP parsing**"; its only dependency is `serde`
  (`crates/paged-model/Cargo.toml:13-14`).
- `crates/paged-scene/src/lib.rs:31-34`, `:47-71` — `Document` is built from `paged_model`
  types, is serialisable, and has a `Default` so a document can be built with no IDML source.

The shape of the model still follows IDML: `Document` holds a `designmap`, spreads, stories and
master spreads (`crates/paged-scene/src/lib.rs:59-70`), and the codec says "The on-disk shape
mirrors today's IDML-derived model structure and **will churn**"
(`crates/paged-store/src/lib.rs:28-29`).

**3. A native model part exists; this ADR does not mention it.** The model is stored whole at
`paged/core/model/document.pgm`. It is neither the composition part nor the publishing format
of the addendum.

- `crates/paged-store/src/lib.rs:36` — the path; `:54` — `PGM_FORMAT_VERSION` (3 at the pinned
  commit); `:72-94` — JSON encode, and a decode that returns `None` for a part of another
  version.
- `crates/paged-canvas/src/model.rs:4659-4682` — every `.paged` export embeds a fresh model
  part beside the IDML parts.
- `crates/paged-canvas/src/model.rs:1520-1560` — on load a model part that decodes wins; an
  absent or incompatible one falls back to the IDML import.
- `crates/paged-store/src/package.rs:15-22` — `wrap_document` packages a natively built
  document with a one-page IDML fallback skeleton.

**4. The shipped container is still a valid IDML package.** The addendum's "`.paged` is a
*container* format, not a single document format", "IDML becomes a *native part*" and "Print
storage resolved — a Paged-native *publishing* format, not raw IDML" describe a design. What
ships is a ZIP that stays a valid IDML package, with a `paged/` namespace for the model part
and plugin parts, and a manifest ([ADR 118](118-paged-file-is-a-valid-idml-package.md);
[`../reference/paged-file-format.md`](../reference/paged-file-format.md)). No publishing format
is defined in `crates/`: the strings `content.pgp` and "publishing format" do not occur there.

- `crates/paged-canvas/src/model.rs:4659-4661` — "valid IDML + the plugin `paged/` parts + a
  refreshed `manifest.json`".
- `plugin-publish: crates/idml-export/src/paged.rs:200` — `write_paged`, the container writer.

**5. The flow and composition models exist as libraries; the composition is a derived
projection.** This is the state of the Consequences bullet "First concrete increment" and of
the addendum's composition part.

- `crates/paged-flow/src/lib.rs:49`, `:175`, `:216`, `:298` — `FlowId`, `RegionChain`,
  `Overset` and the driver `run_flow`. In `crates/`, `run_flow` is called only from tests; the
  renderer's story emitter is still the production path, and
  `crates/paged-renderer/src/flow.rs:28-29` says its flow content "does **not** yet replace
  `StoryEmitter`".
- `crates/paged-composition/src/lib.rs:47` — the part path
  `paged/core/composition/document.pgd`; `:35-38` — the crate is the arrangement kernel
  (surfaces, flows, pages, regions, positioning); templates and instances are "later slices".
  Its dependencies are `paged-flow` and `serde` (`crates/paged-composition/Cargo.toml:18-20`).
- `crates/paged-scene/src/lib.rs:383-396` — `to_composition` derives a composition from the
  document.
- `crates/paged-canvas/src/model.rs:4684-4703` — the composition part is "a persisted *derived*
  projection, not yet authoritative". `refresh_composition_part`, which writes it, is called
  only from tests, so an ordinary export does not contain it.
- `crates/paged-renderer/tests/c1_no_idml.rs:15-25` — the test this ADR asks for under "The C1
  seam gets enforced in code + CI": a hand-built region chain and a non-IDML content engine
  rendered to pixels with no `Document` involved. `crates/paged-composition-render/src/lib.rs:21-24`
  is its reusable form for a composition.

[`../reference/composition-format.md`](../reference/composition-format.md) is the format
document for the composition part.
