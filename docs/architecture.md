# Architecture

How the `paged` engine is built: a Rust render engine for page-layout documents, with the
editing model, the wire protocol and the published surfaces on top of it. This page
describes the code at commit `9f933f1`; the reasons are in the ADRs under
[`adr/`](adr/README.md), linked where they apply. Paths are relative to the repository
root. The root `README.md` and `CLAUDE.md` describe an older workspace in places (crate
count, shaping library, colour on wasm); this page follows the code.

## The workspace

One Cargo workspace (`Cargo.toml`): 24 crates under `crates/` and three spikes that are
workspace members (`spikes/vello-eval`, `spikes/composer-calibration`, `spikes/wasm-size`).
`spikes/blitz-wasm` is excluded and has its own lockfile. The toolchain is pinned to 1.94.1
in `rust-toolchain.toml`, `Cargo.lock` is tracked, and the workspace sets `publish = false`.
Besides Rust there is one TypeScript package (`web/idml-viewer`), bash for the gates
(`scripts/`, `corpus/generated/*.sh`), and ExtendScript in `tools/indesign-export`.

The IDML import/export adapter is not in this repository. `idml-import` and `idml-export`
live in the plugin-publish repository and are taken as git dependencies pinned to a commit
(for example `crates/paged-canvas/Cargo.toml`). They in turn depend on this repository's
model crates, so the root manifest carries a `[patch]` that points those back at the local
sources and Cargo resolves one copy of each. See
[ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md)
and [ADR 650](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/650-mutual-git-revision-pins.md).

## Crates by role

A crate depends only on crates to its right in the same row or in a row below
(dev-dependencies left out):

```
binaries       paged-run   paged-cli   paged-gen   paged-fidelity
wasm surfaces  paged-canvas-wasm   paged-introspect-wasm   paged-sdk
editing        paged-script   paged-canvas   paged-introspect
output         paged-composition-render   paged-export-pdf   paged-renderer   paged-gpu   paged-compose
text, colour   paged-text   paged-color
vocabulary     paged-wire   paged-mutate
model          paged-store   paged-scene   paged-composition   paged-flow   paged-model
```

**Model.** `paged-model` holds the document's data types and their pure value logic; its
only dependency is `serde`. `paged-scene` wraps them in `Document`, the object the pipeline
and the mutation code work on: designmap, swatches, spreads, stories, master spreads and
styles, plus lookup indexes that are not serialised and are rebuilt by
`Document::rebuild_indexes` (`crates/paged-scene/src/lib.rs:59-99`). `paged-flow` is a
content-agnostic protocol for flowing content through an ordered chain of regions,
`paged-composition` an arrangement model (surfaces, pages, regions, flows) that holds no
content, and `paged-store` the codec that writes `Document` as a container part
([ADR 021](adr/021-paged-native-document-model-idml-as-format.md)).

**Text and colour.** `paged-text` shapes runs with `harfrust`, breaks lines with
`paragraph-breaker` and three composers of its own (`single_line.rs`, `ragged.rs`,
`first_fit.rs`), hyphenates, and caches laid-out paragraphs under a blake3 key
([ADR 102](adr/102-text-stack.md), [ADR 103](adr/103-hyphenation-sources.md)).
`paged-color` wraps `lcms2` on native targets and `qcms` on `wasm32`
([ADR 003](adr/003-lcms2-color.md)).

**Output.** `paged-compose` defines the display list, the effect masks and the scene-layer
types plugins submit. `paged-gpu` owns the `PathRasterizer` trait and two implementations
behind Cargo features: `cpu` (tiny-skia, the default) and `vello-backend` (Vello 0.10 on
wgpu 29) ([ADR 100](adr/100-two-rasterisers-one-trait.md)). `paged-renderer` is the
pipeline: `pipeline::build_document`, the asset resolver, diagnostics, and the
`paged-inspect` binary. `paged-export-pdf` writes PDF from the display list
([ADR 119](adr/119-pdf-export-backend.md)). `paged-composition-render` renders a
`Composition` through a caller-supplied region renderer; no other crate depends on it.

**Vocabulary.** `paged-mutate` defines `Operation`, the invertible primitive (87 variants),
its `apply`, the inverses, id minting and the path geometry (`kurbo`, `flo_curves`;
[ADR 004](adr/004-kurbo-geometry-kernel.md)). `paged-wire` holds the identities (`PageId`,
`ElementId`, `TextCellAddr`, `ByteBuf`) and the client-facing `Mutation` enum. The
`mutations` half is a default feature that brings in `paged-mutate`; `paged-renderer` takes
the crate without it, and its own use of `paged-mutate` is a feature the viewer switches
off ([ADR 111](adr/111-wire-vocabulary-leaf-crate.md)).

