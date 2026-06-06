# @paged-media/idml-viewer

Standalone IDML viewer for the browser — camera, pages, input lanes and
events over the `@paged-media/sdk` WebGPU `ViewerSession`. Rendering is
WebGPU-only (the session rejects without `navigator.gpu`).

```ts
import { createViewer, createSessionFromBundledWasm } from "@paged-media/idml-viewer";

const viewer = await createViewer({
  canvas: document.querySelector("canvas")!,
  session: createSessionFromBundledWasm, // wasm ships inside this package
});

viewer.on("pageChanged", ({ page }) => console.log("page", page + 1));
await viewer.load("/files/brochure.idml"); // URL | ArrayBuffer | Uint8Array | Blob
```

## Surface

- **Load** — `load(source)`, repeatable; failures throw a `ViewerError`
  with `code: "PARSE_ERROR" | "UNSUPPORTED" | "GPU_UNAVAILABLE"` and the
  engine's structured diagnostics attached.
- **Camera** — `zoom`, `setZoom(z, { anchor })`, `zoomIn/zoomOut`,
  `fit("page" | "width")`, `minZoom/maxZoom`, `scroll`, `scrollTo`,
  `scrollBy`. Zooming about an anchor keeps the document point under it
  fixed.
- **Pages** — `pageCount`, `currentPage`, `goToPage(n)`,
  `layoutMode("single" | "continuous")`,
  `renderPageThumbnail(n, { width })` (RGBA8 readback).
- **Events** — `on("loaded" | "pageChanged" | "zoomChanged" |
  "scrollChanged" | "error", cb)` → unsubscribe function.
- **Input** — wheel/pinch zoom-to-cursor, drag-pan, double-click zoom,
  keyboard (`+`/`-`/`0`, arrows, PgUp/PgDn, Home/End); each lane
  disableable via `input: {...}`.
- **Teardown** — `dispose()` detaches all listeners and frees the wasm
  session.

Fonts: register faces on the session before `load`
(`session.register_font(family, style, bytes)`) — same contract as
`paged-inspect --font-family`.

## Embedding your own session

`createViewer` takes any object satisfying `ViewerSessionLike` (the
typed contract of the wasm session), so you can init the wasm yourself
— e.g. from a custom URL or a worker — and hand it in. Tests do exactly
this with a fake.

## License

MPL-2.0 OR LicenseRef-PMEL — see the repository `LICENSE.md`.
