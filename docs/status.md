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
- **Locks and deletes follow InDesign.** A locked item, and the content of a locked container,
  is not moved, resized, reshaped, restacked or deleted: the operation is refused with
  `OperationError::Locked`, and a gesture on it fails at `beginGesture`. Its attributes and its
  text still change, and unlocking it is an ordinary property write. Deleting a container deletes
  what is pasted into it, and a group goes with its last member. One undo restores either
  ([ADR 133](adr/133-locks-and-deletes-follow-indesign.md)).
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
  other application state included. `null` clears a nullable row (text, enum or reference)
  exactly as `""` does. A read tells a mixed value (`mixed: true`, a range whose runs
  disagree) from a refusal, which carries a code (`noSuchPath`, `notFound`, `badAddress`,
  `noDocument`). A frame's image content (`imageBytes`), its thread links
  (`nextTextFrame`, `previousTextFrame`), its opacity mask (`opacityMask`) and its path points
  (`points[i].anchor`, `.left`, `.right`, `.curveType`) are rows too, so a binding can write
  them; `null` unlinks, releases or clears.
- **Every operation on the command line.** `paged invoke <doc> <op> [args]` applies any
  operation the catalog lists with its wire arguments (inline JSON, a file, or stdin), and
  `paged ops` lists them. The vocabulary is the catalog's, so a new op is a verb without a CLI
  change; only `bindCreated` (legal only inside a batch) is not invoked on its own.
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
  the script's own records and the host commits it reports in `committed`. Typed commands
  (`core.<op>` and plugin commands) are reached the same way: `paged.invoke(id, args)`,
  `paged.commands(owner?)`, and `{ op: "invoke" }` ops in an object `paged.batch`.
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
  `crates/paged-introspect/catalog.json`: 161 host functions, 221 settable paths, 403
  property schemas, 130 operations, and the 30 address forms of `idGrammar`. 355 of the
  schema rows name the IDML element and attribute their value is read from (`idml`).
- **Nested styles and OpenType as InDesign sets them** ([ADR 134](adr/134-nested-styles-and-opentype-as-indesign-sets-them.md)).
  Nested styles read from InDesign's `<AllNestedStyles>` lists on a style or a range, cascade
  through `EmptyNestedStyles`, layer under a hand-applied character style, and range by the rules
  InDesign 20.0.1 answered (through and up to, words, sentences, repeat, a drop cap's style). A
  paragraph style carries `Ligatures`, `KerningMethod` and the `OTF*` toggles; the OpenType
  positions are the font's `sups` / `subs` / `numr` / `dnom` glyphs; All Caps asks for `case`
  and `cpsp`; historical forms and stylistic alternates shape.
- **Text wrap as InDesign sets it** ([ADR 139](adr/139-jump-object-and-next-column-wraps.md)).
  Bounding-box and contour wraps narrow the lines beside them, and those lines break by minimum
  raggedness, squeezing word spaces to keep a word. A jump-object wrap leaves its rows empty and
  text resumes below it on the leading grid; at a next-column wrap the rest of the story moves to
  the next frame. A contour wrap is written `Contour`, the value InDesign reads.
  Text stands on the side the wrap names (left, right, the larger area, towards or away from the
  spine), a contour wrap follows the item's outline grown by its offset and stroke, a frame's
  `IgnoreWrap` is honoured, and a master item's wrap acts on the pages that show it unless it
  applies to the master only ([ADR 151](adr/151-which-side-text-wraps-to.md)).
- **Line breaks as InDesign's Paragraph Composer sets them** ([ADR 141](adr/141-a-line-may-overrun-its-measure-by-a-sliver.md),
  [ADR 142](adr/142-ragged-text-breaks-by-minimum-raggedness.md)). Ragged text breaks by minimum
  raggedness in every frame; a line keeps its last word while it overruns its measure by about a
  99th of that word, and the overrun comes out of the word spaces. Justified text prices a line
  that shrinks past the minimum word spacing instead of forbidding it, and stretches as far as it
  must ([ADR 144](adr/144-justified-text-prices-over-shrunk-lines.md)): 99 of 160 probed
  paragraphs set every line as InDesign does. A drop cap spans the paragraph's leading at the
  face's cap height. 46 of the 73 generator fixtures set every line as InDesign does.
- **Align to baseline grid** ([ADR 140](adr/140-align-to-baseline-grid.md)). A paragraph aligned
  to the grid stands each line on the first grid line at or below it, on the document's grid or its
  frame's own, first line only where it says so; every baseline of the two grid fixtures matches
  InDesign's to 0.001 pt.
