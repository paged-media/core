/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

import {
  clampScroll,
  clampZoom,
  currentPageAt,
  fitPage,
  scrollToPage,
  zoomAt,
  type Camera,
  type Viewport,
  type ZoomLimits,
} from "./camera.js";
import { Emitter } from "./events.js";
import { attachInput, type InputOptions } from "./input.js";
import type {
  SessionDiagnostics,
  SessionPagesLayout,
  SessionRaster,
  ViewerSessionLike,
} from "./session.js";

/** Structured load/init failure. */
export type ViewerErrorCode =
  | "PARSE_ERROR"
  | "UNSUPPORTED"
  | "GPU_UNAVAILABLE";

export class ViewerError extends Error {
  readonly code: ViewerErrorCode;
  readonly diagnostics: SessionDiagnostics | undefined;

  constructor(
    code: ViewerErrorCode,
    message: string,
    diagnostics?: SessionDiagnostics,
  ) {
    super(message);
    this.name = "ViewerError";
    this.code = code;
    this.diagnostics = diagnostics;
  }
}

export type LayoutMode = "single" | "continuous";

export interface ViewerEvents extends Record<string, unknown> {
  loaded: { pageCount: number };
  pageChanged: { page: number };
  zoomChanged: { zoom: number };
  scrollChanged: { x: number; y: number };
  error: ViewerError;
}

export interface CreateViewerOptions {
  canvas: HTMLCanvasElement;
  /**
   * The wasm session (or a factory). Embedders typically pass
   * `await ViewerSession.new()` from `@paged-media/sdk` after
   * `init(wasmUrl)`; tests pass a fake.
   */
  session: ViewerSessionLike | (() => Promise<ViewerSessionLike>);
  /** Zoom clamps in CSS px per pt. Default `{ min: 0.1, max: 8 }`. */
  zoomLimits?: Partial<ZoomLimits>;
  /** Input lanes; each defaults to enabled. */
  input?: InputOptions;
  /** Initial layout mode. Default `"continuous"`. */
  layoutMode?: LayoutMode;
  /**
   * Frame scheduler — injectable for tests. Defaults to
   * `requestAnimationFrame` (microtask fallback off-DOM).
   */
  schedule?: (frame: () => void) => void;
  /** Device pixel ratio override (defaults to `window.devicePixelRatio`). */
  devicePixelRatio?: number;
}

export interface Viewer {
  load(source: ArrayBuffer | Uint8Array | Blob | string): Promise<void>;

  readonly zoom: number;
  setZoom(zoom: number, opts?: { anchor?: { x: number; y: number } }): void;
  zoomIn(): void;
  zoomOut(): void;
  fit(mode: "page" | "width"): void;
  readonly minZoom: number;
  readonly maxZoom: number;

  readonly scroll: { x: number; y: number };
  scrollTo(x: number, y: number): void;
  scrollBy(dx: number, dy: number): void;

  readonly pageCount: number;
  readonly currentPage: number;
  goToPage(index: number): void;
  layoutMode(mode?: LayoutMode): LayoutMode;
  renderPageThumbnail(
    index: number,
    opts?: { width?: number },
  ): Promise<SessionRaster>;

  on<K extends keyof ViewerEvents>(
    event: K,
    listener: (payload: ViewerEvents[K]) => void,
  ): () => void;

  dispose(): void;
}

const ZOOM_STEP = Math.SQRT2;

function mapDiagnostics(diags: SessionDiagnostics): ViewerError {
  const first = diags.messages.find((m) => m.severity === "error");
  const code: ViewerErrorCode =
    first?.code === "no_gpu" || first?.code === "gpu_init"
      ? "GPU_UNAVAILABLE"
      : first?.code === "open" || first?.code === "build"
        ? "PARSE_ERROR"
        : "UNSUPPORTED";
  return new ViewerError(code, first?.message ?? "load failed", diags);
}

async function sourceBytes(
  source: ArrayBuffer | Uint8Array | Blob | string,
): Promise<Uint8Array> {
  if (typeof source === "string") {
    const res = await fetch(source);
    if (!res.ok) {
      throw new ViewerError(
        "PARSE_ERROR",
        `fetch ${source}: ${res.status} ${res.statusText}`,
      );
    }
    return new Uint8Array(await res.arrayBuffer());
  }
  if (source instanceof Uint8Array) return source;
  if (source instanceof ArrayBuffer) return new Uint8Array(source);
  return new Uint8Array(await source.arrayBuffer());
}

