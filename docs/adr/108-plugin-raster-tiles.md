# ADR 108 — Plugin raster content enters as tiles through the ordinary image lane

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-renderer/src/resource_provider.rs`, `crates/paged-compose` (`scene_layer.rs`, `pixel_layer.rs`), `crates/paged-canvas` (`resource_tiles.rs`, the channel kinds), `crates/paged-canvas-wasm/src/dispatch.rs`

## Context

A plugin can draw inside a frame through a scene layer ([ADR 013](013-in-frame-scenelayer.md)).
For pixels, the first form was one `SceneItem::Image`: a whole-frame RGBA8 buffer, sent
again on every commit. Two cases outgrew it, and the module docs say which.

An interactive drag needs finer granularity: "a slider drag re-streams only the dirtied
tiles onto the frame, not the whole image, every frame"
(`crates/paged-compose/src/pixel_layer.rs:27-32`). And the native image lane is
whole-image, "one `DecodedImage` per placed asset", while a plugin that owns a tiled mip
pyramid can serve any window of a large composition, "but core had no seam to ASK for
tiles" (`crates/paged-renderer/src/resource_provider.rs:17-23`).

A door that hands the engine a GPU texture is deferred
([ADR 018](018-stage-b-gpu-texture-defer-record-only.md)). In the crates, `GPUTexture`
occurs only in three doc comments that call it a later stage.

## Decision

Plugin pixels reach the page as CPU RGBA8 rectangles, and every form is lowered to the same
`DisplayCommand::Image` that placed assets use. No rasteriser path was added for them.

- **Push, whole buffer.** `SubmitSceneLayer` with a `SceneItem::Image`.
- **Push, tiles.** `SubmitPixelLayer` carries a `PixelLayer`: a set of independently
  positioned tiles. `PixelLayer::into_scene_layer` turns each tile into a
  `SceneItem::Image`, and the result is stored in the same per-frame registry as a scene
  layer. `ClearPixelLayer` clears either.
- **Pull, pyramid tiles.** `ClaimImageResource` registers a claim for a frame (id
  namespace `x-paged-image:<frame>`) with the pyramid's level count, tile size and base
  extent. During a build the renderer asks an `ImageResourceProvider` for the tiles of one
  mip level and emits those it gets. Tiles it does not get are listed in a
  `ResourceTilesNeeded` record; the build does not wait. The host answers with
  `SubmitResourceTiles`, whose `generation` must equal the claim's revision or the reply
  is dropped.
- On the worker the provider is `ResourceTileStore`: a least-recently-used cache with a
  default budget of 128 MiB (its tuning is marked a follow-up), where a read during a build
  also refreshes a tile's recency.
- Claims, tiles and layers are render-time state on the `CanvasModel`. They are not
  document mutations and not undo entries. The document change that keeps a result is a
  separate mutation, `ReplaceImageBytes`.

## Evidence

- `crates/paged-renderer/src/resource_provider.rs:15-37`, `:43-62` — the pull contract ("compose NEVER blocks on a tile fetch") and the trait
- `crates/paged-renderer/src/resource_provider.rs:182-236` — `assemble_resource_tiles`: one `DisplayCommand::Image` per returned tile, missing origins returned
- `crates/paged-renderer/src/pipeline/text_frame.rs:1448-1508`, `crates/paged-renderer/src/pipeline/build_engine.rs:1565-1577` — the per-frame call, after the frame's native content
- `crates/paged-compose/src/pixel_layer.rs:15-37`, `:88-112` — `PixelLayer` and its lowering
- `crates/paged-compose/src/scene_layer.rs:76-87`, `:593-632` — `SceneItem::Image` and its lowering to `DisplayCommand::Image`
- `crates/paged-canvas/src/channel.rs:258-277`, `:1059-1112` — the wire kinds for pixel layers, claims and tile submits
- `crates/paged-canvas/src/resource_tiles.rs:15-41`, `:160-173` — the tile store, its budget, and the stale-reply guard
- `plugin-sdk: packages/plugin-api/src/host.ts:533-574` — the plugin-facing `host.images.claimImageResource`

## Alternatives considered

- One whole-frame buffer per update: kept for commits; the pixel-layer doc gives the reason
  it is not used during a drag (quoted above).
- A shared GPU device and plugin textures: deferred, see ADR 018.
- A blocking tile fetch: excluded by the provider contract; the trait returns `None` for a
  tile the provider does not have "*yet*" (`crates/paged-renderer/src/resource_provider.rs:52-56`).

## Consequences

Plugin pixels pass through both rasterisers and the PDF backend like any placed image, and
cost a CPU copy per tile and per build (`rgba.to_vec()` into the display list's image pool).

Three parts of the pull lane are less complete than the comments say:

- The mip level is chosen from `PipelineOptions::render_scale`. The model's value starts at
  1.0 and changes only through `CanvasModel::set_resource_render_scale`, whose comment says
  the dispatcher pushes the camera scale. No caller exists outside
  `crates/paged-canvas/tests/resource_provider.rs`.
- The module doc says a miss "assembles whatever coarser level IS cached", and the wire
  note says "the best cached level". `emit_frame_resource_tiles` assembles the chosen level
  only; where a tile is missing, what the frame drew before the tiles stays visible
  (`crates/paged-renderer/src/resource_provider.rs:173-177`).
- The wire declares an unsolicited `ResourceTilesNeeded` message. The dispatcher never
  posts it; the missing tiles travel in the `needed` field of the `ResourceClaimApplied`
  reply to a claim or a submit (`crates/paged-canvas-wasm/src/dispatch.rs:858-951`).

## Related

- [ADR 013](013-in-frame-scenelayer.md) — the scene layer these lanes extend
- [ADR 018](018-stage-b-gpu-texture-defer-record-only.md) — the GPU texture door, deferred
- [ADR 459](https://github.com/paged-media/plugin-image/blob/main/docs/adr/459-scene-layer-image-and-tiles.md) — the consumer side in plugin-image
