# ADR 100 — Two rasterisers behind one trait: Vello/WebGPU is the forward surface, tiny-skia the path of record

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-gpu`; the `cpu` and `gpu` features of `crates/paged-renderer`, `crates/paged-canvas`, `crates/paged-canvas-wasm` and `crates/paged-sdk`

## Context

The layout pipeline ends in a display list ([ADR 101](101-display-list-single-intermediate.md)).
It has to become pixels in two places: in a browser, drawn interactively onto a canvas, and
headless, where tests and the fidelity gate ([ADR 105](105-fidelity-gate.md)) need a raster
without a GPU.

`paged-gpu` states the purpose of its trait as letting "the pipeline swap between rasterizer
implementations (Vello, forked Vello, or a custom tile-based pipeline) without changing
callers" (`crates/paged-gpu/src/lib.rs:17-19`). The commit that introduced the two
implementations (`809f1aa`, 2026-04-26) says the pipeline and the fidelity harness "can pick
a backend per render without touching the display-list contract". For the browser,
`crates/paged-sdk/WEBGPU.md:50-53` records the rule "No WebGL and no CPU fallback on the
forward surface"; when `navigator.gpu` is absent the consumer shows a message and "the SDK
does not carry a second renderer".

The module doc says the choice of rasteriser "is driven by Spike A in `spikes/vello-eval`"
(`crates/paged-gpu/src/lib.rs:20`). That spike lists six cases and a pass criterion; its
`main` constructs a `vello::Scene` and ends in two `TODO` lines
(`spikes/vello-eval/src/main.rs:71-86`). Vello was chosen over the other candidates the
trait doc names. The repository does not record why.

## Decision

`paged-gpu` owns one small trait, `PathRasterizer` (`name`, and `rasterize` from a
`DisplayList` to an RGBA8 buffer), with two implementations selected by Cargo features:
`cpu` (tiny-skia 0.12, the crate default) and `vello-backend` (Vello 0.10 on wgpu 29).

- The two published wasm packages that draw pages, `@paged-media/canvas-wasm` and
  `@paged-media/sdk`, are built with `--features gpu` and draw the page with Vello onto a
  WebGPU surface through `SurfacePresenter`, which is compiled for `wasm32` only and
  presents without a readback. (The third published wasm package,
  `@paged-media/introspect-wasm`, is built without the feature and draws nothing.)
  `SurfacePresenter` is not an implementation of the trait; it uses the scene builder in
  `vello_rs.rs`.
- The viewer SDK links `paged-renderer` with `default-features = false`, which drops the CPU
  rasteriser. The editor wasm keeps it: `paged-canvas-wasm` depends on `paged-canvas` with
  its default `cpu` feature, which page snapshots and the ink-coverage reading use.
- The CPU rasteriser is "the path of record for the fidelity harness". It is the default of
  `paged-inspect --backend`, the lane the fidelity gate renders through, and the only
  rasteriser the headless `paged` session accepts.
- The trait's contract: a rasteriser that cannot draw a command skips it and does not fail
  the page.

## Evidence

- `crates/paged-gpu/src/lib.rs:163-181` — the trait ("log + skip it rather than fail the whole render")
- `crates/paged-gpu/Cargo.toml:13-22`, `:38-41` — `default = ["cpu"]`, the two features, the pinned `vello`, `tiny-skia` and `wgpu` versions
- `crates/paged-gpu/src/cpu.rs:51-59`, `crates/paged-gpu/src/vello_rs.rs:158-161` — the two implementations; `crates/paged-gpu/src/vello_rs.rs:96-99` — "path of record"
- `crates/paged-gpu/src/surface.rs:15-33` — `SurfacePresenter`: "directly on the browser canvas with no readback"; "wasm32-only by design"
- `.github/workflows/publish-wasm.yml:220`, `:276` — the two page-drawing wasm packages built with `--features gpu`; `:240` — `paged-introspect-wasm` built without it
- `crates/paged-sdk/Cargo.toml:27-32`, `crates/paged-canvas/Cargo.toml:13-18`, `crates/paged-canvas-wasm/Cargo.toml:21-24` — which wasm links the CPU rasteriser
- `crates/paged-renderer/src/bin/inspect.rs:199-205`, `crates/paged-cli/src/session.rs:466-476` — `cpu` is the default backend; the session refuses any other

## Alternatives considered

The trait doc names "Vello, forked Vello, or a custom tile-based pipeline"; only Vello was
built. `spikes/vello-eval`, meant to evaluate Vello against a reference PDF, was not
completed and still pins `vello = "0.3"` and `wgpu = "22"` (`spikes/vello-eval/Cargo.toml:21-22`).
WebGL and a CPU fallback on the SDK's surface are rejected in `crates/paged-sdk/WEBGPU.md:50-53`.

## Consequences

The viewer SDK needs WebGPU to draw a page. The editor wasm is not GPU-only: `initGpu`
documents that on failure "the worker stays on the CPU snapshot-blit fallback path"
(`crates/paged-canvas-wasm/src/lib.rs:400-405`). The wgpu version follows Vello's releases:
wgpu 30 is blocked on Vello (`crates/paged-gpu/Cargo.toml:30-32`).

Some results exist on the CPU lane only. Ink separations and total-area-coverage are read
from the CPU rasteriser's plane state (`crates/paged-gpu/src/lib.rs:86-91`); without the
`cpu` feature `ink_coverage` returns an empty list (`crates/paged-canvas/src/model.rs:7744-7752`).
On Vello, `DropShadow` is skipped and `PathShadow` is a multi-stamp approximation
(`crates/paged-gpu/src/vello_rs.rs:1039-1078`). The Vello lane is not measured against
InDesign: its tests run on a software Vulkan driver (`.github/workflows/gpu.yml:100-108`),
and `crates/paged-gpu/tests/vello_effects.rs` pins it against the CPU rasteriser.

Several comments are stale. `crates/paged-gpu/src/lib.rs:25-26` and
`crates/paged-cli/src/session.rs:63` call the Vello backend a stub; it is implemented, and
`paged-inspect --backend vello` renders through it when built with the `gpu` feature.
`crates/paged-gpu/Cargo.toml:15-16` says only one rasteriser may be enabled at a time;
`paged-inspect` built with `--features gpu` has both (`crates/paged-renderer/Cargo.toml:14`,
`:24-28`, `:33`), and so does the published editor wasm. `crates/paged-gpu/src/vello_rs.rs:54-64` lists
`BevelEmboss` as skipped; the code at `:1079-1099` paints it from the shared masks
([ADR 104](104-effects-follow-indesign-parameters.md)).

## Related

- [ADR 101](101-display-list-single-intermediate.md), [ADR 105](105-fidelity-gate.md) — the list both rasterisers consume; the gate that runs on the CPU lane
- [ADR 018](018-stage-b-gpu-texture-defer-record-only.md), [ADR 112](112-viewer-sdk-is-a-sibling.md) — the deferred plugin GPU texture door; the viewer SDK's dependency boundary
