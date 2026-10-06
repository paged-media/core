# ADR 018 — C-1 Stage B (shared GPUDevice + plugin GPUTexture): record-only deferral

**2026-06-13 · decision record · status: RATIFIED 2026-06-13 (with the v45 wire
batch). REFINED 2026-06-13 (see the dated
investigation section below): the ZERO-COPY Stage B is DEFERRED record-only,
but the realm-local "bless it" half LANDED — `capabilities.gpu: { realm:
"bundle" }` now legitimizes the bundle-realm WebGPU usage Engine-B already does.
Outcome: NO `SceneItem::Texture` variant, NO `requestGpuDevice` / shared-device
surface lands in code; the zero-copy composite door stays CLOSED (validate
rejects the reserved `realm: "shared"`) until the lift condition below is met.
Consequence: the v45 batch carries NO core wire change — the worker-spawn door
(worker spawn + SAB) shipped SDK+editor only, the realm-local `capabilities.gpu` is SDK-only,
and zero-copy Stage B is the batch's only other candidate, so PROTOCOL_VERSION
stays at 44.**

**Sources:** the internal Stage-A/B spike + design note ("the spike note" below: §2.3 the
two-door split, §4.1 verdict #3, §4.2 "Stage B (when it lands, NOT now)", §4.3
"Stage B (deferred — record only)", §5 "Why Stage A is honest (not a fake of
Stage B)"); the internal gap register, entries **C-1** (raw-GPUTexture/GPUDevice stages
remain), the viewport surface, and WebGPU reachability (the
bundle realm has no `navigator.gpu`); the image plugin's GPU-surface RFC, an internal
design note (the Engine-B interactive-viewport budgets);
[ADR 011](https://github.com/paged-media/plugin-web/blob/main/docs/adr/011-web-rendering-fork-defer-to-scenelayer.md) §4
(isolation contract wants alpha/heavy reach behind the plugin boundary),
[ADR 010](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/010-raw-mutate-gate-capability-enforcement.md)
(the real trust boundary is the isolate).

## The decision

**Stage A (RGBA bytes as a retained `SceneItem::Image`) shipped at core v0.41;
its C-6 pyramid-tile resource-provider extension shipped at v0.44 with a live
image consumer. Stage B — a host-blessed shared `GPUDevice` handed to the
bundle, the bundle rendering into a host-provided `GPUTexture`, and that
texture composited zero-copy into the Vello scene at interactive frame rates —
is NOT built. It is recorded here as a frozen contract and deferred. The `gpu`
capability stays REJECTED by the manifest validator (exactly as `assets ∋
"images"` was rejected until C-5's real read existed); no throwing SDK stub is
added, because a reserved member that pretends to a surface it cannot honor is
the honesty bug §5 of the spike note forbids.**

This ratifies the spike note's recommendation (#4: the image plugin's in-frame
rendering = C-5 + C-1 Stage A, Stage B deferred with the risk noted) as a decision, and records
*why a half-wired Stage B would be a fake*, so that later work does not
"finish the v45 wire" by adding a `SceneItem::Texture` the renderer cannot
composite.

## Why record-only, not wire-scaffolding

The plan framed Stage B as "wire scaffolding + honest
deferral." On inspection the scaffolding *is* the deferral: there is no honest
partial wire here, for three reasons the spike note already established.

1. **A wire variant the renderer can't honor is a fake.** Adding
   `SceneItem::Texture { handle, dest }` to the IR commits the lowering
   (`emit_scene_layer`) and both backends (Vello, tiny-skia) to *compose an
   externally-rendered texture* — a path Vello does not expose today (§2.3,
   §4.1 #3: "a Vello composite path for an externally-rendered texture that
   does not exist today"). A variant that lowers to nothing, or to a
   placeholder fill, would make `supports("rendering.sceneLayer@1")` report a
   capability the engine cannot deliver. The brand honesty rule (and §5's
   "Stage A is honest (not a fake of Stage B)") forbids it. The reserved-slot
   precedent is unambiguous: `assets ∋ "images"` was **rejected** by validate
   until C-5 shipped the real read — not accepted with a stub.

2. **Stage B crosses the trust line, and the trust line is moving.** Stage B
   hands the bundle a live `GPUDevice` (today the bundle realm has *no*
   `navigator.gpu` — `loadBundleWasm` grants no ambient authority). Blessing a
   shared device (`capabilities.gpu: { device: true }` + `requestGpuDevice`)
   is the heaviest possible reach across the boundary, in the exact direction
   ADR 010/011 push *against*: the real isolation boundary is the isolate
   migration, and a host-shared GPU device is precisely the kind of authority
   that should be designed *with* the isolate model, not retrofitted onto the
   in-process host shortly before a release.

3. **It is not headlessly testable, so it could not be verified when this was decided.**
   Every other wire item of that batch landed behind a CPU-lane test
   (tiny-skia pixels, or a headless dispatch round-trip). Stage B needs a real
   browser GPU adapter to test the composite at all (§2.3 row "Testable
   headlessly: no"). Landing unverifiable wire
   would violate the verification rule in force (cargo
   test + dispatch tests + fidelity gate for every change). The honest deliverable is the
   design + the deferral, not untested GPU plumbing.

## The frozen contract (for whoever lands it)

When the lift condition is met, Stage B lands as a *second* `SceneItem` kind on
the *same* sceneLayer rail Stage A rides — not a rework of Stage A
(forward-compat is the whole point of §5's closing paragraph):

- **Capability (manifest):** `capabilities.gpu: { device: true, memoryBudgetMiB:
  number }`. Closed vocabulary, validated by schema + CLI **only once the
  surface exists** (un-reject, mirroring the C-5 `"images"` un-reserve). The
  budget is host-enforced (the byte-budget discipline of the blob-storage quota
  and the large-artifact budget, applied to GPU
  memory).
- **SDK surface:** `host.requestGpuDevice(descriptor) → GPUDevice` — host-owned,
  host-budgeted, destroyable on dispose. Gated on `capabilities.gpu`. Absent a
  real editor GPU backend → it rejects honestly (the door-closed posture), it
  does **not** return a stub device.
- **IR (paged-compose):** `SceneItem::Texture { handle: TextureHandle, dest:
  [f32; 4] }` where `handle` references a host-registered texture the bundle
  rendered into. `emit_scene_layer` folds `content_outer` + the content-box
  clip exactly as for `Image`/`FillPath` — the plugin still never compensates
  for the frame transform.
- **Renderer:** the missing piece — a Vello external-texture composite path
  (`crates/paged-gpu/src/vello_rs.rs`). This is the open research item; co-design it
  with **C-6's** pyramid-tile provider so there is **one** shared-device
  contract serving **two** uses (the interactive viewport texture *and* the
  pyramid tiles), not two device-sharing mechanisms.
- **Wire:** rides the existing `SubmitSceneLayer { element_id, layer }` channel
  (a new `SceneItem` variant is a payload change, the v40-added-`Text`
  precedent) — *plus* a small device-handshake message pair for
  `requestGpuDevice`. That is the protocol bump Stage B will carry when it
  lands (call it the future texture batch); it is explicitly NOT v45.

## 2026-06-13 investigation — the two confirmed walls + the realm-local bless-it landed

A source-level investigation (against the pinned Vello v0.9.0 + the bundle
loader) confirmed precisely WHY the zero-copy Stage B cannot be built honestly
today, and separated out the buildable half that CAN.

### The TWO confirmed walls (zero-copy composite stays blocked)

1. **Vello has no external-texture import.** `peniko::ImageData` is BYTES-ONLY
   (`data: Blob<u8>` — a CPU byte blob, not a `wgpu::Texture` handle), and Vello
   uploads images into its OWN CPU-fed atlas at encode time. There is no API to
   hand Vello a bundle-rendered `GPUTexture` and have it composite that texture
   under an affine transform + clip. This is the §4.1 #3 gap, now pinned to the
   exact upstream type. A `SceneItem::Texture` would lower to nothing the
   renderer can honor.
2. **WebGPU can't share a `GPUDevice` across the realm boundary.** A `GPUDevice`
   (and its textures) is NOT transferable across the
   render-worker/main-thread realm boundary — the host's Vello device lives in
   one realm and the plugin's WebGPU work in another, and WebGPU exposes no
   cross-realm device/texture transfer. So a `requestGpuDevice` that hands the
   bundle the host's device cannot work; it would be a fake.

Either wall alone blocks zero-copy; both stand. So `requestGpuDevice` (device
sharing) and `SceneItem::Texture` (external composite) CANNOT be built honestly
today, and the deferral of the zero-copy composite STANDS unchanged.

### The realm-local "bless it" half LANDED

The gap register noted that paged.image's Engine-B ALREADY drives WebGPU from the bundle's
own JS realm (it works) — it was simply OUTSIDE the capability contract. That
realm-local usage needs NEITHER wall lifted (it never touches the host device or
the Vello scene; it draws in its own realm and, for in-frame display today,
re-submits RGBA bytes via Stage A / C-6). It is now BLESSED within the contract:

- **`capabilities.gpu: { realm: "bundle" }`** (plugin-sdk 0.2.20-canary —
  `GpuCapability` type, JSON schema, dependency-free CLI hand-mirror, the
  declaration-driven `gpu@1` feature flag). DECLARE-ONLY: it hands the bundle
  NO device (the bundle already has `navigator.gpu` in its realm); it lets the
  host surface "this plugin uses the GPU" to the user. paged.image's manifest
  declares it; `validate:manifest` is green.
- **`realm: "shared"` is RESERVED but REJECTED** by validation (mirroring how
  `assets ∋ "images"` was rejected until C-5's real read existed) — it reserves
  the shape for the future host-device-sharing path WITHOUT claiming a
  capability the host cannot honor. The CLI/schema reject it with a pointer to
  this ADR.
- **NO device surface added.** There is no `host.gpu` / `requestGpuDevice` /
  device/adapter/texture member on `BundleHost`; a plugin-sdk trust-line test
  asserts that absence (mirroring the secrets door's no-`get` keystone). Building one would
  be a fake while wall #2 stands.

This is the buildable, honest half of the Stage B item: the
realm-local capability landed; the zero-copy composite remains deferred
record-only by the walls above.

## The lift condition (when this ADR is superseded)

Note the SCOPE narrowed: only the ZERO-COPY composite (shared device +
`SceneItem::Texture`, i.e. a future `capabilities.gpu: { realm: "shared" }`)
remains under this deferral — the realm-local `{ realm: "bundle" }` already
landed (above). Zero-copy Stage B leaves record-only status when **BOTH** of the
walls lift AND the test harness exists — all three:

1. **Vello exposes (or core wraps) an external-texture composite path** that
   can draw a bundle-rendered `GPUTexture` into the page scene under an affine
   transform + clip — wall #1 closes (the `peniko::ImageData` bytes-only / CPU
   atlas limitation is gone). (Track upstream wgpu/Vello, or prototype the wrap
   in `spikes/`.)
2. **The cross-realm device-sharing limitation is resolved** — wall #2 closes.
   The likely shapes: run the plugin's GPU work IN the render worker (same realm
   as the Vello device), or transfer via `OffscreenCanvas` / a future WebGPU
   cross-realm handle. Until then `realm: "shared"` stays validation-rejected.
3. **A real-GPU test harness exists** in the editor's Playwright lane (a
   browser GPU adapter) so the composite can be asserted, not just compiled —
   the verification rule is satisfiable.

Until both hold, image's Engine-B *interactive* viewport stays on
the Stage-A re-submit-on-edit path (static quality, honest), and the in-frame
placed-image rendering that v1 actually needs is already live via Stage A
+ C-6. The worker decode pool (shipped alongside) parallelises the
*decode* that feeds Stage A; it does not depend on Stage B.

## Consequences

- **Stage B contributes NO wire change.** The worker-spawn door shipped SDK+editor only; the
  realm-local `capabilities.gpu` is SDK-only; the zero-copy composite is
  record-only. Stage B never bumped the protocol. (Protocol *did* reach **v45**
  — but for an unrelated feature, the C-1.3 sceneLayer **gradient** paint
  `FillPathGradient`, core v0.45.0, 2026-06-13. That v45 is gradients, not
  Stage B; the Stage-B texture variant remains the explicitly-NOT-this-batch
  "future texture batch" of §"The frozen contract".)
- **No honesty debt is added.** The realm-local `capabilities.gpu: { realm:
  "bundle" }` that LANDED is declare-only and the engine fully honors it (the
  bundle drives its own realm's WebGPU; nothing is faked). For the deferred
  ZERO-COPY path: no capability the validator accepts but the engine can't honor
  (`realm: "shared"` is rejected); no SDK member that returns a fake device (no
  `requestGpuDevice` exists — a trust-line test pins its absence); no IR variant
  that lowers to a placeholder. The zero-copy door is visibly closed with a
  pointer here.
- **Image's interactive-viewport milestone is explicitly deferred**, gated
  on the lift condition, tracked in the internal gap register under **C-1**
  (GPU-texture/GPUDevice stages) and its viewport-surface and WebGPU-reachability
  entries. Those rows stay OPEN with this ADR as their rationale —
  they are platform RFCs with a known owner, not dead ends.
- **The forward-compat promise is on the record:** Stage B is additive to Stage
  A's rail when it lands, so nothing shipped so far (Stage A, C-6,
  the decode pool) is rework risk.

## Amendment — 2026-10-02

Checked against core at `9f933f1` and plugin-sdk at `d90f727`. The deferral stands: the engine
has no texture item and no device door, and plugin pixels reach the page as CPU bytes.

**1. "The decision" contradicts the refined header; the code matches the header.** The
2026-06-13 investigation section supersedes two statements written before it: in "The
decision", "The `gpu` capability stays REJECTED by the manifest validator"; and in "The
frozen contract", the manifest shape `capabilities.gpu: { device: true, memoryBudgetMiB:
number }`. What the plugin contract does today:

- `plugin-sdk: packages/plugin-api/src/manifest.schema.json:183-195` — `gpu` is an object
  with one required key, `realm`, whose only schema value is `"bundle"`; other keys are not
  allowed.
- `plugin-sdk: packages/plugin-cli/bin/paged-plugin.mjs:46-47`, `:288-309` — the CLI accepts
  `"bundle"`, rejects `"shared"` as reserved with a pointer to this ADR, and rejects any
  other key.
- `plugin-sdk: packages/plugin-sdk/test/capability-manifest-cli.spec.ts:274-281` — a manifest
  with `gpu: { realm: "bundle", device: true }` is rejected.
- `plugin-sdk: packages/plugin-api/src/manifest.ts:225-230` — the type is
  `realm: "bundle" | "shared"`.
- `plugin-sdk: packages/plugin-sdk/src/host-impl.ts:1124`, `:3174-3182` — `supports("gpu@1")`
  reflects the declaration; no backend is wired.
- `plugin-sdk: packages/plugin-sdk/test/gpu.spec.ts:80-95` — the test that the host object has
  no `gpu` or `requestGpuDevice` member.

**2. The zero-copy path is still not built in the engine.** `SceneItem` has ten variants and
none is a texture (`crates/paged-compose/src/scene_layer.rs:54-210`). The names
`SceneItem::Texture`, `TextureHandle` and `requestGpuDevice` do not occur in this
repository. `GPUTexture` occurs in three doc comments, each calling it a later stage
(`crates/paged-compose/src/scene_layer.rs:31`, `crates/paged-compose/src/pixel_layer.rs:35`,
`crates/paged-renderer/src/resource_provider.rs:36`).

**3. Plugin raster content enters as CPU tiles.** Besides the two lanes this ADR names (the
whole-frame `SceneItem::Image` of Stage A and the C-6 pyramid-tile pull), protocol 50 added a
push lane for tiles: `SubmitPixelLayer` / `ClearPixelLayer`, lowered to `SceneItem::Image`
items in the same per-frame registry (`crates/paged-canvas/src/channel.rs:315-325`,
`:1059-1079`; `crates/paged-compose/src/pixel_layer.rs:88-112`). All three end as
`DisplayCommand::Image`; [ADR 108](108-plugin-raster-tiles.md) records them. In the pinned
plugin-sdk checkout the pixel layer occurs only in the vendored wire types
(`plugin-sdk: packages/plugin-api/src/wire.d.ts:93`); the host contract there has no door
for it.

**4. "Stage B" names two things in the code.** Core comments label the protocol-50 pixel
layer "C-1 Stage B" (`crates/paged-canvas/src/channel.rs:315`,
`crates/paged-compose/src/pixel_layer.rs:15`, `crates/paged-canvas/src/model.rs:9044`). That
lane carries RGBA8 bytes and is not the Stage B of this ADR; the same module says it is
"forward-compatible with the eventual zero-copy shared-`GPUTexture` Stage B"
(`crates/paged-compose/src/pixel_layer.rs:34-35`). The comment at
`crates/paged-renderer/src/resource_provider.rs:36-37` places the texture variant at
"Stage B (v45)"; protocol 45 carried the gradient fill
(`crates/paged-canvas/src/channel.rs:278-283`), as the Consequences above say, and that
comment is stale.

**5. The Vello version moved.** The investigation above was made against Vello 0.9.0. The
engine now builds against Vello 0.10.0 (`crates/paged-gpu/Cargo.toml:38`). The repository
holds no record of wall 1 being checked against that version. The Vello version in
`Cargo.lock` is 0.10.0 (`Cargo.lock:4328-4329`); its public API includes
`Renderer::register_texture` and `Renderer::override_image`, and core calls neither.
