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

import { describe, expect, it } from "vitest";

import {
  clampScroll,
  contentExtent,
  currentPageAt,
  fitPage,
  scrollToPage,
  zoomAt,
  FIT_MARGIN,
  type Camera,
} from "../src/camera.js";
import { letterLayout } from "./fake-session.js";

const VIEWPORT = { width: 800, height: 600 };
const LIMITS = { min: 0.1, max: 8 };

describe("zoomAt", () => {
  it("keeps the doc point under the anchor fixed (invariance)", () => {
    const camera: Camera = { zoom: 1, x: 40, y: -120 };
    const anchor = { x: 333, y: 222 };
    const before = {
      x: (anchor.x - camera.x) / camera.zoom,
      y: (anchor.y - camera.y) / camera.zoom,
    };
    const next = zoomAt(camera, 2.5, LIMITS, VIEWPORT, anchor);
    expect(anchor.x - (camera.x + 0)).not.toBe(0); // anchor off-origin
    expect(before.x * next.zoom + next.x).toBeCloseTo(anchor.x, 6);
    expect(before.y * next.zoom + next.y).toBeCloseTo(anchor.y, 6);
  });

  it("defaults the anchor to the viewport centre", () => {
    const camera: Camera = { zoom: 1, x: 0, y: 0 };
    const next = zoomAt(camera, 2, LIMITS, VIEWPORT);
    const centre = { x: VIEWPORT.width / 2, y: VIEWPORT.height / 2 };
    const doc = { x: centre.x / 1, y: centre.y / 1 };
    expect(doc.x * next.zoom + next.x).toBeCloseTo(centre.x, 6);
    expect(doc.y * next.zoom + next.y).toBeCloseTo(centre.y, 6);
  });

  it("clamps to the zoom limits", () => {
    const camera: Camera = { zoom: 1, x: 0, y: 0 };
    expect(zoomAt(camera, 100, LIMITS, VIEWPORT).zoom).toBe(LIMITS.max);
    expect(zoomAt(camera, 0.0001, LIMITS, VIEWPORT).zoom).toBe(LIMITS.min);
  });
});

describe("fitPage", () => {
  it('"page" fits both dimensions with the margin', () => {
    const layout = letterLayout(1);
    const cam = fitPage(layout, 0, "page", VIEWPORT, LIMITS);
    const expected =
      Math.min(VIEWPORT.width / 612, VIEWPORT.height / 792) * FIT_MARGIN;
    expect(cam.zoom).toBeCloseTo(expected, 6);
    // Page is horizontally and vertically centred.
    expect(cam.x).toBeCloseTo((VIEWPORT.width - 612 * cam.zoom) / 2, 6);
    expect(cam.y).toBeCloseTo((VIEWPORT.height - 792 * cam.zoom) / 2, 6);
  });

  it('"width" fits the page width', () => {
    const layout = letterLayout(1);
    const cam = fitPage(layout, 0, "width", VIEWPORT, LIMITS);
    expect(cam.zoom).toBeCloseTo((VIEWPORT.width / 612) * FIT_MARGIN, 6);
    expect(cam.x).toBeCloseTo((VIEWPORT.width - 612 * cam.zoom) / 2, 6);
  });

  it("targets the page's continuous offset", () => {
    const layout = letterLayout(3);
    const cam = fitPage(layout, 2, "page", VIEWPORT, LIMITS);
    const page = layout.pages[2]!;
    // The page's centre lands on the viewport centre.
    const pageCentreDocY = page.yPt + page.heightPt / 2;
    expect(pageCentreDocY * cam.zoom + cam.y).toBeCloseTo(
      VIEWPORT.height / 2,
      4,
    );
  });
});

describe("clampScroll", () => {
  const layout = letterLayout(3);

  it("centres content smaller than the viewport", () => {
    const cam = clampScroll({ zoom: 0.5, x: -999, y: 50 }, layout, VIEWPORT);
    // 612 * 0.5 = 306 < 800 → centred horizontally.
    expect(cam.x).toBeCloseTo((800 - 306) / 2, 6);
  });

  it("clamps content larger than the viewport to the edges", () => {
    const { height } = contentExtent(layout);
    const big = { zoom: 2, x: 500, y: 500 };
    const cam = clampScroll(big, layout, VIEWPORT);
    expect(cam.x).toBe(0); // can't open a gap on the left
    expect(cam.y).toBe(0); // …or at the top
    const low = clampScroll({ zoom: 2, x: -99999, y: -99999 }, layout, VIEWPORT);
    expect(low.x).toBe(VIEWPORT.width - 612 * 2);
    expect(low.y).toBe(VIEWPORT.height - height * 2);
  });
});

describe("currentPageAt", () => {
  const layout = letterLayout(3);

  it("derives the page under the viewport centre", () => {
    const zoom = 0.5;
    // Page 1 spans yPt 816..1608; put its middle on the centre line.
    const target = 816 + 396;
    const camY = VIEWPORT.height / 2 - target * zoom;
    expect(
      currentPageAt({ zoom, x: 0, y: camY }, layout, VIEWPORT),
    ).toBe(1);
  });

  it("falls back to the nearest band inside a gap", () => {
    const zoom = 1;
    const gapMiddle = 792 + 12; // inside the 24pt gap after page 0
    const camY = VIEWPORT.height / 2 - gapMiddle * zoom;
    const page = currentPageAt({ zoom, x: 0, y: camY }, layout, VIEWPORT);
    expect([0, 1]).toContain(page);
  });
});

describe("scrollToPage", () => {
  it("brings the page top into view and centres horizontally", () => {
    const layout = letterLayout(3);
    const cam = scrollToPage({ zoom: 1, x: 0, y: 0 }, layout, 2, VIEWPORT);
    const page = layout.pages[2]!;
    // Page top sits just below the viewport top (≤ gap of breathing room).
    const topCss = page.yPt * cam.zoom + cam.y;
    expect(topCss).toBeGreaterThanOrEqual(0);
    expect(topCss).toBeLessThanOrEqual(24);
    expect(cam.x).toBeCloseTo((VIEWPORT.width - 612 * cam.zoom) / 2, 6);
  });
});
