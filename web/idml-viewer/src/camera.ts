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

/**
 * Pure camera math over the continuous page stack. Doc space is pt
 * (pages stacked vertically per `page_layout()`); the camera maps a
 * doc point `p` to viewport CSS px as `scroll + p * zoom`. Everything
 * here is side-effect-free so the invariants (zoom-to-anchor, fit,
 * clamps, current-page) are unit-testable without a canvas.
 */

import type { SessionPagesLayout } from "./session.js";

export interface Camera {
  /** CSS px per pt. */
  zoom: number;
  /** Doc origin in viewport CSS px. */
  x: number;
  y: number;
}

export interface Viewport {
  /** CSS px. */
  width: number;
  height: number;
}

export interface ZoomLimits {
  min: number;
  max: number;
}

/** Default page-fit margin — matches the wasm fit-page present. */
export const FIT_MARGIN = 0.95;

export function clampZoom(zoom: number, limits: ZoomLimits): number {
  return Math.min(limits.max, Math.max(limits.min, zoom));
}

/** Total doc-space extents of the continuous stack (pt). */
export function contentExtent(layout: SessionPagesLayout): {
  width: number;
  height: number;
} {
  let width = 0;
  let height = 0;
  for (const p of layout.pages) {
    width = Math.max(width, p.widthPt);
    height = Math.max(height, p.yPt + p.heightPt);
  }
  return { width, height };
}

/**
 * Zoom about an anchor point (viewport CSS px): the doc point under
 * the anchor stays under it. With no anchor, zoom about the viewport
 * centre.
 */
export function zoomAt(
  camera: Camera,
  nextZoom: number,
  limits: ZoomLimits,
  viewport: Viewport,
  anchor?: { x: number; y: number },
): Camera {
  const z = clampZoom(nextZoom, limits);
  const ax = anchor?.x ?? viewport.width / 2;
  const ay = anchor?.y ?? viewport.height / 2;
  const docX = (ax - camera.x) / camera.zoom;
  const docY = (ay - camera.y) / camera.zoom;
  return { zoom: z, x: ax - docX * z, y: ay - docY * z };
}

/**
 * Keep content on screen: per axis, content smaller than the viewport
 * centres; larger content clamps so no blank gap opens at either edge.
 */
export function clampScroll(
  camera: Camera,
  layout: SessionPagesLayout,
  viewport: Viewport,
): Camera {
  const { width, height } = contentExtent(layout);
  const w = width * camera.zoom;
  const h = height * camera.zoom;
  const clampAxis = (pos: number, content: number, view: number): number =>
    content <= view
      ? (view - content) / 2
      : Math.min(0, Math.max(view - content, pos));
  return {
    zoom: camera.zoom,
    x: clampAxis(camera.x, w, viewport.width),
    y: clampAxis(camera.y, h, viewport.height),
  };
}

/**
 * Fit a page: `"page"` fits both dimensions, `"width"` fits the page
 * width (top-aligned to the page). Returns the camera centred on the
 * page in continuous coordinates; single-page mode passes a layout
 * containing just that page at `yPt: 0`.
 */
export function fitPage(
  layout: SessionPagesLayout,
  pageIndex: number,
  mode: "page" | "width",
  viewport: Viewport,
  limits: ZoomLimits,
): Camera {
  const page = layout.pages[pageIndex] ?? layout.pages[0];
  if (!page) return { zoom: 1, x: 0, y: 0 };
  const pw = Math.max(page.widthPt, 1);
  const ph = Math.max(page.heightPt, 1);
  const zoom = clampZoom(
    mode === "page"
      ? Math.min(viewport.width / pw, viewport.height / ph) * FIT_MARGIN
      : (viewport.width / pw) * FIT_MARGIN,
    limits,
  );
  const x = (viewport.width - pw * zoom) / 2;
  const y =
    mode === "page"
      ? (viewport.height - ph * zoom) / 2 - page.yPt * zoom
      : (viewport.height - ph * zoom) / 2 -
        page.yPt * zoom +
        Math.max(0, (ph * zoom - viewport.height) / 2);
  return { zoom, x, y };
}

/**
 * The page whose band contains the viewport-centre line (continuous
 * mode). Falls back to the nearest band when the centre sits in a gap.
 */
export function currentPageAt(
  camera: Camera,
  layout: SessionPagesLayout,
  viewport: Viewport,
): number {
  if (layout.pages.length === 0) return 0;
  const centreDocY = (viewport.height / 2 - camera.y) / camera.zoom;
  let best = 0;
  let bestDist = Number.POSITIVE_INFINITY;
  for (const p of layout.pages) {
    if (centreDocY >= p.yPt && centreDocY <= p.yPt + p.heightPt) return p.index;
    const mid = p.yPt + p.heightPt / 2;
    const dist = Math.abs(mid - centreDocY);
    if (dist < bestDist) {
      bestDist = dist;
      best = p.index;
    }
  }
  return best;
}

/** Scroll that brings `pageIndex`'s top into view (continuous mode). */
export function scrollToPage(
  camera: Camera,
  layout: SessionPagesLayout,
  pageIndex: number,
  viewport: Viewport,
): Camera {
  const page = layout.pages[pageIndex];
  if (!page) return camera;
  const x = (viewport.width - page.widthPt * camera.zoom) / 2;
  // Small breathing room above the page top, mirroring the gap.
  const y = -page.yPt * camera.zoom + Math.min(12, layout.gapPt * camera.zoom);
  return { zoom: camera.zoom, x, y };
}
