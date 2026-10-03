# Technical Briefing: The `.paged` Container Format

18 June 2026. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

> **Evolution (2026-07-19, [ADR 021](../adr/021-paged-native-document-model-idml-as-format.md)):** the
> native-document-model direction *evolves* this container — it inherits everything here and changes
> one thing: **print/IDML content moves to a Paged-native publishing part (the truth), and the
> canonical IDML parts become its *derived* projection** (the same `spec → derived` pattern §4/§5
> already applies to plugin objects), plus a core-owned *composition* part and a formal
> versioning/extensibility model. The details are in an internal design note. This doc
> remains the authoritative description of the shipped v1 container.

*Status note (2026-10-02): the container as it ships today, including the core-owned native model part `paged/core/model/document.pgm` (`crates/paged-store/src/lib.rs`), is recorded in [ADR 118](../adr/118-paged-file-is-a-valid-idml-package.md). This document names the crates `paged-parse` and `paged-write`, which no longer exist in this repository: the IDML reader and writer moved to the plugin-publish repository ([ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md)). The container writer `write_paged` is now `plugin-publish: crates/idml-export/src/paged.rs`, and the type that retains every ZIP entry is `SourceArchive` in `plugin-publish: crates/idml-import/src/lib.rs`.*

---

## 1. Summary

Paged has outgrown IDML as a *storage substrate*. The new plugins — `plugin-sheet` (spreadsheet + charting), `plugin-image` (non-destructive raster), `plugin-data` (DuckDB-backed publishing) — introduce object models (live formulas, layer stacks, datasets and queries) that IDML was never designed to hold.

The recommended response is **not** to invent a bespoke binary format. It is to **promote Paged to own the document container, and demote IDML to a profile of it.**

The canonical format becomes a **ZIP/OPC container** with the `.paged` extension that is, at all times, a **structurally valid IDML package** — so InDesign can open and view it directly. IDML-structured parts remain intact and untouched; each plugin owns a namespace of additional parts that ride alongside as unreferenced entries (the EPUB/ODF private-parts idiom). Genuinely-binary payloads reuse existing standard formats (Parquet/Arrow, PNG/WebP/EXR). A bespoke binary format is reserved, if ever, for an **optional derived fast-load cache** — never the source of truth.

**Critical boundary:** InDesign can *open and view* the file, but cannot *round-trip* it. InDesign regenerates the IDML package from its internal model on save, which strips all `paged/` parts. InDesign is therefore a view/layout-edit target, never a custodian of Paged-specific data. This boundary drives the extension and export decisions in §3.1.

### 1.1 Implementation status

*Status note (2026-10-02): this table is the state on 18 June 2026 and predates the move of the IDML code; see [ADR 118](../adr/118-paged-file-is-a-valid-idml-package.md) for the container as shipped. Where a row says "core `paged-write`" or "`paged-parse`", read `plugin-publish: crates/idml-export` and `plugin-publish: crates/idml-import`. The wire operations are declared in `crates/paged-canvas/src/channel.rs`.*

The container keystone is **shipped end-to-end and published** (engine wire protocol **v51**; packages publish at `0.<protocol>.<patch>`, so the door is `canvas-wasm`/`introspect-wasm` 0.51.0). Two plugins are migrated. The remainder is either InDesign-empirically-gated or has no current producer/consumer to build against.