**Editing.** `paged-canvas` is the worker-side model: `CanvasModel`, the message envelopes
(`channel.rs`), hit testing, selection, gestures, snapping, caret geometry, snapshots, the
PDF export session and the container parts; it exports no functions to JavaScript.
`paged-introspect` builds the scene tree, the property descriptors and the capability
catalog. `paged-script` embeds the Boa JavaScript engine and installs the `paged.*` host
functions ([ADR 001](adr/001-boa-over-quickjs.md)).

**Surfaces.** `paged-canvas-wasm`, `paged-introspect-wasm` and `paged-sdk` are the
wasm-bindgen crates. `paged-cli` builds the `paged` binary; `paged-run` is a shim over its
session module. `paged-gen` generates the fixture documents, writing IDML with `zip` and
`quick-xml` and no other crate of this workspace. `paged-fidelity` is the image-diff
library and the `paged-diff` binary.

## From document bytes to pixels

1. **Load.** `CanvasModel::load` (`crates/paged-canvas/src/model.rs:1513`) opens the ZIP
   with `idml_import::open_source_archive`. If the archive holds
   `paged/core/model/document.pgm` and `paged_store::from_bytes` accepts it, the model is
   rebuilt from that part with no IDML parse; otherwise, or when the part has another
   format version, `idml_import::import_idml_archive` parses the IDML parts. The viewer
   calls `idml_import::import_idml_doc` directly (`crates/paged-sdk/src/lib.rs:203`).
2. **Build.** `pipeline::build_document(&Document, &PipelineOptions)`
   (`crates/paged-renderer/src/pipeline/mod.rs:1198`) lays out the whole document and
   returns a `BuiltDocument`: one `BuiltPage` per page, each with its `DisplayList`, plus
   diagnostics. The renderer keeps no layout state between calls (its one memo is of font ids).
   Fonts, the CMYK profile, decoded images and the layout caches belong to the caller, which hands them in through
   `PipelineOptions` (the paragraph cache is installed around the call)
   ([ADR 107](adr/107-whole-document-build.md)). Running headers and cross-references cost
   a second pass; a story with a grow rule is rebuilt with generated pages until it fits,
   at most 24 passes ([ADR 026](adr/026-auto-growing-region-chains.md)). Keep options are
   settled inside the build ([ADR 028](adr/028-pagination-rules-are-engine-owned.md)).
3. **Assets.** The pipeline does no file or network I/O. Fonts and placed images come
   through the `AssetResolver` trait, and the CMYK profile arrives as bytes. No typeface and
   no profile is bundled ([ADR 109](adr/109-engine-does-no-io.md)). Degraded output is
   reported in `BuiltDocument::diagnostics` ([ADR 007](adr/007-carry-through-rendering-honesty.md)).
4. **Display list.** `DisplayList` (`crates/paged-compose/src/display_list.rs:1463`) is a
   flat `Vec<DisplayCommand>` with pools for paths, gradients, images and spot inks. Text
   becomes one `FillPath` per glyph over interned outlines. Clips, blend groups, layers and
   soft masks are bracket markers in the same stream, and each object effect is its own
   command ([ADR 104](adr/104-effects-follow-indesign-parameters.md)). Paints are resolved
   when the list is built; a CMYK paint keeps its channels and spot ink next to the RGB
   value ([ADR 106](adr/106-colour-resolved-at-build-time.md)). `DisplayList::digest()`
   hashes the whole list and is the equality test between two builds
   ([ADR 101](adr/101-display-list-single-intermediate.md)).
5. **Raster.** `paged_gpu::rasterize` draws a list with tiny-skia into an RGBA image; this
   is what the command line, snapshots, ink coverage and the fidelity gate use.
   `VelloRasterizer` draws the same list with Vello; in the browser `SurfacePresenter`
   (compiled for `wasm32` only) presents it to a canvas. Ink separations exist on the CPU
   path only (`crates/paged-gpu/src/lib.rs:86-91`).
6. **PDF.** The export session rebuilds the document once with the glyph-run side channel
   on and the live build's caches off, then `paged-export-pdf` writes the pages from those
   display lists, one page per message (`crates/paged-canvas/src/export.rs:15-26`).

## The mutation path

A change arrives as a `Mutation` (`crates/paged-wire/src/lib.rs`), tagged `{op, args}`; the
roster holds 118 operations (comments in several files still say 117).
`CanvasModel::apply_mutation` (`crates/paged-canvas/src/model.rs:1894`) routes it:

- Settings that are not document content (`SetDocumentDefaults`, `SetColorSettings`, ink
  settings) are applied directly and leave no undo entry.
