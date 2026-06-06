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
 * The typed contract of the wasm `ViewerSession`
 * (`core/crates/paged-sdk/src/lib.rs`). The viewer programs against
 * this interface so tests can inject a fake session and embedders can
 * wrap their own init flow. Method names mirror the wasm-bindgen
 * exports (snake_case); the viewer surface re-exposes everything in
 * idiomatic camelCase.
 */

/** One diagnostic from `load` / `render_*`. */
export interface SessionDiagnostic {
  /** `"error" | "warning" | "info"`. */
  severity: string;
  /** Short machine code, e.g. `"open"`, `"build"`, `"no_gpu"`. */
  code: string;
  message: string;
  part?: string | null;
  line?: number | null;
}

export interface SessionDiagnostics {
  ok: boolean;
  messages: SessionDiagnostic[];
}

/** Continuous-layout geometry from `page_layout()`. */
export interface SessionPageRect {
  index: number;
  yPt: number;
  widthPt: number;
  heightPt: number;
}

export interface SessionPagesLayout {
  gapPt: number;
  pages: SessionPageRect[];
}

export interface SessionRaster {
  width: number;
  height: number;
  rgba: Uint8Array;
}

export interface ViewerSessionLike {
  load(idml: Uint8Array, font?: Uint8Array): SessionDiagnostics;
  register_font(family: string, style: string | undefined, bytes: Uint8Array): void;
  page_count(): number;
  set_page(index: number): void;
  page_layout(): SessionPagesLayout;
  /**
   * Camera-transformed present of the continuous page stack. `zoom`
   * is CSS px per pt; `scrollX`/`scrollY` place the doc origin in CSS
   * px; `onlyPage` restricts to one page laid out at y = 0.
   */
  present(
    zoom: number,
    scrollX: number,
    scrollY: number,
    dpr: number,
    onlyPage?: number | null,
  ): SessionDiagnostics;
  /** Binds the presenter to `canvas` on first call (fit-page paint). */
  render_to_canvas_main(canvas: HTMLCanvasElement): Promise<SessionDiagnostics>;
  render_page_to_bytes(index: number, targetWidthPx: number): Promise<SessionRaster>;
  resize(width: number, height: number, devicePixelRatio: number): void;
  /** wasm-bindgen finalizer. */
  free?(): void;
}