export async function createViewer(
  options: CreateViewerOptions,
): Promise<Viewer> {
  const { canvas } = options;
  let session: ViewerSessionLike;
  try {
    session =
      typeof options.session === "function"
        ? await options.session()
        : options.session;
  } catch (e) {
    throw new ViewerError(
      "GPU_UNAVAILABLE",
      e instanceof Error ? e.message : String(e),
    );
  }

  const limits: ZoomLimits = {
    min: options.zoomLimits?.min ?? 0.1,
    max: options.zoomLimits?.max ?? 8,
  };
  const dpr =
    options.devicePixelRatio ??
    (typeof window !== "undefined" ? window.devicePixelRatio : 1);
  const schedule =
    options.schedule ??
    (typeof requestAnimationFrame === "function"
      ? (frame: () => void) => void requestAnimationFrame(frame)
      : (frame: () => void) => void queueMicrotask(frame));

  const events = new Emitter<ViewerEvents>();

  let layout: SessionPagesLayout = { gapPt: 0, pages: [] };
  let camera: Camera = { zoom: 1, x: 0, y: 0 };
  let mode: LayoutMode = options.layoutMode ?? "continuous";
  let page = 0;
  let bound = false;
  let disposed = false;
  let frameQueued = false;

  function viewport(): Viewport {
    return {
      width: canvas.clientWidth || canvas.width || 1,
      height: canvas.clientHeight || canvas.height || 1,
    };
  }

  /** Layout the camera works against — single mode sees one page at y=0. */
  function activeLayout(): SessionPagesLayout {
    if (mode === "continuous") return layout;
    const p = layout.pages[page];
    return p
      ? { gapPt: layout.gapPt, pages: [{ ...p, yPt: 0, index: 0 }] }
      : { gapPt: layout.gapPt, pages: [] };
  }

  function requestFrame(): void {
    if (frameQueued || disposed || !bound) return;
    frameQueued = true;
    schedule(() => {
      frameQueued = false;
      if (disposed) return;
      const diags = session.present(
        camera.zoom,
        camera.x,
        camera.y,
        dpr,
        mode === "single" ? page : null,
      );
      if (!diags.ok) events.emit("error", mapDiagnostics(diags));
    });
  }

  function applyCamera(next: Camera, force = false): void {
    const clamped = clampScroll(next, activeLayout(), viewport());
    const zoomChanged = clamped.zoom !== camera.zoom;
    const scrollChanged =
      clamped.x !== camera.x || clamped.y !== camera.y;
    if (!zoomChanged && !scrollChanged && !force) return;
    camera = clamped;
    if (zoomChanged) events.emit("zoomChanged", { zoom: camera.zoom });
    if (scrollChanged)
      events.emit("scrollChanged", { x: camera.x, y: camera.y });
    if (mode === "continuous") {
      const now = currentPageAt(camera, layout, viewport());
      if (now !== page) {
        page = now;
        session.set_page(page);
        events.emit("pageChanged", { page });
      }
    }
    requestFrame();
  }

  function setPage(index: number): void {
    const clamped = Math.max(0, Math.min(layout.pages.length - 1, index));
    if (clamped === page && layout.pages.length > 0) return;
    page = clamped;
    session.set_page(page);
    events.emit("pageChanged", { page });
    if (mode === "continuous") {
      applyCamera(scrollToPage(camera, layout, page, viewport()), true);
    } else {
      applyCamera(fitPage(activeLayout(), 0, "page", viewport(), limits), true);
    }
  }

  const detachInput = attachInput(canvas, options.input ?? {}, {
    zoomAt: (factor, anchor) =>
      applyCamera(
        zoomAt(camera, camera.zoom * factor, limits, viewport(), anchor),
      ),
    panBy: (dx, dy) =>
      applyCamera({ zoom: camera.zoom, x: camera.x + dx, y: camera.y + dy }),
    zoomStep: (dir, anchor) =>
      applyCamera(
        zoomAt(
          camera,
          camera.zoom * (dir > 0 ? ZOOM_STEP : 1 / ZOOM_STEP),
          limits,
          viewport(),
          anchor,
        ),
      ),
    fit: () => viewer.fit("page"),
    pageStep: (dir) => setPage(page + dir),
    home: () => setPage(0),
    end: () => setPage(layout.pages.length - 1),
  });

  const viewer: Viewer = {
    async load(source) {
      const bytes = await sourceBytes(source);
      const diags = session.load(bytes);
      if (!diags.ok) {
        const error = mapDiagnostics(diags);
        events.emit("error", error);
        throw error;
      }
      if (!bound) {
        const bind = await session.render_to_canvas_main(canvas);
        if (!bind.ok) {
          const error = mapDiagnostics(bind);
          events.emit("error", error);
          throw error;
        }
        bound = true;
        session.resize(viewport().width, viewport().height, dpr);
      }
      layout = session.page_layout();
      page = 0;
      session.set_page(0);
      events.emit("loaded", { pageCount: layout.pages.length });
      applyCamera(fitPage(activeLayout(), 0, "page", viewport(), limits), true);
    },

    get zoom() {
      return camera.zoom;
    },
    setZoom(zoom, opts) {
      applyCamera(
        zoomAt(
          camera,
          clampZoom(zoom, limits),
          limits,
          viewport(),
          opts?.anchor,
        ),
      );
    },
    zoomIn() {
      this.setZoom(camera.zoom * ZOOM_STEP);
    },
    zoomOut() {
      this.setZoom(camera.zoom / ZOOM_STEP);
    },
    fit(fitMode) {
      const idx = mode === "single" ? 0 : page;
      applyCamera(fitPage(activeLayout(), idx, fitMode, viewport(), limits), true);
    },
    get minZoom() {
      return limits.min;
    },
    get maxZoom() {
      return limits.max;
    },

    get scroll() {
      return { x: camera.x, y: camera.y };
    },
    scrollTo(x, y) {
      applyCamera({ zoom: camera.zoom, x, y });
    },
    scrollBy(dx, dy) {
      applyCamera({ zoom: camera.zoom, x: camera.x + dx, y: camera.y + dy });
    },

    get pageCount() {
      return layout.pages.length;
    },
    get currentPage() {
      return page;
    },
    goToPage(index) {
      setPage(index);
    },
    layoutMode(next) {
      if (next && next !== mode) {
        mode = next;
        applyCamera(
          fitPage(activeLayout(), mode === "single" ? 0 : page, "page", viewport(), limits),
          true,
        );
      }
      return mode;
    },
    renderPageThumbnail(index, opts) {
      return session.render_page_to_bytes(index, opts?.width ?? 160);
    },

    on(event, listener) {
      return events.on(event, listener);
    },

    dispose() {
      if (disposed) return;
      disposed = true;
      detachInput();
      events.clear();
      session.free?.();
    },
  };

  return viewer;
}