| Area (section) | Status | Where it lives |
|---|---|---|
| Container writer; unknown parts round-trip untouched (§3) | ✅ shipped | core `paged-write::write_paged`; `paged-parse Container.entries` retains every ZIP entry, `raw_copy_file` carries them byte-identical |
| `manifest.json` self-identity + IDML-parts content hash (§3.1) | ✅ shipped | core `write_paged` / `idml_parts_hash` (FNV-1a, excludes `paged/` + `manifest` + `mimetype`) |
| `host.parts` read/write/list door, namespaced per plugin (§3, §8.1) | ✅ shipped + published | plugin-sdk `PartsSurface` + runtime `storage.parts@1`; wire ops `WritePagedPart`/`ReadPagedPart`/`ListPagedParts`/`ExportPaged` at protocol v51 |
| Plugin-declared `contributes.partTypes[]` (§8.1) | ✅ shipped | plugin-sdk manifest schema + CLI; declared by plugin-web (`webSource`/spec) + plugin-sheet (`workbook`/source) |
| Parts index + per-part content digest in the manifest (§7, §8.3) | ✅ shipped | core `write_paged` `parts` array `{path, plugin, bytes, hash}`, recomputed each save |
| Per-plugin migration of spec/source into the container (§4) | ◑ web + sheet | plugin-web `source.json` (spec); plugin-sheet `workbook.xlsx` (source). data persistence is unimplemented, image save-back unbuilt — no existing lane to migrate; draw is label-resident |
| Flatten → IDML `<Image>` placement for the InDesign-visible copy (§3.1, §5) | ◻ deferred | InDesign-empirically-gated **and** no derived-part producer exists yet |
| Open-time data-loss-guard *reader* (§3.1) | ◻ deferred | needs a breadcrumb in an InDesign-*preserved* part (InDesign strips `manifest.json` + `paged/` alike) — empirically gated |
| Empirical InDesign silent-open verification (§3.1, §9) | ◻ open (blocking) | no InDesign available to test against on the dev machine |
| Producer **version** stamp + object↔page-item bindings (§7, §8.2) | ◻ deferred | the writer records path/plugin/bytes/hash; the plugin-supplied version + the bindings map layer on later |
| Embedded-or-linked source + `snapshot.parquet` (§6) | ◻ deferred | depends on plugin-data persistence, which is unimplemented |
| Fast-load cache (§9) | ◻ deferred *by decision* | only when load-time is a measured bottleneck |

Legend: ✅ shipped · ◑ partial · ◻ not yet. Note: the "unknown parts round-trip" guarantee (§3, the single most important one) was effectively **free** — the carry-through writer already preserved foreign ZIP entries byte-identically; the only new work was *emitting* model-added parts.

---

## 2. Why not a bespoke binary format

A custom binary source-of-truth is almost always the wrong default for a document format, and here it would be a strategic regression:

- **Loses inspectability, diffability, and toolability.** The open-container approach keeps the document greppable, version-controllable, and repairable.
- **Loses forward compatibility.** A container of standard parts degrades gracefully; a bespoke binary format requires every reader to understand every version.
- **Off-brand.** It contradicts the plugin-owns-its-namespace logic of the Paged platform. The same principle that governs the platform should govern the file.

Invent a binary format only when a measured need arises that a container-of-standard-parts genuinely cannot meet. That threshold has not been reached.

Two separations keep the binary question clean:

1. **Reuse existing binary formats for binary payloads.** Pixel layers → PNG/WebP/EXR + a JSON layer-stack manifest. Datasets → Parquet/Arrow straight out of DuckDB. Font subsets → as-is. This work is done and better than anything bespoke would ship.
2. **Separate disk format from runtime format.** The Vello/WASM renderer wanting a tight binary scene graph *in memory* is legitimate and unrelated to how the *file* is stored. If large-document load time ever becomes a measured bottleneck, the answer is an optional derived binary cache alongside the canonical container, not a bespoke source of truth.

---

## 3. Container architecture

A ZIP/OPC container, mirroring the OOXML convention:

```
document.paged  (ZIP; valid IDML package + Paged parts)
├── mimetype                     STORED, first entry: application/vnd.adobe.indesign-idml-package
├── manifest.json                Paged self-identity + object↔page-item map + producer metadata
│
├── designmap.xml                ┐
├── Stories/                     │  canonical IDML parts, untouched
├── Spreads/                     │  (a valid .idml on their own)
├── Resources/                   ┘
│
├── paged/sheet/<id>/...         plugin-sheet namespace (unreferenced by designmap.xml)
├── paged/image/<id>/...         plugin-image namespace
└── paged/data/<id>/...          plugin-data namespace
```

