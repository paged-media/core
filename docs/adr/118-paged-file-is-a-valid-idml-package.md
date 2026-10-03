# ADR 118 — A `.paged` file is a ZIP that stays a valid IDML package

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** the container-parts door and load path in `crates/paged-canvas`, `crates/paged-store`, and the container writer in `plugin-publish: crates/idml-export`

## Context

Plugins persist their own data with a document. The note for protocol 51 gives the reason
for putting it inside the file: parts written into the container "travel with the `.paged`
file, unlike per-browser OPFS blob storage" (`crates/paged-canvas/src/channel.rs:330-332`).

The container writer states the rule the file obeys and what it buys. Extra parts "ride
alongside as ZIP entries UNREFERENCED by `designmap.xml` (the EPUB/ODF private-parts idiom —
InDesign ignores them on open)", and the mimetype "stays the Adobe IDML magic so InDesign
opens the file" (`plugin-publish: crates/idml-export/src/paged.rs:17-21`, `:156-159`).
[ADR 021](021-paged-native-document-model-idml-as-format.md) set the direction of a
container of native parts; this record describes the shape that is shipped.

## Decision

A `.paged` file is a ZIP that is a structurally valid IDML package. Everything Paged adds
is either under the `paged/` prefix or the top-level `manifest.json`.

- **Layout.** `mimetype` (the Adobe IDML value) is the first entry, stored. The IDML parts
  follow. Plugin parts live at `paged/<plugin>/…`; `paged/core/` is the engine's own
  namespace. `designmap.xml` references none of them.
- **`manifest.json`.** Written on every container save: `v`, `format: "paged-container"`,
  `pagedProtocol`, `idmlPartsHash` (FNV-1a 64 over the IDML parts), `domVersion`, and
  `parts`, an index of every `paged/` entry with its plugin, byte length and hash. Keys the
  writer does not own are kept.
- **The parts door.** `WritePagedPart`, `ReadPagedPart`, `ListPagedParts` and `ExportPaged`
  (protocol 51). The model keeps written parts as an overlay on the loaded archive. A
  write outside `paged/` is refused; with a named `caller` it is confined to
  `paged/<caller>/`. Reads and lists never serve IDML parts or the manifest.
- **The model part.** `paged-store` serialises the document model to
  `paged/core/model/document.pgm`, a JSON envelope `{format_version, model}` at version 3.
  `export_paged` adds a fresh one on each save. On load the part wins; when it is absent,
  unparseable or of another version, the IDML parts are imported instead.
- **Which half lives where.** The engine owns the parts door, the load order and the model
  codec. The writer `write_paged`, the manifest and the constants `PAGED_PREFIX` and
  `MANIFEST_NAME` are in `plugin-publish`'s `idml-export`, which the engine links at a
  pinned revision. `paged_store::package::wrap_document` is a second writer, for documents
  with no IDML source: the model part plus a one-page IDML skeleton.

## Evidence

- `crates/paged-canvas/src/channel.rs:327-335` — protocol 51: the four messages, "valid IDML + the paged/ parts + manifest.json"
- `crates/paged-canvas/src/model.rs:4600-4622` — the `paged/` and per-caller checks; `:4624-4657` read and list; `:4659-4682` `export_paged` embeds the model part and calls `idml_export::write_paged`
- `crates/paged-canvas/src/model.rs:1520-1560` — the load order: native part, else IDML import
- `crates/paged-store/src/lib.rs:36`, `:54`, `:72-94` — the part path, `PGM_FORMAT_VERSION = 3`, the envelope and `None` on mismatch
- `crates/paged-store/src/package.rs:15-23`, `:143-163` — `wrap_document`: the fallback skeleton and the entries it writes
- `plugin-publish: crates/idml-export/src/paged.rs:49-61`, `:82-105`, `:175-186`, `:200-280` — prefix and manifest name, the parts hash, the manifest fields, `write_paged`
- `plugin-publish: crates/idml-export/src/lib.rs:325-345` — one writer for `.idml` and `.paged`; the `.idml` product drops the container parts
- `crates/paged-canvas/Cargo.toml:35` — `idml-export` as a git dependency at revision `a88315d`

## Alternatives considered

- A mimetype of its own: not chosen. Paged "detects its own identity by this part + the
  `paged/` namespace, never by the mimetype" (`plugin-publish: crates/idml-export/src/paged.rs:156-159`).
- A binary model codec: "a deferred optimization"; JSON was taken "for now (inspectable,
  wasm-clean, matching the `document.pgd` precedent)" (`crates/paged-store/src/lib.rs:26-27`).

## Consequences

A `.paged` file carries the document twice: the IDML parts and the model part. The comment
there calls it a "Transitional cost" that goes away "when IDML leaves core"
(`crates/paged-canvas/src/model.rs:4668-4671`); the IDML code has since moved to
`plugin-publish` and both copies are still written. Inline image bytes are fields of the
model and are serialised inside the JSON model part; no separate image part exists.

Any change to the model's serde shape must raise `PGM_FORMAT_VERSION`; older files then
open through their IDML parts. With a model part adopted, the loader still reads
`Resources/Styles.xml` to fill missing style leading (`crates/paged-canvas/src/model.rs:9904-9912`).

The manifest is written but not read. No code in this repository or in `plugin-publish`
reads `idmlPartsHash` or `manifest.json` on load, so the data-loss guard the writer
describes has no read side, and a file whose IDML parts were changed while the model part
was kept opens from the model part. `wrap_document` writes no `manifest.json`.

The caller check is "an HONESTY AID", "not a security boundary": a write that names no
caller bypasses it (`crates/paged-canvas/src/model.rs:4589-4593`).
`crates/paged-store/src/lib.rs:19-20` still describes the raw IDML bytes as a skipped field
of the model; `Document` has no such field, and the loader keeps the source archive beside
the model (`crates/paged-canvas/src/model.rs:1526-1527`).

## Related

- [ADR 021](021-paged-native-document-model-idml-as-format.md) — the direction; [ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md) — why the writer is in `plugin-publish`
- [ADR 650](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/650-mutual-git-revision-pins.md) — the mutual revision pins; [ADR 007](007-carry-through-rendering-honesty.md) — carry-through on save
- `../reference/paged-file-format.md` — the format note the writer cites
