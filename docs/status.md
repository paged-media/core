# Status

What the engine ships and what it does not, read from the code of the protocol-71 batch
(`PROTOCOL_VERSION` 71: the object model — one address grammar and typed property schemas —
on top of `v0.70.0`, the presentation batch and the platform doors for plugin content). This page stays
at the level of the engine and its surfaces; it does not list which document constructs are
rendered. How the parts fit is in [`architecture.md`](architecture.md).

## Shipped

- **Four npm packages per release tag.** `@paged-media/canvas-wasm`,
  `@paged-media/introspect-wasm`, `@paged-media/sdk` and `@paged-media/idml-viewer`, built
  and published by `.github/workflows/publish-wasm.yml`.
- **Open and create.** `LoadDocument` opens an `.idml` package or a `.paged` container;
  `NewBlankDocument` and `paged new` make an empty document.
- **Render.** One display list per page, drawn on the CPU (`paged render`, snapshots) or
  with Vello on WebGPU (`presentFrame` in the editor wasm, `ViewerSession` in the viewer).
- **Edit.** 130 mutation operations over the wire, batches that are atomic and cost one
  rebuild and one undo step, undo and redo, and engine-minted ids reported in the reply.
- **Interaction.** Hit testing, element and text selection, caret and selection geometry,
  gestures (rotate, scale and shear
  about a given point) and snapping, as message kinds. Snapping is one engine resolver
  ([ADR 125](adr/125-snapping-lives-in-the-engine.md)): `requestSnapPoint` and the move, resize
  and path-edit gestures snap to every visible element's points and outlines, the page, ruler
  guides, the grid and the lines through those points, under `setSnapSettings`.
- **Plugin fields and labels.** Placeholder fields are placed at a story offset or at the caret
  (`insertField.contentOffset`). Text typed at a field's edge lands beside it. A
  document-scoped, undoable plugin label is written with `setDocumentMetadata` and read in
  `DocumentMeta` ([ADR 127](adr/127-fields-and-document-labels-for-data.md), protocol 69); the
  IDML adapter carries it as the designmap `<Document>`'s label.
