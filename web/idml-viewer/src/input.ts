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
 * Input lanes for the viewer — each independently disableable:
 * - wheel: Ctrl/⌘-wheel and pinch (`ctrlKey` wheel) zoom to cursor,
 *   plain wheel pans.
 * - drag: pointer-drag pans.
 * - double-click: zoom step at the cursor.
 * - keyboard: `+`/`-`/`0`, arrows, PgUp/PgDn, Home/End.
 */

export interface InputOptions {
  wheelZoom?: boolean;
  dragPan?: boolean;
  doubleClickZoom?: boolean;
  keyboard?: boolean;
}

export interface InputSink {
  zoomAt(factor: number, anchor: { x: number; y: number }): void;
  panBy(dx: number, dy: number): void;
  zoomStep(direction: 1 | -1, anchor?: { x: number; y: number }): void;
  fit(): void;
  pageStep(direction: 1 | -1): void;
  home(): void;
  end(): void;
}

const ARROW_PAN_PX = 48;

/** Attach listeners; returns the detach function. */
export function attachInput(
  canvas: HTMLCanvasElement,
  options: InputOptions,
  sink: InputSink,
): () => void {
  const teardown: Array<() => void> = [];
  const listen = <K extends keyof HTMLElementEventMap>(
    type: K,
    handler: (event: HTMLElementEventMap[K]) => void,
    opts?: AddEventListenerOptions,
  ): void => {
    canvas.addEventListener(type, handler as EventListener, opts);
    teardown.push(() =>
      canvas.removeEventListener(type, handler as EventListener, opts),
    );
  };

  const anchorOf = (e: { clientX: number; clientY: number }) => {
    const rect = canvas.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };

  if (options.wheelZoom !== false) {
    listen(
      "wheel",
      (e) => {
        e.preventDefault();
        if (e.ctrlKey || e.metaKey) {
          // Trackpad pinch arrives as ctrlKey wheel; exponential map
          // keeps pinch and wheel-notch speeds comparable.
          sink.zoomAt(Math.exp(-e.deltaY * 0.0024), anchorOf(e));
        } else {
          sink.panBy(-e.deltaX, -e.deltaY);
        }
      },
      { passive: false },
    );
  }

  if (options.dragPan !== false) {
    let dragging = false;
    let lastX = 0;
    let lastY = 0;
    listen("pointerdown", (e) => {
      if (e.button !== 0) return;
      dragging = true;
      lastX = e.clientX;
      lastY = e.clientY;
      canvas.setPointerCapture?.(e.pointerId);
    });
    listen("pointermove", (e) => {
      if (!dragging) return;
      sink.panBy(e.clientX - lastX, e.clientY - lastY);
      lastX = e.clientX;
      lastY = e.clientY;
    });
    const stop = (e: PointerEvent) => {
      dragging = false;
      canvas.releasePointerCapture?.(e.pointerId);
    };
    listen("pointerup", stop);
    listen("pointercancel", stop);
  }

  if (options.doubleClickZoom !== false) {
    listen("dblclick", (e) => {
      e.preventDefault();
      sink.zoomStep(e.shiftKey ? -1 : 1, anchorOf(e));
    });
  }

  if (options.keyboard !== false) {
    // The canvas needs focusability for keydown.
    if (!canvas.hasAttribute("tabindex")) canvas.setAttribute("tabindex", "0");
    listen("keydown", (e) => {
      switch (e.key) {
        case "+":
        case "=":
          sink.zoomStep(1);
          break;
        case "-":
        case "_":
          sink.zoomStep(-1);
          break;
        case "0":
          sink.fit();
          break;
        case "ArrowLeft":
          sink.panBy(ARROW_PAN_PX, 0);
          break;
        case "ArrowRight":
          sink.panBy(-ARROW_PAN_PX, 0);
          break;
        case "ArrowUp":
          sink.panBy(0, ARROW_PAN_PX);
          break;
        case "ArrowDown":
          sink.panBy(0, -ARROW_PAN_PX);
          break;
        case "PageUp":
          sink.pageStep(-1);
          break;
        case "PageDown":
          sink.pageStep(1);
          break;
        case "Home":
          sink.home();
          break;
        case "End":
          sink.end();
          break;
        default:
          return;
      }
      e.preventDefault();
    });
  }

  return () => {
    for (const detach of teardown) detach();
  };
}
