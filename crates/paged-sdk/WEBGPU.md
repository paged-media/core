<!--
  Design note for the renderer roadmap. Part of paged (https://paged.media),
  dual-licensed MPL-2.0 OR PMEL like the rest of this crate. This is a target
  spec, not yet implemented — see "Status" below.
-->

# `paged-sdk` → WebGPU renderer-core session

**Status:** implemented in `src/lib.rs` as `ViewerSession` (the legacy
`render_to_png`/`render_pages`/`parse_summary` free functions are gone).
CPU/tiny-skia as a *page rasterizer* is excluded from this crate
(`paged-renderer` is linked `default-features = false`, dropping
`paged-gpu/cpu`); it stays only as core's *internal* headless CI fidelity
backend (`corpus/generated/diff.sh`). This note is the design spec.

**Deviations from the sketch below, as built:**
- `new()` is a static async method (`ViewerSession.new(): Promise<…>`), not a
  JS `constructor` (wasm-bindgen constructors can't be async). It probes
  `navigator.gpu` for early rejection; the wgpu **device is acquired lazily**
  on the first `render_to_canvas` (bound to that canvas) rather than in
  `new()`, because `paged-gpu::SurfacePresenter` couples device+surface.
- `render_to_canvas` has a sibling `render_to_canvas_main(HtmlCanvasElement)`
  for main-thread embedders (both `SurfacePresenter::new`/`new_offscreen`).
- `render_to_bytes` returns raw RGBA via the new
  `SurfacePresenter::render_scene_to_rgba` (no `image` dep on this crate;
  for headless calls without a canvas it spins up a throwaway 1×1
  `OffscreenCanvas` purely for its device).
- **Known follow-up:** tiny-skia is still pulled transitively by `resvg`
  (build-time SVG asset decode in `paged-renderer`), independent of the
  forward Vello renderer. Removing it (to actually shrink the wasm) means
  making `resvg` optional in `paged-renderer` — deferred; size is currently
  not a constraint.

## What this crate is

`paged-sdk` is the **renderer-core-only** wasm surface: it links
`paged-renderer` + `paged-gpu` and **nothing from the editor** (no
`paged-mutate`, `paged-canvas`, `paged-script`). It is:

- the published SDK (`@paged-media/sdk`), and
- the engine the slim public viewer (`paged-media/viewer`) wraps, which is in
  turn the `<live preview>` embedded by the docs site (`paged-media/docs`).

It is distinct from `paged-canvas-wasm`, which is the editor's GPU surface and
*does* pull the mutation/canvas crates. Keeping `paged-sdk` renderer-core-only is
the whole point — it is the "sibling, not a shrunk app" boundary.

## Why the change

- **WebGPU-first, CPU is legacy.** All forward rendering is WebGPU (wgpu / Vello),
  the same GPU path the editor ships. No WebGL and no CPU fallback on the forward
  surface. When `navigator.gpu` is absent, the *consumer* (viewer/docs) shows a
  one-line "requires WebGPU" message — the SDK does not carry a second renderer.
- **Diagnostics as teaching.** Readers break the XML on purpose; the render call
  must return structured diagnostics alongside the result, not throw an opaque
  `JsError`, so consumers can surface parser/layout errors inline.

## Target API (wasm-bindgen, `#[cfg(target_arch = "wasm32")]`)

A small **session** object instead of one-shot free functions. Render is **async**
(GPU adapter + queue + readback are async) and gates on `navigator.gpu`.

```rust
#[wasm_bindgen]
pub struct ViewerSession { /* device, queue, presenter, built doc, current page */ }

#[wasm_bindgen]
impl ViewerSession {
    /// Acquire a WebGPU adapter/device. Rejects if `navigator.gpu` is absent or
    /// no adapter is available — the consumer maps that to the WebGPU-absent note.
    #[wasm_bindgen(constructor)]
    pub async fn new() -> Result<ViewerSession, JsError>;

    /// Parse + build a complete IDML package. Returns structured diagnostics
    /// (does NOT throw on recoverable parse/layout problems). `font` optional.
    pub fn load(&mut self, idml: &[u8], font: Option<Box<[u8]>>) -> Diagnostics;

    pub fn page_count(&self) -> u32;
    pub fn set_page(&mut self, index: u32);

    /// Render the current page to a bound OffscreenCanvas via WebGPU.
    /// Returns the diagnostics for this render (layout overset, missing refs…).
    pub async fn render_to_canvas(&mut self, canvas: web_sys::OffscreenCanvas) -> Diagnostics;

    /// Headless path: render the current page to an RGBA texture and read it
    /// back to bytes (replaces the legacy `render_to_png`). For screenshots/SSR.
    pub async fn render_to_bytes(&mut self, dpi: f32) -> Result<RenderedRaster, JsError>;

    pub fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32);
}

/// Structured, JSON-serializable (tsify) — NOT a JsError.
#[wasm_bindgen(getter_with_clone)]
pub struct Diagnostics {
    pub ok: bool,
    pub messages: Vec<Diagnostic>, // { severity: "error"|"warning"|"info", code, message, part?, line? }
}

#[wasm_bindgen(getter_with_clone)]
pub struct RenderedRaster { pub width: u32, pub height: u32, pub rgba: Vec<u8> }
```

TypeScript consumers see (the generated `.d.ts`; the viewer hand-writes a stub of
this until the wasm is built):

```ts
export class ViewerSession {
  static new(): Promise<ViewerSession>;
  load(idml: Uint8Array, font?: Uint8Array): Diagnostics;
  page_count(): number;
  set_page(index: number): void;
  render_to_canvas(canvas: OffscreenCanvas): Promise<Diagnostics>;
  render_to_bytes(dpi: number): Promise<RenderedRaster>;
  resize(width: number, height: number, device_pixel_ratio: number): void;
}
```

## Implementation notes

- **Reuse, don't reinvent.** `Document::open` + `pipeline::build_document` /
  `pipeline::render_document` already exist (see `src/lib.rs` today). The GPU
  presentation already exists in `paged-gpu` (`src/surface.rs` `SurfacePresenter`,
  Vello scene). `paged-canvas-wasm` is the working precedent for "compose →
  present to OffscreenCanvas via WebGPU" — copy its device/surface setup, drop the
  canvas/mutation layer.
- **Cargo:** enable `paged-gpu/vello-backend` (gated behind a `gpu` feature, the
  default for wasm builds). Add `wasm-bindgen-futures`, `web-sys` features
  (`Gpu`, `OffscreenCanvas`, `GpuCanvasContext`, …). Keep the `image` dep only if
  `render_to_bytes` encodes PNG; raw RGBA readback may not need it.
- **wasm size:** the 3.5 MB compressed budget (`spikes/wasm-size`, CI gate) still
  applies — Vello/wgpu is heavier than tiny-skia; measure.
- **Build/publish:** `core/.github/workflows/publish-wasm.yml` is the template
  (no-op without `NPM_TOKEN`). Interim: built from a sibling checkout by
  `viewer/build-wasm.sh` (pinned in `viewer/core.pin`).

## Consumers building against this today

Until this lands, `viewer/src/session.ts` is a **typed stub** mirroring the TS
shape above (its `render_*` reject with "WebGPU SDK not yet built"). That lets the
viewer app shell, package assembly, canvas wiring, and WebGPU-absent fallback be
developed and tested now; swapping the stub for the generated bindings is the only
change when the crate ships.
