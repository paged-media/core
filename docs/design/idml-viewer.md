# Paged Viewer — A Pure IDML Viewer as the SDK's Ground Floor

June 2026. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

Sections not relevant outside the original planning context have been removed; numbering is unchanged.

*Status note (2026-10-02): the viewer was built and ships from this repository, with a package shape that differs from this paper. `@paged-media/sdk` (`crates/paged-sdk`) is the read-only wasm `ViewerSession`, and `@paged-media/idml-viewer` (`web/idml-viewer`) is a TypeScript wrapper over it: camera, pages, input and events, with no UI chrome and no `"spreads"` mode. [ADR 112](../adr/112-viewer-sdk-is-a-sibling.md) records the read-only boundary and [ADR 123](../adr/123-viewer-ships-from-core.md) where the viewer ships from. Section notes below say where the text and the code differ; they were checked at commit `9f933f1`.*

**Scope:** A read-only IDML viewer as a first-class SDK deliverable: a stripped WASM build of the core (no mutations, no Boa, no plugin runtime) plus the minimal SDK surface a viewer needs — load, render, zoom, scroll, navigate.

---

## 1. One-liner

> **What PDF.js is to PDF, Paged Viewer is to IDML.** A canvas, a document, zoom and scroll — nothing else. Compiled from the *same* Rust pipeline as the editor with the mutable half switched off, so it renders pixel-identically to the editor at a fraction of the size, and embeds anywhere with three lines of JavaScript.

---

## 2. Motivation

### 2.2 The architecture case

The viewer is not a second implementation — it is **the editor minus its mutable half**. Because rendering, layout, and parsing live in one Rust pipeline, a read-only build is a *compilation subset*, not a fork. This yields the property that makes the product trustworthy: **the viewer cannot render differently from the editor**, because it is the same code. Any "viewer-vs-editor difference" is by construction a build configuration bug, not a fidelity bug.

It also imposes a healthy discipline on core: the feature-gate boundary between "read/layout/render" and "mutate/script/extend" must be clean for the viewer build to exist at all. The viewer is therefore also a continuous architectural test that the core's read path has no accidental dependencies on the write path.

---

## 3. Goals & Non-Goals

### Goals

1. **Pure viewer.** Load an IDML document, render it, zoom, scroll, navigate pages. Full stop.
2. **Strict subset build.** One core codebase, two WASM artifacts: `paged-editor.wasm` (full) and `paged-viewer.wasm` (read-only subset). The viewer artifact contains **no mutation engine (Operations/Gestures), no Boa, no plugin runtime, no edit contexts, no panel schema** — compiled out, not disabled at runtime.
3. **Pixel-identical rendering.** Same Vello scene production as the editor; verified by scene-digest equivalence in CI.
4. **Minimal, stable JS API.** The viewer surface is the *base layer* of `@paged-media/sdk` — the editor session API extends it, never duplicates it.
5. **Embeddable by strangers.** Three-line integration, no build-tool assumptions, documented bundle sizes, predictable lifecycle.

### Non-Goals