- Structural and property edits are translated into a `paged_mutate::Operation` and applied
  by `paged_mutate::apply`, which returns the applied operation with its inverse
  ([ADR 116](adr/116-mutations-lowered-onto-operations.md), [ADR 005](adr/005-wire-recipe.md)). The engine mints
  ids: page items, groups, tables, anchored frames and links take the next `u<hex>` number above the highest
  in use; story, section, colour and guide ids have their own namespaces (`crates/paged-mutate/src/ids.rs:15-39`).
- `InsertText` and `DeleteRange` go through a second lane, `TextOp`
  (`crates/paged-canvas/src/mutate.rs`). Text is addressed by story id and UTF-8 byte
  offsets ([ADR 117](adr/117-story-local-text-offsets.md)).
- A `Batch` applies its children with the rebuild deferred, rolls all of them back if one
  fails, and is logged as one undo step.

After a commit the model runs `build_document` over the whole document again
(`rebuild_after_mutation`, `model.rs:8779`), with the caches it owns handed in. A build
whose commits were text edits of one story may reuse the pages it did not lay out afresh
([ADR 027](adr/027-incremental-flow-invalidation.md)). With `PAGED_DIGEST_GATE=1` every
rebuild is compared with a cold build and a difference panics.

Undo is a log of pre-captured inverses, capped at 10,000 entries (`MAX_APPLIED_LOG`); it is
not the save path ([ADR 110](adr/110-one-undo-timeline.md)). `paged-mutate` has a second,
separate history (`history.rs`), used by the inspector wasm and by `paged-inspect`.
Interaction is computed in `paged-canvas` too: hit testing in paint order, content-addressed
selection, gestures with a begin, update, commit and cancel lifecycle, snapping, and caret
geometry ([ADR 114](adr/114-interaction-lives-in-the-engine.md)).

## Surfaces

**The worker protocol.** `MainToWorker` and `WorkerToMain` (`crates/paged-canvas/src/channel.rs`)
are envelopes with a `seq` and a `protocol` number; a request is one of 62 message kinds.
`PROTOCOL_VERSION` is 64 (`channel.rs:510`); `Hello` is answered with `Ready` carrying it.
`WorkerCore` (`crates/paged-canvas-wasm/src/dispatch.rs`) parses a message, runs the arm for
its kind and serialises the reply; it compiles on every target and is tested natively. Reads
are message kinds too ([ADR 008](adr/008-read-surfaces-first-class-wire-collections.md)).

**Editor wasm.** `paged-canvas-wasm` wraps `WorkerCore` in `CanvasWorker`. Messages cross
as JSON strings through `handleMessage`. The camera and gesture pointer deltas use fixed
`SharedArrayBuffer` layouts defined in `camera.rs` and `gesture.rs`, and document bytes can
be passed as a typed array through `loadDocumentDirect`
([ADR 115](adr/115-worker-boundary-transports.md)). With the `gpu` feature, `initGpu` and
`presentFrame` draw the visible pages with Vello onto an `OffscreenCanvas`.

**Viewer wasm.** `paged-sdk` exposes `ViewerSession`: `load`, `page_layout`, `present`,
`render_to_canvas`, RGBA readback. It links `paged-renderer` without default features and
`paged-gpu` with the Vello backend, and none of `paged-mutate`, `paged-canvas` or
`paged-script`; the release workflow checks the dependency tree for those three
([ADR 112](adr/112-viewer-sdk-is-a-sibling.md)). `web/idml-viewer` is a TypeScript wrapper
with no runtime dependencies: camera, input and events over an injectable session
interface ([ADR 123](adr/123-viewer-ships-from-core.md)). The third wasm crate,
`paged-introspect-wasm`, exposes `Inspector` (tree, properties, apply, undo, redo, render)
and `describeCatalog()`.

**Command line.** `paged` (`crates/paged-cli`) has the subcommands `render`, `inspect`,
`export`, `script`, `new`, `gen`, `diff`, `read`, `place`, `parts`, `describe`, `digest`
and `session`. State-changing commands go through `WorkerCore::dispatch`, the same entry
the wasm shell wraps. `paged session` and the `paged-run` binary speak one JSON object per
line on stdin and stdout, holding one model for the life of the process
([ADR 113](adr/113-one-typed-door.md)). `ExecuteScript` runs a script in a fresh Boa
context under a budget for loop iterations, recursion, stack and wall clock; its document
writes are `Mutation` values passed to `CanvasModel::apply_mutation`.

**Surface descriptions.** `crates/paged-introspect/catalog.json` (host functions, settable
paths, operations, elements) and `crates/paged-cli/cli.json` are generated and committed; a
test fails when either differs from the code. `web/idml-viewer/api-catalog.json` is written
by hand, and a test fails when an export is missing from it
([ADR 019](adr/019-capability-catalog-one-contract.md)). Two more tests pin what a surface
cannot reach: the command line sends 32 of the 62 message kinds
(`crates/paged-cli/tests/cli_surface.rs`), and one operation has no `paged.*` function
(`crates/paged-script/tests/script_surface.rs`).

