# ADR 019 — Capability catalog: one generated contract, projected to every surface

**2026-06-19 · decision record · status: ACCEPTED 2026-06-19 — Phases 1–4
landed (see the implementation addendum at the end).**
The capability surface (host fns, property paths, the element-id grammar, the
operation vocabulary) is now described in *several* places — and a new consumer
(the document-automation agent) just added another. This ADR proposes that the
machine-readable catalog be a **single generated contract owned by core**, with
each surface (Boa scripting, the plugin SDK, the agent, the feature registry, docs) a thin
**projection** of it — applying ADR 005's principle, not inventing a new one.

**Sources:** ADR [005](005-wire-recipe.md) (the wire recipe — "one descriptor
source feeds introspect + script; completeness-check is the guard") and
[008](008-read-surfaces-first-class-wire-collections.md) (read surfaces as
first-class wire collections — "single source of truth, no editor-side
parsing"); `crates/paged-script/src/catalog.rs` (the new `api_catalog()` +
the `SETTABLE_PATHS` list) and `parse_property_path` in
`crates/paged-script/src/lib.rs` (the 179
string→`PropertyPath` arms); the tsify projection (`PropertyPath` /
`ElementProperties` / `SceneLayer` `#[derive(Tsify)]` → `@paged-media/canvas-wasm`
`.d.ts`); `plugin-sdk: packages/plugin-api/src/wire.d.ts` (header: *"tsify output
from paged-media/core … Synced from @paged-media/canvas-wasm@0.51.0"*) and the
host-door files `editor.ts` / `mutations.ts` / `clipboard.ts` / `assets.ts` /
`widgets.ts` (capability-gated `host.*` surfaces); the feature registry's
completeness check ("every wire op / `paged.*` fn / gesture / panel
must map to a registry row"); the headless emitter `paged-run describe`
(`crates/paged-run`).

## The problem

The engine's capability surface is, today, described in five places that
overlap heavily and are maintained independently:

| Surface | What it describes | Source | Drift risk |
|---|---|---|---|
| **Vocabulary** (`PropertyPath`/`Mutation`/`SceneLayer`/`ElementProperties`) | The *types* | core enums → tsify → `canvas-wasm` `.d.ts` | Single-source ✓ — but **`plugin-api/wire.d.ts` is a *manual sync*** ("Synced from @0.51.0"), so it can lag the engine |
| **Boa scripting catalog** | What a `paged.*` script may do | `catalog.rs` `api_catalog()` + `SETTABLE_PATHS` | **NEW hand-list parallel to `parse_property_path`** — two lists of the same 179 paths |
| **Plugin host doors** | What a *plugin* may do (`host.sceneLayers/assets/clipboard/widgets`) + `capabilities.*` gates | `plugin-api/*.ts`, hand-authored | Independent of the catalog |
| **Capability matrix** | The human/dashboard view | the feature registry, hand-curated | Validated by completeness-check, but not against a generated catalog |
| **Docs** | The user/dev reference | `paged-media/docs` | Hand-written |

This is the smell ADR 005 already ruled against: *"one descriptor source feeds
introspect + script; completeness-check is the guard."* The wire-op surface
obeys 005. The **property-path + host-fn catalog does not yet** — and the
automation work just made it worse by adding `SETTABLE_PATHS` as a second
hand-maintained copy of the `parse_property_path` arms.

## The decision

**Make the machine-readable capability catalog a single contract, generated
from core's own descriptors, published like the protocol, and consumed by every
surface as a projection. Per-surface façades (Boa `paged.*`, plugin `host.*`)
stay distinct entry points but are *tagged subsets of the one catalog*, never
independent re-descriptions. The feature registry's completeness-check is the cross-surface
guard.** This is ADR 005 applied to the full catalog, not a new architecture.

Two layers, treated differently:

1. **Vocabulary (types) — already single-source; close the last gap.** Keep
   `PropertyPath`/`Mutation`/`SceneLayer`/… as core enums projected via tsify.
   The only fix: **automate the `wire.d.ts` sync** (it is a manual copy today)
   so the plugin SDK cannot lag the engine version.
2. **Catalog (the described surface) — make it one generated artifact.** Emit
   `{ hostFunctions, idGrammar, settablePaths, operations, constraints }` from
   the *same descriptors* that drive `parse_property_path` / introspect / script
   (per ADR 005). Publish it (a generated JSON/`.d.ts`, like the protocol) so
   build-time TS consumers (plugin SDK, editor) read it directly and the
   headless agent reads it at runtime via `paged-run describe`.

## Options

| Option | Mechanism | Verdict |
|---|---|---|
| **A — status quo** | Each consumer hand-describes its slice; tsify covers types only; manual `wire.d.ts` sync. | **Rejected.** Already drifting (two path lists in core); violates ADR 005; every new consumer multiplies the maintenance. |
| **B — one generated catalog, surface projections (chosen)** | One core-owned descriptor table feeds `parse_property_path`, the catalog, and tsify; catalog published; façades are tagged projections; completeness-check guards. | **Chosen.** Minimal new concept — it is ADR 005's rule, finished. Incremental: collapse the in-core dup first, then point consumers at the catalog one at a time. |
| **C — full façade codegen** | Generate the plugin `host.*` TS *and* the Boa registration *and* docs from the catalog. | **Deferred.** The right end-state for the *vocabulary*, but the façades carry hand-written semantics (capability gates, async boundaries, undo coalescing) that aren't mechanical. Generate the *catalog* now; codegen façades only where they're truly mechanical. |

## Why B, on the evidence

1. **It's already the house rule.** ADR 005 ratified "one descriptor source
   feeds introspect + script; completeness-check is the guard" for wire ops.
   The property-path catalog is the same shape of thing and should obey the
   same rule. This ADR doesn't add a principle — it stops an exception.
2. **The duplication is real and just grew.** `SETTABLE_PATHS` (179 entries) is
   a hand-copy of the `parse_property_path` arms, kept honest only by a
   round-trip test. That test catches *phantom* paths but not *missing* ones —
   exactly the half-guarantee ADR 005 exists to avoid. One table feeding both
   removes the class of bug.
3. **The plugin SDK already trusts this pipe.** `plugin-api/wire.d.ts` is
   literally tsify output synced from `canvas-wasm`. The catalog is the same
   pipe widened from *types* to *described capabilities* — the SDK gets it the
   way it already gets the wire types, so the integration cost is "add a file to
   the sync," not "invent a channel."
4. **The feature registry is already the validator.** completeness-check enforces
   "capability → registry row" today by reading a list of capabilities. Feed it
   the *generated* catalog and the registry, the agent's catalog, and the plugin
   surface are all checked against one truth — drift becomes a failing gate, not
   a silent divergence.
5. **The agent needs it to be true, not just present.** An LLM that authors
   from a hand-list will eventually be handed a path the engine dropped, get a
   silent `false`, and flail. Generated-from-core is the difference between a
   catalog that's *documentation* and one that's a *contract*.

The honest cost: B is a **cross-repo convergence** (core emits → plugin-sdk +
editor + agent + the feature registry + docs consume), so it lands incrementally over several
changes, not in one commit. Until it's complete the duplication persists behind
the round-trip/completeness guards — acceptable because those guards already
catch the dangerous direction (phantom capabilities), and the convergence
removes the rest.

## Consequences

- **Immediate:** collapse the in-core duplication — one path
  table feeding *both* `parse_property_path` and `api_catalog()` — so core stops
  carrying two lists. (Deferred to its own change by this ADR; not blocking the
  automation merge.)
- **Placement:** the shared catalog should be emitted from a neutral core crate
  (candidate: `paged-introspect`, already the scene/property introspection home)
  and surfaced through `canvas-wasm`/tsify — *not* left in `paged-script`, which
  is only the Boa surface. `paged-run describe` stays the headless emitter; the
  published artifact serves build-time TS consumers.
- **Sync automation:** `plugin-api/wire.d.ts`'s manual "Synced from @x.y.z"
  becomes a generated/checked step so it can't lag a protocol bump.
- **completeness-check ingests the catalog:** the guard widens from wire-op /
  `paged.*` / gesture / panel coverage to "every catalog capability has a
  registry row, and every surface projection is a subset of the catalog."
- **Façades stay hand-written where semantics are non-mechanical** (capability
  gates, async worker boundaries, K-1 undo coalescing) — they reference the
  catalog for vocabulary but keep their own behavior. Option C (codegen) is
  revisited per-façade only where the mapping is purely mechanical.
- **Docs become a projection too** — the format/engine reference can cite the
  generated catalog rather than re-listing paths, closing the last hand-list.

## 2026-06-19 addendum — RATIFIED + implemented (Phases 1–4 landed)

All four phases shipped the same day the ADR was written:

- **Phase 0 (in-core dedup) — core PR #9.** One `PROPERTY_PATHS` table feeds
  both `parse_property_path` (a one-line delegate) and `api_catalog()`; the
  193-line duplicate match is gone.
- **Phase 1 (relocate) — core PR #10.** The catalog moved from `paged-script` to
  the neutral, published `paged-introspect`; `paged-script` delegates across the
  boundary (acyclic). `paged-run describe` unchanged.
- **Phase 2 (publish) — core PR #10.** `paged-introspect-wasm` gained
  `describeCatalog()` (rides the existing `@paged-media/introspect-wasm` release
  at `0.<protocol>.<patch>`); a committed `catalog.json` (drift-gated by
  `catalog_json_artifact_is_current`) is the build-time artifact, shipped in the
  npm package.
- **Phase 3 (consumers) — plugin-sdk PR #5.** `scripts/sync-catalog.mjs` (sibling
  of `sync-wire.mjs`) vendors the catalog with a `--check` gate (live once
  `introspect-wasm` ships `catalog.json`). The document-automation agent already
  reads it via `paged-run describe`.
- **Phase 4 (the guard) — the feature registry.** Its `completeness-check` now ingests the
  catalog and asserts every `paged.*` host fn → a registry row (bidirectional
  drift); the authoring fns are mapped to their `scripting.*` rows.

**Corrections to the body, learned from implementation:**
- The `wire.d.ts` sync is **already automated** (`sync-wire.mjs --check`, a hard
  CI gate) — the body's "manual copy" was wrong; the real gap (now filled) was
  the *missing catalog sync*, not the type sync.
- `paged-introspect` was the right home: it already carried a `PropertyPathJson`
  mirror of `PropertyPath` (different naming scheme — ergonomic JS aliases vs
  wire variant names — so they project the same variant set, *not* merged).
- The plugin-side duplication is the **capability-manifest triplet**
  (`manifest.ts`/`assets.ts` ↔ `manifest.schema.json` ↔ `plugin-cli` Sets) — a
  *plugin-host* vocabulary, a sibling surface to the engine catalog. **Collapsed
  2026-06-19 (plugin-sdk #6):** `manifest.schema.json` is declared the single
  source (`$comment`) and `capability-vocabulary.spec.ts` gates the TS unions +
  CLI Sets against it (7 vocabularies). The hard constraints (type-only
  `plugin-api`, zero-dep standalone CLI) rule out one runtime literal feeding all
  three, so the collapse is a *gate*, not a physical merge — but drift is now a
  CI failure, not a discipline. All **9** capability vocabularies are gated
  (plugin-sdk #7 closed the last one — `gpu` realm, which is schema-constrained
  by `const "bundle"`, not an `enum`; the gate is now `const`-aware and models
  the accepted/reserved/type three-way: schema accepts `bundle`, the CLI splits
  accepted vs reserved, the TS type is `bundle|shared`, and the reserved `shared`
  is proven disjoint + schema-excluded per
  [ADR-018](018-stage-b-gpu-texture-defer-record-only.md)).

**Still open (tracked, not blocking):** activate `sync-catalog --check` in
plugin-sdk CI once `introspect-wasm` ships `catalog.json`; project the docs API
reference from the catalog.

## Amendment — 2026-10-02

Checked against core at `9f933f1`, plugin-sdk at `d90f727` and docs at `2094ba4`. The
decision stands and both "Still open" items are closed. Five statements above no longer
match the code.

**1. The plugin-sdk catalog gate is armed, with three outcomes.** This supersedes "activate
`sync-catalog --check` in plugin-sdk CI once `introspect-wasm` ships `catalog.json`".

- `plugin-sdk: .github/workflows/publish.yml:56-77` — the publish workflow installs
  `@paged-media/introspect-wasm` at the version the vendored wire types are stamped with and
  runs `scripts/sync-catalog.mjs --check` as a hard gate.
- `plugin-sdk: scripts/sync-catalog.mjs:136-227` — the check has three outcomes: in sync,
  drifted, or leading the published package. A lead passes only while a provenance record
  declares it, gives a reason, and targets a protocol newer than the published one; once the
  published package matches, a record that still claims the lead fails the check.
- `plugin-sdk: packages/plugin-api/catalog.provenance.json` — the provenance record. At the
  pinned commit it names core `6dea692`, protocol 64, and `aheadOfPublished: false`.

A limit: the vendored copy (`plugin-sdk: packages/plugin-api/src/catalog.json`) is gated
but not consumed there. No file under `packages/*/src` imports it, and the `files` list of
`packages/plugin-api/package.json` (`dist`, `src/manifest.schema.json`) does not ship it.

**2. The docs reference is projected from the catalog.** This supersedes "project the docs
API reference from the catalog".

- `docs: scripts/generate/gen-scripting.mjs:3-18`, `docs: scripts/generate/gen-idml-schema.mjs:4-22`
  — both generators read the pulled `catalog.json`.
- `.github/workflows/notify-docs.yml:3-14` — a change to `crates/paged-introspect/catalog.json`
  on `main` asks the docs site to rebuild.

**3. The path table is generated by one macro, and the `PropertyPathJson` mirror is gone.**
This supersedes, in the corrections above, "it already carried a `PropertyPathJson` mirror of
`PropertyPath` … *not* merged", and the path counts (179) in the body.

- `crates/paged-introspect/src/catalog.rs:498-547` — `property_paths!` generates
  `PROPERTY_PATHS` (the advertised names), `ALL_PATHS`, `wire_name`, `unadvertised_reason`
  and `variant_name` from one token list; the `match` arms have no wildcard, so a new
  `PropertyPath` variant does not compile until it is listed.
- `crates/paged-introspect/src/descriptor.rs:29-43` — the comment recording that the
  `PropertyPathJson` mirror was removed because both types serialised to identical JSON.
- `crates/paged-script/src/lib.rs:3957-3973` — `parse_property_path` and
  `property_path_label` both delegate to `paged-introspect`. `crates/paged-script/src/catalog.rs`,
  named in Sources, no longer exists.
- `crates/paged-introspect/catalog.json` at the pinned commit lists 219 settable paths;
  15 more paths are named but not advertised, each carrying a reason string
  (`crates/paged-introspect/src/catalog.rs:1023`, `:1103-1107`).

**4. The catalog carries more than the five keys in "The decision".** `ApiCatalog` also has
`elements` (IDML elements and their attributes, for the generated docs tables) and
`pathAliases` (the eight paths whose raw wire spelling differs from the advertised name);
`operations` is read from the `paged-wire` crate (`crates/paged-introspect/src/catalog.rs:82-111`,
`:196-201`). The committed artifact lists 145 host functions and 118 operations.

**5. Emitters and gates in this repository.** Where the text says `paged-run describe`:
`paged-run` is now the session binary under a second name
(`crates/paged-run/src/main.rs:15-35`). The catalog is emitted by `paged describe`
(`crates/paged-cli/src/inspect.rs:117-121`), by the session command `{"cmd":"describe"}`
(`crates/paged-cli/src/session.rs:436-440`) and by `describeCatalog()`
(`crates/paged-introspect-wasm/src/lib.rs:62-66`); all three serialise
`paged_introspect::api_catalog()` (`crates/paged-script/src/lib.rs:64` re-exports it). The
committed `catalog.json` is held current by `catalog_json_artifact_is_current`
(`crates/paged-introspect/src/catalog.rs:1340-1352`) and is copied into the published
`@paged-media/introspect-wasm` package (`.github/workflows/publish-wasm.yml:247-257`).

The completeness check that this ADR names as the cross-surface guard lives outside this
repository. Inside it, two tests gate what each surface can reach:
`crates/paged-script/tests/script_surface.rs` (which wire operations a `paged.*` script can
name) and `crates/paged-cli/tests/cli_surface.rs` (which message kinds the CLI sends).