**Design rules:**

- **IDML parts stay in their canonical locations, untouched.** A valid `.idml` is recoverable by taking just those parts plus flattened renderings of plugin objects.
- **Each plugin owns a namespace of parts.** No central blessing of individual data types; the format manifest references the plugin manifest's declared part-types directly, so the persistence contract and the capability contract are the same thing.
- **Unknown parts must round-trip untouched, never dropped.** An older Paged build (or one missing a given plugin) must open the document, render the flattened output, and **preserve the live part on save.** This is the single most important guarantee.

### 3.1 InDesign interoperability

*Status note (2026-10-02): this section predates the implementation; "Export for InDesign" is built as a separate IDML write that drops the container parts, not as a copy of the file — see the note in §9 and [ADR 118](../adr/118-paged-file-is-a-valid-idml-package.md).*

**Goal:** InDesign can open and view a `.paged` file directly. **Non-goal (impossible):** InDesign preserving plugin data on save.

**Mechanism — dual identity.** The `.paged` file is, at all times, a structurally valid IDML package. The `paged/` parts ride alongside as ZIP entries that are *not referenced from `designmap.xml`*, so InDesign ignores them on open (the same way EPUB and ODF carry private parts). One ZIP, two readers: InDesign sees valid IDML; Paged additionally reads its own namespace.

**Identity / mimetype.** Because InDesign identifies the package by its magic `mimetype` entry (first in the ZIP, stored uncompressed), that entry **must** be `application/vnd.adobe.indesign-idml-package` — *not* a Paged-specific MIME type. Paged therefore detects its own identity by the presence of `manifest.json` and the `paged/` namespace, not by the mimetype. The file is "an IDML package that Paged recognises as also a Paged document," not the reverse.

**What InDesign renders.** Each plugin object's flattened rendering is embedded and referenced from its IDML page item, so InDesign displays a normal placed graphic. For the InDesign-visible copy, use **PDF (vector) or PNG** — IDML/InDesign place these reliably. SVG may remain Paged's own high-fidelity derived, but the IDML-referenced flatten should be PDF/PNG.

**The round-trip boundary (unavoidable).** InDesign builds an internal model on open and regenerates the IDML package on export. It does **not** carry foreign ZIP entries through. Any save from InDesign strips every `paged/` part. This is structural, not fixable. The danger is that it *looks* like it worked — formulas, layer stacks and datasets vanish silently.

**Extension decision — keep `.paged` canonical.** Do not make `.idml` the primary extension; that invites edit-in-InDesign → save → silent total data loss. Keep `.paged` so the OS routes the file to Paged, and treat the explicit export step's friction as a safety feature. Because the file is *already* valid IDML internally, **"Export for InDesign" is nearly free** — a copy with the `.idml` extension, no transformation, since the flatten happened at save time.

**Data-loss guard.** Stamp `manifest.json` with a content hash of the IDML parts at each Paged save. On reopen, if the IDML parts changed but the `paged/` parts are gone, the file went through InDesign — warn and offer a clean re-import rather than silently mis-merging. — *Status: the write side is ✅ shipped (`idml_parts_hash` in `manifest.json` each save). The open-time **reader** is ◻ deferred: InDesign strips `manifest.json` along with the `paged/` parts, so the breadcrumb must live in an InDesign-preserved (designmap-referenced) part — which one survives needs the empirical test below.*