- **Positions are exact sums** ([ADR 138](adr/138-positions-are-exact-sums.md)). Baselines and
  glyph advances are summed exactly and rounded once, so a 14.4 pt leading or a 12 pt Open Sans
  line does not drift off InDesign's positions down a page or along a line.
- **Condition sets as InDesign records them.** A set keeps each condition's recorded visibility;
  applying it restores the listed conditions and leaves the others as they are. Bleed and slug
  set in paged reach an IDML save, with a `DocumentPreference` added to a package that had none.
- **Fidelity tooling.** The fixture generator (67 samples), the image-diff tool, 66
  reference PDFs with thresholds (`docx-pagination` has none and is not gated), and the
  scripts that drive InDesign to export them.

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
  `paged-canvas`. Undo keeps the newest 10,000 steps; a batch is one step at any size and in
  either lane.
- **The command line sends 39 of the 70 message kinds.** The rest are listed with a reason
  each in `crates/paged-cli/tests/cli_surface.rs`; half of them (15 of 31) need a pointer or a caret.
- **The Vello path is held to the CPU path, with known gaps.** `corpus/generated/parity-diff.sh`
  renders every generated fixture through both and budgets each in
  `corpus/generated/parity-thresholds.json` ([ADR 146](adr/146-the-gpu-lane-is-held-to-the-cpu-lane.md)).
  Ink separations and coverage are CPU only. A knockout group, and a non-isolated group below
  100 % opacity whose members use blend modes, are drawn as isolated groups. The blur of a
  `PushLayer` is a multi-tap approximation of the CPU's Gaussian.
- **The viewer reads IDML parts only.** `ViewerSession::load` calls the IDML importer and
  does not read the native model part of a `.paged` container. It has no write surface.
- **A `.paged` save carries the document twice**: the IDML parts and the model part, which
  is one JSON document (`crates/paged-canvas/src/model.rs:5122-5125`). A model part with
  another format version is ignored on load, and the IDML parts are parsed instead.
- **Hash and diamond stroke styles draw their marks with the default cap and join, and no gap
  colour.** The marks are laid out along the path as InDesign lays them out
  ([ADR 150](adr/150-hashes-and-diamonds-are-marks-along-the-path.md)); a frame's own `EndCap` /
  `EndJoin` and a gap colour under them are not measured yet.
- **Line and GREP styles are read and kept, not applied.** `<AllLineStyles>` and
  `<AllGREPStyles>` reach the model and the IDML export; the renderer sets no line or GREP style.
  End Nested Style Here (`<?ACE 3?>`) is not kept by the story reader, so a nested style that
  ends there runs on to its other limits.
- **An inverted text wrap wraps round its obstacle.** InDesign sets the band's text inside the
  shape; the composer cannot leave a row empty in mid-frame, which that needs
  ([ADR 151](adr/151-which-side-text-wraps-to.md)). A contour from an image uses the frame's path,
  and an object style's `IgnoreWrap` is not read.
- **A cell's fill tint is not modelled.** `cellFillTint` reads `null` on every cell and its
  write is accepted but not stored, rendered or saved; the cell keeps its fill at 100 %.
  It needs a `TableCell` field and the IDML adapter's parse and emit (ADR 132).
- **A host that does not send `HostObjectsResult.committed`** gets its plugin write as an
  undo step of its own, separate from the script's core writes.
- **An edit made while a script waits splits the script's undo step.** The fold merges only
  the script's own CONSECUTIVE records (and the host commits it reports). When a UI edit is
  logged during a host wait, the script's writes made before it stay one undo step and its
  writes after it another, with the UI edit between them: undoing the whole script then takes
  two steps, and undoing once takes back only the later part. Folding across the foreign
  record would swallow the UI edit into the script's step, which is worse.
- **A layer's lock is enforced by hit testing only.** A mutation addressed to an item on a locked
  layer applies; only the item's own lock, or its container's, is refused by the engine.
- **Script wall-clock limits are checked at host-function calls**, not inside a loop that
  makes none; such a loop is bounded by the iteration limit.
- **The fidelity gate needs the reference colour profile.** On a runner without a FOGRA39
  profile a failing fixture ends the run "inconclusive" (exit 3), which the workflow reports
  as a warning (`.github/workflows/fidelity.yml:98-121`). In the PDF export-diff gate three fixtures
  (`effects`, `footnotes`, `swatches`) are advisory.
- **The size gate measures a stand-in.** `spikes/wasm-size` links the viewer crate and
  wgpu 22; the size of the published viewer wasm is printed at release, not enforced.

## Releases

Protocol 71 shipped as `v0.71.0` through `v0.71.6` (npm `latest` 0.71.6). The release order
when core and the IDML adapter pin each other is in
[`reference/protocol-governance.md`](reference/protocol-governance.md).

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
