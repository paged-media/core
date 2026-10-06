# Design — C-6 renderer resource provider (pyramid tiles · I-06)

*Status note (2026-10-02): this design is built (`crates/paged-renderer/src/resource_provider.rs`, wire protocol 44); [ADR 108](../adr/108-plugin-raster-tiles.md) records the decision and what the code does today.*

**2026-06-12 · design note · status: FROZEN for the v44 wire batch.**
Companion to an internal note on the scene-layer texture and placed-asset
doors (Stage A/C-5 landed v41/v42; Stage B deferred to the v45 batch, see
[ADR 018](../adr/018-stage-b-gpu-texture-defer-record-only.md)). One
shared-device contract, two uses: this note is the *tile* use; Stage B is
the *viewport texture* use.

## The gap (restated from the internal gap register)

The renderer's image lane is whole-image: `DisplayCommand::Image` over a
`DecodedImage` pushed into the display list, decoded once, no
level-of-detail. paged.image's Engine B holds a *tiled mip pyramid*
(image-graph: NodeCache + Damage + mip-aware evaluation) that can serve
any `(level, x, y)` window of a 100+ MP composition — but core has no
seam to ASK for tiles, so the plugin must flatten to one RGBA buffer
(Stage A) at whatever resolution it guesses the viewport wants. That
caps quality and wastes memory exactly when the document is large.

## Decision shape

**A pull seam, renderer→plugin, keyed by image id — not a push channel.**
The renderer knows the visible rect + scale at composite time; the plugin
knows the pixels. So the provider is a *callback contract the host wires*,
not a plugin-initiated submit (push would re-invent damage tracking on the
wrong side of the wire).

### Core (paged-renderer / paged-compose)

```rust
// crates/paged-renderer/src/resource_provider.rs (new)
pub trait ImageResourceProvider {
    /// One tile of `image_id` at pyramid `level` (0 = full res; each level
    /// halves). Tile geometry is provider-owned; core treats tiles as
    /// opaque RGBA8 rects placed by `dest` (image-space px at `level`).
    fn tile(&self, image_id: &str, level: u8, x: u32, y: u32)
        -> Option<ProviderTile>;
    /// Monotonic revision per image — core re-pulls when it changes
    /// (the damage signal, same etag discipline as the data provider).
    fn revision(&self, image_id: &str) -> u64;
}
pub struct ProviderTile {
    pub rgba: Arc<[u8]>, pub width: u32, pub height: u32,
    pub dest: [u32; 2], // tile origin in level-space px
}
```

- `PipelineOptions` gains `resource_providers: HashMap<String, …>` keyed by
  the *provider-claimed* image id (`x-paged-image:<frame>` namespace), the
  same injection pattern as `scene_layers`.
- The renderer's image emit path checks the map first: claimed id → tile
  assembly at the level matching the current scale (mip pick =
  `floor(log2(1/scale))` clamped), else the existing `DecodedImage` lane.
  tiny-skia + Vello both consume the assembled tiles as ordinary
  `DisplayCommand::Image` entries — **no Vello fork** in v44; the zero-copy
  GPU tile path is Stage B's follow-on, same provider contract.

### Wire (paged-canvas, the v44 bump)

Because plugin wasm lives main-thread-side and the renderer in the worker,
the pull crosses the channel as a request/reply pair plus a claim message:

- `ClaimImageResource { image_id, levels, tile_size, revision }` /
  `ReleaseImageResource { image_id }` (main→worker): registers the claim in
  `CanvasModel` (the worker never *pulls* what nobody claimed).
- `ResourceTilesNeeded { image_id, level, tiles: Vec<[u32;2]>, generation }`
  (worker→main): emitted during compose when a claimed image lacks tiles at
  the chosen level (compose proceeds with the best cached level — never
  blocks).
- `SubmitResourceTiles { image_id, level, tiles: Vec<ProviderTileWire>,
  generation }` (main→worker): fills the worker-side tile cache (LRU,
  budgeted) and dirties the page.

This is an *async fill* pattern: first paint uses Stage A's whole-image
fallback (or a coarse level), tiles sharpen it. Deterministic for tests:
the headless harness drives `ResourceTilesNeeded` → `SubmitResourceTiles`
synchronously.

### SDK (plugin-api / plugin-sdk)

```ts
// host.images (new surface, gated on capabilities.rendering ∋ "resourceProvider")
claimImageResource(elementId: string, opts: { levels: number; tileSize: number;
  source: (level: number, x: number, y: number) => Promise<TileBytes | null>;
  revision: () => number }): Disposable;
```
`supports("rendering.resourceProvider@1")` probes it. The SDK owns the
needed→fetch→submit plumbing; the bundle supplies only the `source`
callback (paged.image points it at image-graph's evaluated tiles).

## Out of scope (pinned)

- GPU-resident tiles / external textures — Stage B (v45), same trait, the
  `ProviderTile` grows a texture-handle variant then.
- Non-image resources (fonts ride W-06; vector ride sceneLayer).
- Eviction policy beyond LRU-with-budget; revisit with real telemetry.

## Acceptance

Headless: claim → compose emits `ResourceTilesNeeded` → submit → re-compose
consumes tiles at the right level → release restores the fallback lane.
Editor: a 50 MP fixture pans/zooms with tile sharpening, memory stays under
the budget, tiny-skia/Vello parity on the assembled output.
