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

export { createViewer, ViewerError } from "./viewer.js";
export { createSessionFromBundledWasm } from "./bootstrap.js";
export type {
  CreateViewerOptions,
  LayoutMode,
  Viewer,
  ViewerErrorCode,
  ViewerEvents,
} from "./viewer.js";
export type { InputOptions } from "./input.js";
export type {
  SessionDiagnostic,
  SessionDiagnostics,
  SessionPageRect,
  SessionPagesLayout,
  SessionRaster,
  ViewerSessionLike,
} from "./session.js";
export {
  clampScroll,
  clampZoom,
  contentExtent,
  currentPageAt,
  fitPage,
  scrollToPage,
  zoomAt,
} from "./camera.js";
export type { Camera, Viewport, ZoomLimits } from "./camera.js";