**Verify empirically before committing:** confirm the target InDesign version opens a package containing unreferenced extra entries *silently* (the spec says it should ignore them, but Adobe's importer has its own behaviour). One real test file settles it. — *Status: ◻ open (blocking) — no InDesign on the dev machine; the dual-identity layout ships unverified against a real Adobe importer.*

---

## 4. The three-role storage model

The key insight for "where does the embedded content live" is that "embedded content" is three different things with different sizes, lifecycles, and owners. Storing them by role makes the layout self-evident and every plugin follow the same shape.

| Role | What it is | Characteristics | Source of truth? |
|------|-----------|-----------------|------------------|
| **Spec** | The declarative definition: formula set, chart definition, DuckDB query + field-to-frame bindings, image layer stack | Small, JSON, human-readable, diffable, authored | **Yes** — canonical, must round-trip |
| **Source** | The bytes the spec operates on: cell inputs, dataset rows, pixel layers | Potentially large, binary-friendly; may live *outside* the document | Yes (but may be linked, see §6) |
| **Derived** | The computed output: evaluated values, rendered chart, composited image | Regenerable; kept for viewers without a compute engine | **No** — regenerable cache |

**The traps this avoids:**

- Mixing **derived into source** → bloat plus staleness bugs.
- Mixing **source into spec** → giant unreadable JSON, loss of diffability.

### Per-plugin layout

```
paged/sheet/<id>/spec.json        spec   (formulas, chart definition)
paged/sheet/<id>/values.parquet   source (literal cell inputs)
paged/sheet/<id>/chart-1.svg      derived (IDML-visible flatten)

paged/data/<id>/query.sql         spec   (DuckDB query)
paged/data/<id>/source.json       spec   (external URI + fetch metadata)
paged/data/<id>/snapshot.parquet  source (cached snapshot of external)

paged/image/<id>/stack.json       spec   (blend modes, masks, adjustments)
paged/image/<id>/layer-2.png      source (pixel layer)
paged/image/<id>/composite.png    derived (composited result)
```

> *Status: ✅ shipped for **web** and **sheet**; the rest is illustrative.* The live namespace uses the **full plugin id**, not the short name above — `paged/media.paged.web/<id>/source.json` (spec, the HTML/CSS envelope) and `paged/media.paged.sheet/workbook.xlsx` + `workbook.name` (source). Both write through `host.parts` and are preferred over the legacy label/OPFS on read, with a one-time blob→part migration. `data`/`image` remain illustrative until those plugins grow a persistence lane (see §1.1).

---

## 5. Definition vs. placement

Separate the *definition* of an object from its *placement* on a page.

- The full spec is **one part, keyed by object ID.**
- The placed frame is an **IDML page item that references that ID** and carries only instance overrides (crop, scale, which chart view).

This reuses the exact idiom IDML already uses for linked stories and images — no invention required — and it lets the same dataset be placed twice with two different chart views.

---

## 6. Embedded vs. linked source

For data especially, `source` should support **embedded-or-linked**, mirroring IDML's linked-vs-embedded image distinction:

- `source.json` points at an external URI — a file, an API, or a governed data endpoint — plus fetch metadata.
- A **cached snapshot** (`snapshot.parquet`) lives in the container so the document opens offline and reproducibly.

Large or live data does not bloat the document; it is referenced with a fallback.

---

## 7. Staleness management via producer stamps

Every **derived** part carries who produced it: `plugin id`, `version`, and a **content hash of its inputs** (spec + source).

On open, a viewer decides per object:

| Condition | Action |
|-----------|--------|
| Inputs unchanged, engine present | Trust cached derived (or recompute) |
| Engine absent | Show cached derived as-is |
| Inputs changed | Recompute, mark stale |

> *Status: ◑ partial. The manifest's `parts` index ships the **content-hash + plugin-id** half (`{path, plugin, bytes, hash}`, FNV-1a, recomputed each save) — the integrity/staleness substrate. The plugin-supplied **version** stamp and the per-object trust/recompute decision table above are ◻ deferred (no derived-part producer exists yet, so there is nothing to stamp or recompute).*

### Alignment with salsa

*Status note (2026-10-02): this section predates the implementation; salsa was not adopted. See [ADR 107](../adr/107-whole-document-build.md) (layout is a whole-document build) and [ADR 027](../adr/027-incremental-flow-invalidation.md) (the incremental invalidation built on top of it).*

> *Correction (implementation reality): core is **not** salsa-based / incremental today — `CanvasModel` rebuilds the whole document per mutation; `salsa` is an unstarted retrofit, not a current dependency. The shipped staleness mechanism is therefore plain **hash-compare on open** (the `parts` index), independent of salsa. The alignment below is aspirational — the target the retrofit should reach, not a description of what runs.*

The intent is for this to become the same dependency logic a future salsa-based incremental layout runs at runtime:

- `spec` and `source` parts → **salsa inputs**
- `derived` parts → **memoized queries** that recompute when an input hash changes

The on-disk role split is the incremental computation graph serialized. The file is designed to mirror that computation — so it feels native rather than bolted on once the retrofit lands.

---

## 8. Manifest schema implications

The recommended direction shapes the manifest schema in three ways to decide early:

1. **Part-types are declared by plugins, not centrally.** `manifest.json` references the capability-based plugin manifest's declared part-types directly. One registry, not two kept in sync. — *✅ shipped: `contributes.partTypes[] { type, role, format, linkable? }` in the plugin manifest + CLI validation; declared by web + sheet.*
2. **The manifest holds the object ID ↔ IDML page-item bindings** so flattening and re-linking are mechanical. — *◻ deferred (layers on with the flatten/placement work, §5).*
3. **Producer metadata (plugin id, version, input hash) is recorded per derived part** for the staleness logic in §7. — *◑ partial: the `parts` index records path/plugin/bytes/hash for every part; the plugin **version** is not yet stamped (the writer derives the plugin id from the path but doesn't receive a version at write time).*

---

## 9. Open decisions

*Status note (2026-10-02): this section predates the implementation; the export to `.idml`, described below as not yet built, ships as the `ExportIdml` wire operation (`crates/paged-canvas/src/channel.rs`) over `write_idml` in `plugin-publish: crates/idml-export/src/lib.rs`, which drops the container parts instead of copying the file. See [ADR 118](../adr/118-paged-file-is-a-valid-idml-package.md).*

- **Third-party persistence:** ✅ **RESOLVED — yes.** Third-party plugins persist their own namespaced parts without central blessing; the `host.parts` door is namespace-gated to the plugin's own id, and `write_paged` carries through *and* re-indexes unknown third-party parts + manifest keys untouched (covered by the multi-tenant preservation test). The format manifest binds to the plugin capability manifest (§8.1).
- **Identity / extension:** ✅ **Confirmed (unchanged).** `.paged` extension; magic `mimetype` = `application/vnd.adobe.indesign-idml-package`; Paged self-detects via `manifest.json` + `paged/` namespace. Do not adopt a Paged-specific MIME type in the magic position, and do not make `.idml` the primary extension (§3.1). *Self-detection is shipped in the writer; the export-to-`.idml` copy step is not yet built.*
- **InDesign open behaviour:** ◻ **STILL OPEN (blocking).** Verify empirically that the target InDesign version opens a package with unreferenced extra entries silently (§3.1). Not yet verified — no InDesign on the dev machine. The dual-identity layout ships unverified against a real Adobe importer.
- **Derived retention policy:** ◻ **Open (recommended: always embed).** Always embed derived for viewer fallback vs. embed-on-export-only. (Recommended: always embed, since `idml-viewer` and InDesign both need it. Use PDF/PNG for the InDesign-visible flatten.) *Moot until a plugin produces a derived flatten part (§3.1, §5).*
- **Fast-load cache:** ◻ **Deferred by decision.** Defer until load time is a *measured* bottleneck; then add an optional derived binary cache, never a bespoke source of truth.

---

## 10. One-line recommendation

> Adopt a ZIP `.paged` container that is, at all times, a valid IDML package — Adobe mimetype magic, IDML parts intact, plugin parts in unreferenced namespaces, Paged self-detecting via its manifest. InDesign opens and views it directly (flattened representations as PDF/PNG); it cannot round-trip plugin data, so keep `.paged` canonical and make "Export for InDesign" a near-free `.idml` copy. Store every extended object in three roles — spec (canonical, JSON), source (binary, embedded or linked), derived (regenerable, stamped). Reserve bespoke binary for an optional fast-load cache only, never the source of truth.