**Doors for plugin hosts.** A host can submit vector, text and image content to be drawn
inside a frame (`SubmitSceneLayer`, [ADR 013](adr/013-in-frame-scenelayer.md)), stream
raster tiles (`SubmitPixelLayer`), and serve image pyramid tiles on request
(`ClaimImageResource`, `SubmitResourceTiles`). All of it is lowered to ordinary display-list
commands ([ADR 108](adr/108-plugin-raster-tiles.md)). A shared GPU texture is not offered
([ADR 018](adr/018-stage-b-gpu-texture-defer-record-only.md)).

## Where a document is stored

In memory, `CanvasModel` owns the scene, the built pages, the source archive, an overlay of
container parts and the undo log. Fonts and colour profiles are registered on `WorkerCore`
and survive a new `LoadDocument`. Submitted scene layers are session state, not content.

On disk a `.paged` file is a ZIP that is still a valid IDML package, with extra parts under
`paged/` and a `manifest.json` ([ADR 118](adr/118-paged-file-is-a-valid-idml-package.md)).
Parts are read, listed and written through `ReadPagedPart`, `ListPagedParts` and
`WritePagedPart`. A write must be under `paged/`, and under `paged/<caller>/` when the
message names a caller; without a caller only the prefix is checked, and the code comment
calls the check an aid for correct callers, not a security boundary
(`crates/paged-canvas/src/model.rs:4578-4593`). `ExportPaged` embeds a fresh model part,
`paged/core/model/document.pgm` (JSON, `PGM_FORMAT_VERSION` 3), and hands the IDML parts
and the overlay to `idml_export::write_paged` in the plugin-publish repository.
`paged_store::package::wrap_document` writes documents that never were IDML: the model part
plus a one-page IDML skeleton, parsed only if the model part cannot be decoded.
`ExportIdml` writes plain IDML through the same external writer and lists what the format
could not carry ([ADR 124](adr/124-opacity-masks-native.md)).

## Releases and the boundary to other repositories

Nothing in this repository is consumed as source by the editor. A tag `v0.<protocol>.<patch>`
runs `.github/workflows/publish-wasm.yml`, which builds each wasm crate with
`wasm-bindgen --target web` and `wasm-opt -Oz` and publishes four npm packages at the tag's
version: `@paged-media/canvas-wasm`, `@paged-media/introspect-wasm` (with `catalog.json`
beside the wasm), `@paged-media/sdk` and `@paged-media/idml-viewer` (with the SDK wasm
copied in). Only the last has a `package.json` in the tree; the workflow writes the other
three. CI fails a push to `main` that changes `PROTOCOL_VERSION` without the matching tag
([ADR 006](adr/006-protocol-coupled-versioning.md)). The consuming half is the editor's
[ADR 200](https://github.com/paged-media/editor/blob/main/docs/adr/200-engine-as-npm-wasm-packages.md)
and [ADR 202](https://github.com/paged-media/editor/blob/main/docs/adr/202-render-worker-owns-the-canvas.md).

## Build and test

- Locally: `cargo build --workspace`, `cargo test --workspace`, clippy with `-D warnings`,
  `cargo fmt --all --check`; `make verify` runs the lanes and prints a table. Several tests
  read generated fixtures that are not committed: run `bash scripts/regen-fixtures.sh` first.
- `.github/workflows/ci.yml`: format, clippy, a font-hash check, tests on Ubuntu and macOS,
  a nextest run uploaded as JUnit, the wasm tests (`make test-wasm`), a size gate on
  `spikes/wasm-size` (3.5 MB brotli), the protocol tag guard, and the PDF export-diff gate,
  which compares a poppler raster of the exported PDF with the engine's own CPU render.
- `.github/workflows/fidelity.yml`: the fidelity gate. `corpus/generated/diff.sh`
  regenerates each fixture with `paged-gen`, renders it on the CPU at 144 dpi, rasterises
  the committed InDesign PDF with `pdftoppm`, and compares mean ΔE2000, p99 ΔE2000 and SSIM
  with the fixture's thresholds in `corpus/generated/fidelity-thresholds.json` (58 fixtures)
  ([ADR 105](adr/105-fidelity-gate.md), [ADR 120](adr/120-indesign-is-the-oracle.md)).
- `gpu.yml` runs the Vello tests of `paged-gpu` on software Vulkan; `licenses.yml` runs
  `cargo deny`. No gate needs a private repository ([ADR 121](adr/121-public-repo-builds-alone.md)).
