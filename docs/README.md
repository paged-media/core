# Documentation

What this folder holds.

- [`concept.md`](concept.md): why the engine exists, what it is for, and what it will never do.
- [`architecture.md`](architecture.md): how it is built. The crates by role, the path from
  document bytes to pixels, the mutation path, the surfaces (wasm packages, viewer, command
  line, scripting), where a document is stored, releases, and the test gates.
- [`status.md`](status.md): what ships today, the limits of what ships, and what is not built.
- [`adr/`](adr/README.md): the decision records of this repository: fourteen that predate
  the numbering scheme (001 to 028) and 100–124.

Reference:

- [`reference/paged-file-format.md`](reference/paged-file-format.md): the `.paged` container
  format.
- [`reference/composition-format.md`](reference/composition-format.md): the composition
  part, `document.pgd`. Partly implemented; status notes mark which sections are.
- [`reference/deferred-scope.md`](reference/deferred-scope.md): a register of capabilities
  that were deliberately left out, each with its reason.
- [`reference/protocol-governance.md`](reference/protocol-governance.md): when the wire
  protocol number is bumped, and how it relates to package and plugin-API versions.
- [`reference/release-flow.md`](reference/release-flow.md): an engine release, step by step.

Design notes for single mechanisms. Each describes intent at the time it was written;
status notes inside say where the code went another way.

- [`design/canvas.md`](design/canvas.md): the canvas concept behind `paged-canvas`.
- [`design/canvas-interaction.md`](design/canvas-interaction.md): selection, gestures and
  transforms; the two sections that the source code cites by number.
- [`design/colour-and-swatches.md`](design/colour-and-swatches.md): colour management and
  swatches.
- [`design/pdf-export.md`](design/pdf-export.md): PDF export.
- [`design/incremental-flow-plan.md`](design/incremental-flow-plan.md): the measured steps
  behind [ADR 027](adr/027-incremental-flow-invalidation.md).
- [`design/plugin-metadata-and-baking.md`](design/plugin-metadata-and-baking.md): plugin
  metadata on page items, and baking plugin content to native items.
- [`design/image-resource-provider.md`](design/image-resource-provider.md): pulled image
  tiles, the design behind [ADR 108](adr/108-plugin-raster-tiles.md).
- [`design/idml-viewer.md`](design/idml-viewer.md): the viewer package; see
  [ADR 112](adr/112-viewer-sdk-is-a-sibling.md) and
  [ADR 123](adr/123-viewer-ships-from-core.md) for what was built.

## Decisions in other repositories that bind this one

These records live in other public paged-media repositories. The code here rests on each of
them. The last column says what the decision means for this repository.

| ADR | Repository | Decision | What it means here |
|---|---|---|---|
| [022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md) | plugin-publish | IDML relocates to plugin-publish; the model self-owns natively | There is no IDML parser or writer in this tree. `idml-import` and `idml-export` are git dependencies (for example `crates/paged-canvas/Cargo.toml`), and the model persists itself through `crates/paged-store`. |
| [650](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/650-mutual-git-revision-pins.md) | plugin-publish | The IDML adapter and the engine pin each other by git revision | Every crate that needs the adapter names the same commit. The root `Cargo.toml` carries a `[patch]` that points the adapter's copies of `paged-model`, `paged-scene`, `paged-flow` and `paged-composition` at the local crates, and `deny.toml` allows the git source. Taking a newer adapter is a pin change here. |
| [651](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/651-idml-compiled-into-engine-wasm.md) | plugin-publish | IDML is compiled into the engine wasm; the bundle is a registration shim | `paged-canvas` links both adapter crates, so the editor wasm opens and writes `.idml` itself (`LoadDocument`, `ExportIdml`). `paged-sdk` links `idml-import` for the viewer. |
| [652](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/652-idml-save-back-patches.md) | plugin-publish | IDML save-back patches the source package | `CanvasModel` keeps the bytes it was loaded from and hands them to the writer on every export. A blank document is therefore created as a minimal IDML package and loaded through the normal path (`crates/paged-canvas/src/blank.rs`). |
| [010](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/010-raw-mutate-gate-capability-enforcement.md) | plugin-sdk | The raw-mutate gate and the capability enforcement line | The engine carries the engine-side half. A plugin metadata key must be `x-paged:<plugin>`, and when the request names its caller it must be that caller's own (`crates/paged-mutate/src/apply/layer.rs`). Container-part writes and scene-layer submissions take the same optional `caller` (`crates/paged-canvas/src/channel.rs`). A request without a caller is not checked against an owner. |
| [302](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/302-vendored-wire-types.md) | plugin-sdk | Engine wire types are vendored and re-synced on each protocol bump | The TypeScript declarations generated from the `tsify` derives, and `crates/paged-introspect/catalog.json`, are published inside the npm packages because the plugin contract copies them from there (`.github/workflows/publish-wasm.yml`). A wire change reaches plugins only through a tagged release. |
| [200](https://github.com/paged-media/editor/blob/main/docs/adr/200-engine-as-npm-wasm-packages.md) | editor | The engine is consumed as published npm wasm packages | This repository builds and publishes those packages from a tag. Three of the four package manifests exist only in the release workflow. |
| [202](https://github.com/paged-media/editor/blob/main/docs/adr/202-render-worker-owns-the-canvas.md) | editor | The render worker owns the canvas; main thread and engine talk over a sequenced channel and shared memory | The engine half is here: `CanvasWorker` takes an `OffscreenCanvas` in `initGpu`, every envelope carries a `seq`, and the camera and gesture `SharedArrayBuffer` layouts are defined in Rust and exported (`cameraSabLayout`, `gestureSabLayout`). See [ADR 115](adr/115-worker-boundary-transports.md). |