- No editing of any kind — not even "just move this one frame." The moment a mutation API exists, the subset guarantee and the size budget die together.
- No Boa, no scripting, no transforms. Documents containing paged.web frames or paged.draw live constructs render their **baked IDML fallback** (the baked-fallback doctrine of the plugin system pays off here: every plugin-extended document is viewer-renderable by definition, with zero plugin code in the viewer).
- No panels, no shell (no Cockpit; the editor's "no Dockview" — the viewer links neither shell substrate). The viewer owns exactly one DOM element: its canvas (plus optional minimal chrome, §6.4).
- No annotation/commenting layer in v1 (recorded as a tiered candidate, §8 — it is the most-requested viewer feature class and must not creep into v1).

---

## 4. Build Architecture: One Crate Tree, Two Artifacts

*Status note (2026-10-02): this section predates the implementation; see [ADR 112](../adr/112-viewer-sdk-is-a-sibling.md). There are no `read` and `write` cargo features: the split is by crate, and `crates/paged-sdk` depends on `paged-renderer`, `paged-compose`, `paged-gpu` and the IDML reader (`crates/paged-sdk/Cargo.toml`). The check that no editor crate enters its dependency tree runs in the publish workflow (`.github/workflows/publish-wasm.yml:261`), not on every merge. No crate uses salsa. The publish workflow prints the SDK wasm's sizes next to a recorded baseline and cites §4.2 for it (`.github/workflows/publish-wasm.yml:283`); it does not enforce the 40% ratio. Rendering is WebGPU only and no CPU fallback was built.*

### 4.1 Feature gates

The core workspace gains a strict two-tier feature topology:

```
paged-core
├── [feature: read]   ── default for viewer ──────────────┐
│     idml parse · document model (immutable view)        │
│     fonts · color · salsa layout pipeline (read-only)   │  paged-viewer.wasm
│     vello scene production · camera/viewport            │
│     page geometry & navigation                          │
├── [feature: write]  ── editor adds ──────────────────────┘
│     operations/gestures · mutation engine · undo
│     Boa scripting · plugin runtime · edit contexts
│     panel schema host · tool registry
```

Rules:

- `write` depends on `read`; `read` must compile with `write` absent. CI builds both permutations on every merge — the viewer build is a **gate**, not a release-time scramble.
- No `#[cfg]` scattered through shared files as the primary mechanism; the split follows crate/module boundaries (`paged-mutate`, `paged-scripting`, `paged-plugins` simply don't enter the viewer dependency graph). `#[cfg(feature = "write")]` is the exception for genuinely shared types, not the rule.
- The salsa pipeline runs in both builds; in the viewer it is populated once at load (plus on window/zoom-driven re-layout if applicable) and never receives mutations. Incremental machinery costs little when nothing invalidates.

### 4.2 Size budget

Binary size is the viewer's headline engineering metric — it is the difference between "embeddable" and "technically embeddable."

- **Budget:** viewer WASM ≤ **40%** of the editor WASM, with an absolute target set after the first measured build (placeholder until measured; record the actuals once measured).
- Measured in CI per merge, with a ratchet: the budget may only tighten.
- Expected major savings: Boa (a full ES engine), the mutation/undo machinery, plugin runtime, tsify surface for editing types. Expected stubborn weight: fonts/text shaping and Vello itself — shared with the editor and non-negotiable for fidelity.

### 4.3 Rendering backend & browser scope

The editor is deliberately Chrome/WebGPU-only. The viewer's embedding use cases (DAM previews, docs, approval links) argue for broader reach — a stakeholder clicking an approval link may be on Safari.

- **v1:** WebGPU (same as editor), documented as a requirement; graceful capability detection with a clear "browser not supported" state and a host-provided fallback slot (e.g. host shows a static preview image).
- **Tracked, not promised:** a CPU/raster fallback path via Vello's evolving CPU rendering work, evaluated when upstream matures. The decision deliberately mirrors the editor's Chrome-first discipline rather than fragmenting render correctness across backends prematurely. (Open question Q2.)

---

## 5. SDK Layering

*Status note (2026-10-02): this section predates the implementation, and the layering was built the other way round; see [ADR 112](../adr/112-viewer-sdk-is-a-sibling.md) and [ADR 123](../adr/123-viewer-ships-from-core.md). `@paged-media/sdk` is the wasm `ViewerSession`, and `@paged-media/idml-viewer` wraps a session it is handed (`web/idml-viewer/src/viewer.ts:67`). There is no `EditorSession` and no `/ui` export (`web/idml-viewer/package.json:14`). The editor's write SDK is a separate package in the editor repository ([ADR 205](https://github.com/paged-media/editor/blob/main/docs/adr/205-client-packaged-as-write-sdk.md)).*

The viewer is a **standalone package** — `@paged-media/idml-viewer` — and the **ground floor** of the SDK family. The editor SDK depends on it and extends it, never duplicates it:

```
@paged-media/idml-viewer                 ← this concept · publishes from core · ships paged-viewer.wasm
│   ViewerSession
│     load · render · pages · camera (zoom/scroll) · events · thumbnails · dispose
│   /ui  — optional minimal chrome (separate import)
│
@paged-media/sdk   (depends on @paged-media/idml-viewer, re-exports its types)
└── EditorSession extends ViewerSession   (requires paged-editor.wasm)
      selection · operations · gestures · tools · plugin host · …
```

Why standalone rather than an SDK subpath: the audiences differ (DAM/CMS embedders vs. editor/plugin integrators), the version lines should decouple (the viewer freezes first and stays at a boring 1.x while the SDK iterates), npm discoverability ("idml viewer" *is* the package name — the `pdfjs-dist` precedent), and install optics are honest (a small package + `paged-viewer.wasm`, not "install the whole SDK to embed a canvas").

Consequences:

- **One camera, one coordinate system, one page model** across viewer and editor. An app that starts with the viewer upgrades to the editor by swapping the WASM artifact and constructing `EditorSession` — the viewer code keeps working unchanged.
- The viewer API is the most stability-critical surface in the SDK (it is the one strangers embed), so it freezes **first** and stays deliberately small.

---

## 6. API Surface (v1)

*Status note (2026-10-02): this section predates the implementation; the built interface is `Viewer` in `web/idml-viewer/src/viewer.ts:90`. It differs in these points: `createViewer` takes `{ canvas, session }`, and the bundled wasm is loaded with `createSessionFromBundledWasm(wasmUrl?)`; `fit` accepts `"page" | "width"`; `layoutMode` is `"single" | "continuous"`; `renderPageThumbnail` resolves to an RGBA8 raster, not an `ImageBitmap`; there is no `/ui` chrome.*

### 6.1 Construction & lifecycle

```ts
import { createViewer } from "@paged-media/idml-viewer";

const viewer = await createViewer({
  canvas: HTMLCanvasElement,          // the one DOM element the viewer owns
  wasmUrl?: string,                   // default: bundled paged-viewer.wasm
});

await viewer.load(source);            // ArrayBuffer | Blob | URL (IDML)
viewer.dispose();                     // releases GPU + WASM resources, removes listeners
```

`load` is repeatable (document switching without re-init). Errors are structured: `{ code: "PARSE_ERROR" | "UNSUPPORTED" | "GPU_UNAVAILABLE" | …, detail }`.

### 6.2 Camera: zoom & scroll

The complete v1 camera surface — small by intent:

```ts
viewer.zoom                            // number (1 = 100%)
viewer.setZoom(z, opts?: { anchor?: Point })   // anchor in canvas px; default center
viewer.zoomIn(); viewer.zoomOut();             // stepped, configurable steps
viewer.fit("page" | "width" | "spread");
viewer.minZoom / maxZoom                       // configurable clamps

viewer.scroll                          // { x, y } in document space
viewer.scrollTo(point, opts?: { animate?: boolean })
viewer.scrollBy(delta)
```

Built-in input handling (each individually disableable for hosts that bring their own UI):
wheel + `Ctrl`/pinch zoom-to-cursor, drag-to-pan (and/or scrollbar mode), double-click zoom step, keyboard (`+`/`-`/`0`, arrows, PgUp/PgDn, Home/End), touch pinch/pan. Zoom and pan are the viewer's only "gestures," implemented on the camera — **no document hit-testing required**, keeping the gesture system out of the subset.

### 6.3 Pages & navigation

```ts
viewer.pageCount
viewer.currentPage                     // 1-based; derived from viewport in continuous mode
viewer.goToPage(n)
viewer.layoutMode: "single" | "continuous" | "spreads"
viewer.renderPageThumbnail(n, { width }): Promise<ImageBitmap>   // for host-built page rails
```

### 6.4 Events & chrome

```ts
viewer.on("loaded" | "pageChanged" | "zoomChanged" | "scrollChanged" | "error", handler)
```

Default chrome is **none** — the host builds UI from the API and events. An optional, separately-importable minimal chrome (`@paged-media/idml-viewer/ui`: zoom buttons, page indicator, styleguide-conformant) keeps the core surface pure while serving the three-line-integration story.

### 6.5 Explicitly absent from v1

No selection, no text extraction, no search, no hit-testing, no annotations, no printing helper, no outline/bookmarks. Each is a tiered candidate (§8); none is allowed to blur the v1 line.

---

## 7. Fidelity & Testing

*Status note (2026-10-02): this section predates the implementation. Item 1 exists in its native form: `crates/paged-sdk/tests/digest_equivalence.rs` compares the viewer's build path with the stock build, page digest by page digest; its header names the comparison across the two wasm artifacts as a follow-up. Item 2 is the dependency check named under §4. The camera invariants of item 4 are unit tests (`web/idml-viewer/test/camera.test.ts`), not a Playwright lane. No viewer test in this repository covers item 5.*

1. **Scene-digest equivalence (the keystone test):** for every corpus document, the viewer build and the editor build must produce identical scene digests at identical camera states. Runs in CI on both artifacts per merge. This is the executable form of the "same pixels" promise.
2. **Subset gate:** viewer WASM build (`read` only) compiles green per merge; symbol-level check asserts no mutation/Boa/plugin symbols leak into the artifact.
3. **Size ratchet:** §4.2 budget enforced in CI.
4. **Camera tests:** Playwright lane reusing the existing harness pattern — zoom-to-cursor invariants (point under cursor stays fixed), fit-mode correctness, clamping, continuous-scroll page derivation.
5. **Degradation fixtures:** documents carrying `x-paged-draw:*` / `x-paged-web:*` metadata render their baked fallback identically in viewer and editor-with-plugins-disabled.

---

## 8. Tiered Candidates Beyond v1

| Capability | Tier | Notes |
|---|---|---|
| Text selection & copy | **B** | First real post-v1 feature; requires read-only hit-testing + glyph-to-text mapping in the subset. Gateway to search. |
| Find/search in document | **B** | Builds on the same text mapping. |
| Outline / page-thumbnail rail as built-in chrome | **B** | Pure UI over existing API. |
| Deep links (`#page=3&zoom=fit-width`) | **B** | Trivial, high embed value. |
| Annotation/commenting layer | **B/C** | Most-requested viewer feature class; likely a separate package over viewer events + an overlay canvas — not in the subset WASM. |
| Print-to-PDF helper | **C** | Belongs to the PDF-export backend concept, exposed through the editor build. |
| Progressive/partial loading of large IDML | **B** | Matters for DAM-scale documents; needs measurement first. |
| CPU render fallback (non-WebGPU browsers) | **Open** | Q2; tracks upstream Vello CPU work. |

---

## 9. Delivery Plan

*Status note (2026-10-02): this section predates the implementation. The session, the wrapper API, thumbnails and the native digest test are built and published, and docs.paged.media embeds the published package ([ADR 803](https://github.com/paged-media/docs/blob/main/docs/adr/803-live-preview-uses-viewer-package.md)). The chrome of V2 and everything in V3 are not built. Source comments cite these phase names (`crates/paged-sdk/tests/digest_equivalence.rs:22`).*

| Phase | Milestone | Exit criterion |
|---|---|---|
| **V0** | Feature-gate split lands in core; `read`-only build compiles; symbol check in CI | `paged-viewer.wasm` exists and is mutation/Boa/plugin-free by symbol audit |
| **V1** | ViewerSession API (load, camera, pages, events) + scene-digest equivalence lane + size ratchet | Three-line embed renders the corpus pixel-identically to the editor; budgets green |
| **V2** | `viewer-ui` minimal chrome; thumbnails; docs.paged.media live-sample integration; published quickstart | An outside developer embeds the viewer from docs alone in <15 minutes |
| **V3** | First Tier-B wave (text selection → search → deep links), demand-ordered | Selection/copy works without breaking the size ratchet's revised budget |

Sequencing note: **V0 belongs early** — it is cheap while the core is young and expensive after more write-path code accretes. V1+ can follow at lower priority without losing the architectural benefit.

---

## 10. Open Questions

1. **Re-layout policy in the viewer** — IDML layout is page-fixed, so v1 can layout once at load. Confirm no zoom-dependent layout exists (e.g. hairline policies) that would require pipeline re-runs on camera change.
2. **Non-WebGPU fallback** — track Vello CPU rendering maturity; decide at V2 whether "browser not supported + host fallback slot" remains acceptable for the approval-link use case.
3. **~~Package shape~~ — RESOLVED: standalone `@paged-media/idml-viewer`** (optional chrome at `@paged-media/idml-viewer/ui`), published from the core repo, shipping `paged-viewer.wasm`. `ViewerSession` lives here; `@paged-media/sdk` depends on this package and re-exports its types (`EditorSession extends ViewerSession` across the package boundary — one implementation, one camera, one page model). Rationale: distinct audience, decoupled version line (viewer freezes at 1.x while the SDK iterates), npm discoverability, honest install optics. The `idml-` prefix keeps the namespace open for future format viewers. Locked before the first canary publish — add it to the publish chain. *(Status note, 2026-10-02: the package is standalone and published from this repository, but `@paged-media/sdk` does not depend on it: the viewer package wraps the SDK's session and bundles its wasm; see the note under §5.)*
4. **Font licensing surface** — embedded document fonts render in the viewer exactly as in the editor; confirm no additional obligations attach to *distribution* of a freely-embeddable viewer.
5. **Size budget absolute number** — set after the first measured V0 build; 40%-of-editor is the placeholder ratio.
6. **Streaming load** — `load(URL)` v1 semantics: full fetch then parse, or range-request streaming for large documents? v1 ships full-fetch; measure before adding complexity.

---

## 11. Summary

The viewer is the smallest possible expression of Paged: the read half of the pipeline, compiled alone, wrapped in the smallest API that makes a document visible and navigable. It is simultaneously a product (the PDF.js of IDML), an architectural enforcement mechanism (the read/write boundary must stay clean for the build to exist), and a fidelity guarantee (same code, same scene, same pixels — proven by digest equivalence, not promised). It contains no mutations, no Boa, no plugins — and because every plugin-extended document bakes to honest IDML, it renders the whole ecosystem's output anyway.

**One pipeline, two artifacts: the editor that makes documents, and the viewer that shows them everywhere.**
