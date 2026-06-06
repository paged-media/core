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

import { createViewer, ViewerError } from "../src/viewer.js";
import { fakeCanvas, FakeSession, syncSchedule } from "./fake-session.js";

async function makeViewer(session = new FakeSession()) {
  const canvas = fakeCanvas();
  const viewer = await createViewer({
    canvas,
    session,
    schedule: syncSchedule,
    devicePixelRatio: 2,
  });
  return { viewer, session, canvas };
}

const IDML = new Uint8Array([0x50, 0x4b]); // any bytes — the fake ignores them

describe("load", () => {
  it("binds, lays out, fits page 0 and emits loaded", async () => {
    const { viewer, session } = await makeViewer();
    const events: number[] = [];
    viewer.on("loaded", (e) => events.push(e.pageCount));

    await viewer.load(IDML);

    expect(session.loads).toBe(1);
    expect(events).toEqual([3]);
    expect(viewer.pageCount).toBe(3);
    expect(viewer.currentPage).toBe(0);
    // fit("page") at 800×600 for 612×792 → zoom ≈ 600/792*0.95.
    expect(viewer.zoom).toBeCloseTo((600 / 792) * 0.95, 5);
    // A present was scheduled with the camera + dpr.
    expect(session.presents.length).toBeGreaterThan(0);
    expect(session.presents.at(-1)?.dpr).toBe(2);
  });

  it("is repeatable — a second load rebinds nothing and re-fits", async () => {
    const { viewer, session } = await makeViewer();
    await viewer.load(IDML);
    await viewer.load(IDML);
    expect(session.loads).toBe(2);
    expect(viewer.currentPage).toBe(0);
  });

  it("maps parse failures to ViewerError PARSE_ERROR and emits error", async () => {
    const session = new FakeSession();
    session.loadResult = {
      ok: false,
      messages: [
        { severity: "error", code: "open", message: "open IDML: bad zip" },
      ],
    };
    const { viewer } = await makeViewer(session);
    const seen: ViewerError[] = [];
    viewer.on("error", (e) => seen.push(e));

    await expect(viewer.load(IDML)).rejects.toMatchObject({
      code: "PARSE_ERROR",
    });
    expect(seen).toHaveLength(1);
    expect(seen[0]?.diagnostics?.messages[0]?.code).toBe("open");
  });

  it("maps GPU bind failures to GPU_UNAVAILABLE", async () => {
    const session = new FakeSession();
    session.bindResult = {
      ok: false,
      messages: [
        { severity: "error", code: "gpu_init", message: "WebGPU init failed" },
      ],
    };
    const { viewer } = await makeViewer(session);
    await expect(viewer.load(IDML)).rejects.toMatchObject({
      code: "GPU_UNAVAILABLE",
    });
  });

  it("a rejecting session factory becomes GPU_UNAVAILABLE", async () => {
    await expect(
      createViewer({
        canvas: fakeCanvas(),
        session: () => Promise.reject(new Error("navigator.gpu is undefined")),
      }),
    ).rejects.toMatchObject({ code: "GPU_UNAVAILABLE" });
  });
});

describe("camera surface", () => {
  it("zoomChanged/scrollChanged fire once per change", async () => {
    const { viewer } = await makeViewer();
    await viewer.load(IDML);
    const zooms: number[] = [];
    const scrolls: Array<{ x: number; y: number }> = [];
    viewer.on("zoomChanged", (e) => zooms.push(e.zoom));
    viewer.on("scrollChanged", (e) => scrolls.push(e));

    viewer.setZoom(viewer.zoom); // no-op → no events
    expect(zooms).toHaveLength(0);

    viewer.zoomIn();
    expect(zooms).toHaveLength(1);
    expect(zooms[0]).toBeCloseTo((600 / 792) * 0.95 * Math.SQRT2, 5);

    viewer.scrollBy(0, -40);
    expect(scrolls.length).toBeGreaterThan(0);
  });

  it("presents with the rAF batch — one present per applied change", async () => {
    const { viewer, session } = await makeViewer();
    await viewer.load(IDML);
    const before = session.presents.length;
    viewer.zoomIn();
    expect(session.presents.length).toBe(before + 1);
  });

  it("clamps zoom to min/max", async () => {
    const { viewer } = await makeViewer();
    await viewer.load(IDML);
    viewer.setZoom(99);
    expect(viewer.zoom).toBe(viewer.maxZoom);
    viewer.setZoom(0.000001);
    expect(viewer.zoom).toBe(viewer.minZoom);
  });
});

describe("pages", () => {
  it("goToPage scrolls the page into view and emits pageChanged", async () => {
    const { viewer, session } = await makeViewer();
    await viewer.load(IDML);
    const pages: number[] = [];
    viewer.on("pageChanged", (e) => pages.push(e.page));

    viewer.goToPage(2);
    expect(viewer.currentPage).toBe(2);
    expect(pages).toEqual([2]);
    expect(session.pageSets.at(-1)).toBe(2);
    expect(pages.at(-1)).toBe(2);
  });

  it("goToPage clamps to the page range", async () => {
    const { viewer } = await makeViewer();
    await viewer.load(IDML);
    viewer.goToPage(99);
    expect(viewer.currentPage).toBe(2);
    viewer.goToPage(-5);
    expect(viewer.currentPage).toBe(0);
  });

  it("scrolling continuous mode derives the current page", async () => {
    const { viewer } = await makeViewer();
    await viewer.load(IDML);
    const pages: number[] = [];
    viewer.on("pageChanged", (e) => pages.push(e.page));
    // Scroll page 1's middle under the viewport centre.
    const z = viewer.zoom;
    viewer.scrollTo((800 - 612 * z) / 2, 600 / 2 - (816 + 396) * z);
    expect(viewer.currentPage).toBe(1);
    expect(pages).toContain(1);
  });

  it("single mode presents only the current page at y=0", async () => {
    const { viewer, session } = await makeViewer();
    await viewer.load(IDML);
    viewer.layoutMode("single");
    viewer.goToPage(1);
    const last = session.presents.at(-1);
    expect(last?.onlyPage).toBe(1);
    expect(viewer.layoutMode()).toBe("single");
  });

  it("renderPageThumbnail delegates with the requested width", async () => {
    const { viewer, session } = await makeViewer();
    await viewer.load(IDML);
    const raster = await viewer.renderPageThumbnail(2, { width: 240 });
    expect(raster.width).toBe(240);
    expect(session.thumbnails).toEqual([{ index: 2, width: 240 }]);
  });
});

describe("dispose", () => {
  it("detaches input, clears listeners, frees the session", async () => {
    const { viewer, session, canvas } = await makeViewer();
    await viewer.load(IDML);
    const counts = canvas as unknown as {
      listenerCount(type: string): number;
    };
    expect(counts.listenerCount("wheel")).toBe(1);

    let fired = 0;
    viewer.on("zoomChanged", () => fired++);
    viewer.dispose();

    expect(session.freed).toBe(true);
    expect(counts.listenerCount("wheel")).toBe(0);
    expect(counts.listenerCount("pointerdown")).toBe(0);
    expect(counts.listenerCount("keydown")).toBe(0);

    const before = session.presents.length;
    viewer.zoomIn(); // post-dispose: no event, no present
    expect(fired).toBe(0);
    expect(session.presents.length).toBe(before);
    viewer.dispose(); // idempotent
  });
});