- **A drop shadow on every shape.** The `frameDropShadow*` paths apply to text frames,
  rectangles, ovals, polygons and lines (a line's shadow is cast by its stroke), and IDML carries
  them ([ADR 130](adr/130-plugin-faces-shadows-and-batch-handles.md), protocol 70).
- **Pages in a batch.** `bindCreated` names a page that `insertPage` or `duplicatePage` minted, so
  pages and their content are one batch and one undo step. Every batch lane reports each mint's
  handle in `minted[].handle` ([ADR 130](adr/130-plugin-faces-shadows-and-batch-handles.md)). A duplicated page owns copies of its
  stories, including their hyperlink sources, and keeps its margins. An inserted page takes its
  master's margins. `duplicateElements` copies a story that holds hyperlink sources
  ([ADR 128](adr/128-pages-a-merge-can-make.md), protocol 69).
- **Pages and masters for presentations.** `movePage` reorders pages. A page carries plugin
  labels that travel with it (`setPageMetadata`), and a text range links to a page
  (`insertHyperlink.page`). A snapshot can leave items out (`requestSnapshot.hideItems`).
  `onMaster` edits a master's items with the ordinary mutations, and `createMaster`,
  `deleteMaster` and `renameMaster` make, remove and name masters. A radial gradient is centred
  at its `GradientFillStart`, and a cell edge draws in its stroke style (a double border as two
  rules) ([ADR 129](adr/129-pages-and-masters-for-presentations.md), protocol 70).
- **One object model.** Every object has an address (`doc`, `page:#2`, `swatch:Color/Red`,
  `style:object/<id>`, `cell:<table>/0,1`, `textVariable:<id>`, …) and selectors find them
  (`page:#3 > frame[name="price"]`, `RequestQuery`, `paged.query`, `paged query`)
  ([ADR 131](adr/131-one-address-grammar-and-selectors.md)). Every property has a typed schema
  in the catalog's `propertySchemas`, and one `set` writes any of them by (address, path, value)
  through the operation that already owns it, with `RequestGet` as its read twin —
  `paged.set`/`paged.get`, `paged set`/`paged get`
  ([ADR 132](adr/132-typed-property-schemas-and-virtual-paths.md), protocol 71). Object, cell
  and table styles, page margins and columns, document bleed and slug, text-variable
  definitions, effects on lines and item names became settable; a custom text variable can be
  created and instanced in text, and one `set` switches every instance. A swatch is
  addressed by its id or its name. A `set` is always one undo step, colour settings and the
  other application state included.
- **Read.** Document collections, element properties, the scene tree (each item with its
  plugin metadata), layers, frame chains, story content, a text frame's glyphs as outlines,
  colour previews and ink coverage as message kinds; most also in `paged read`.
- **Page growth.** A story with a grow rule (`setFlowGrowRule`) gets generated pages until
  it fits or reaches its page cap.
- **Scripting.** `ExecuteScript` and `paged script` run JavaScript against the document
  under a budget. Every operation but one (`BindCreated`, legal only inside a batch) has a
  `paged.*` function; `paged.batch` accepts raw operations. With `ExecuteScript.hostObjects`
  a script reaches plugin objects (`plugin:` addresses): it suspends with
  `RequestHostObjects`, the host answers with `HostObjectsResult`, and the script resumes
  (`await` works; the Boa clock is wasm-safe). Its writes undo as one step that holds only
  the script's own records and the host commits it reports in `committed`.
- **Export.** IDML (with a list of what the format could not carry), `.paged`, and PDF
  through an export session; `paged export --format idml|paged|pdf`.
- **Container parts and plugin content.** Read, list, write and delete parts under
  `paged/` (`paged parts`); scene layers, pixel layers and pulled image tiles drawn inside a frame.
  A scene layer's text run draws in the face it names, resolved through the registered fonts;
  a family that does not resolve draws in the default font and is reported per frame
  ([ADR 126](adr/126-scene-text-in-its-own-face.md), protocol 68).
  A face registered with `scope: "sceneLayer"` serves scene-layer text only and never the
  document's layout, missing-font report or substitution trace
  ([ADR 130](adr/130-plugin-faces-shadows-and-batch-handles.md), protocol 70).
- **Viewer.** `ViewerSession` (load, layout, present, RGBA readback) and the TypeScript
  wrapper with camera, input and events.
- **Command line.** `paged` with sixteen top-level subcommands, the line-delimited JSON
  session (`paged session`, `paged-run`), and the binaries `paged-inspect`, `paged-diff`,
  `paged-gen` and `paged-export`.
- **Capability catalog.** `paged describe`, `describeCatalog()` and the committed
  `crates/paged-introspect/catalog.json`: 158 host functions, 221 settable paths, 394
  property schemas, 130 operations, and the 30 address forms of `idGrammar`.
- **Fidelity tooling.** The fixture generator (60 samples), the image-diff tool, 59
  reference PDFs with thresholds, and the scripts that drive InDesign to export them.

## Limits of what is shipped

- **WebGPU is required in the browser.** The viewer rejects when `navigator.gpu` is absent
  and has no CPU page rasteriser. In the editor wasm `presentFrame` returns `false` without a
  GPU surface; what happens then is the host's choice.
- **No font and no colour profile is bundled.** Text for which neither a registered font
  nor a default font resolves is not shaped. Without a CMYK profile, CMYK is converted by a
  naive formula.
- **A commit rebuilds the whole document.** Caches make unchanged paragraphs and stories
  cheap, and only a build of text edits in one story reuses previous pages.
- **Two mutation lanes.** Text edits have no `Operation` form; they use `TextOp` in
  `paged-canvas`. Undo keeps the newest 10,000 entries.
- **The command line sends 37 of the 67 message kinds.** The rest are listed with a reason
  each in `crates/paged-cli/tests/cli_surface.rs`; half of them (15 of 30) need a pointer or a caret.
- **The Vello path is not at parity with the CPU path.** Ink separations and coverage are
  CPU only; the `DropShadow` command is skipped and a path shadow is approximated by
  stamped fills (`crates/paged-gpu/src/vello_rs.rs:1039-1059`).
- **The viewer reads IDML parts only.** `ViewerSession::load` calls the IDML importer and
  does not read the native model part of a `.paged` container. It has no write surface.
- **A `.paged` save carries the document twice**: the IDML parts and the model part, which
  is one JSON document (`crates/paged-canvas/src/model.rs:5122-5125`). A model part with
  another format version is ignored on load, and the IDML parts are parsed instead.
- **Some object-model state does not reach an IDML save yet.** The IDML adapter (in the
  plugin-publish repository) writes neither a page item's `Visible="false"` when it was hidden
  in paged, nor a text variable authored in paged (its definition or its instances), nor an item
  name set in paged; the engine model keeps all three. The first two are pinned by tests in
  `crates/paged-canvas/tests/idml_export_object_model_state.rs` that turns red when the adapter
  is fixed.
- **A cell's fill tint is not modelled.** `cellFillTint` reads `null` on every cell and its
  write is accepted but not stored, rendered or saved; the cell keeps its fill at 100 %.
  It needs a `TableCell` field and the IDML adapter's parse and emit (ADR 132).
- **A host that does not send `HostObjectsResult.committed`** gets its plugin write as an
  undo step of its own, separate from the script's core writes.
- **Script wall-clock limits are checked at host-function calls**, not inside a loop that
  makes none; such a loop is bounded by the iteration limit.
- **The fidelity gate needs the reference colour profile.** On a runner without a FOGRA39
  profile a failing fixture ends the run "inconclusive" (exit 3), which the workflow reports
  as a warning (`.github/workflows/fidelity.yml:98-121`). In the PDF export-diff gate three fixtures
  (`effects`, `footnotes`, `swatches`) are advisory.
- **The size gate measures a stand-in.** `spikes/wasm-size` links the viewer crate and
  wgpu 22; the size of the published viewer wasm is printed at release, not enforced.

## Releasing protocol 71

Not tagged yet. The order, because core and the IDML adapter pin each other:

1. Tag core `v0.71.0` from this batch (the tag guard checks `PROTOCOL_VERSION` 71); the
   publish workflow ships the four packages.
2. In plugin-publish, merge the protocol-71 export fixes (`Visible`/`Locked`, text variables
   authored in paged, the text-path `EndBracket`, item names always written), re-pin its core
   git dependency to the `v0.71.0` commit, and push.
3. In core, re-pin `idml-import`/`idml-export` to that plugin-publish commit (both `Cargo.toml`
   pins and `Cargo.lock`), flip `crates/paged-canvas/tests/idml_export_object_model_state.rs`
   to expect the state in the save, update the "does not reach an IDML save" limit above, and
   tag `v0.71.1` (patch: no wire change).
4. plugin-sdk re-syncs the vendored wire and catalog and bumps its contract; the editor moves
   its pins and sends `HostObjectsResult.committed`.

## Not built

- A retained layout tree or demand-driven recomputation. Every build is the whole-document
  build with caller-owned caches ([ADR 107](adr/107-whole-document-build.md)); no manifest
  depends on `salsa`, which `crates/paged-canvas/src/lib.rs` still names as a later phase.
- Rendering a document from the composition model. `paged-composition` is derived from the
  scene, the function that stores it as a part is called only by tests, and
  `paged-composition-render` (single page, page-relative positions) has no dependents.
- `TextFlow`, the region-chain form of story layout, as the live path: per
  `crates/paged-renderer/src/flow.rs:27-29` it does not replace `StoryEmitter`.
- A shared GPU device or texture for plugins
  ([ADR 018](adr/018-stage-b-gpu-texture-defer-record-only.md)).
- A CPU or WebGL fallback in the viewer, and any editing in it.
- A raw-JSON subcommand on the command line; `paged session` is the general door.
- A digest comparison between the browser wasm and the native build; the equivalence test
  runs natively (`crates/paged-sdk/tests/digest_equivalence.rs:20-22`).
- A bundled typeface or ICC profile; crates on a Rust registry (`publish = false`).
