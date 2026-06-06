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

import type { ViewerSessionLike } from "./session.js";

/**
 * Create a `ViewerSession` from the wasm bundled with the PUBLISHED
 * package (`wasm/paged_sdk.js` + `wasm/paged_sdk_bg.wasm`, placed by
 * the publish workflow — absent in a source checkout, where embedders
 * inject a session built via `viewer/build-wasm.sh` instead).
 *
 * `wasmUrl` overrides the `.wasm` location for bundlers that move
 * assets; default resolves next to the glue file.
 */
export async function createSessionFromBundledWasm(
  wasmUrl?: string,
): Promise<ViewerSessionLike> {
  const glueUrl = new URL("../wasm/paged_sdk.js", import.meta.url).href;
  // Dynamic, computed import — left untouched by TS and bundlers so
  // the glue resolves at runtime inside the published package.
  const mod = (await import(/* @vite-ignore */ glueUrl)) as {
    default: (init?: { module_or_path: string }) => Promise<unknown>;
    // `new` here is wasm-bindgen's STATIC factory (async), not a
    // construct signature — hence the quoted method name.
    ViewerSession: { ["new"](): Promise<ViewerSessionLike> };
  };
  await mod.default(wasmUrl ? { module_or_path: wasmUrl } : undefined);
  return mod.ViewerSession["new"]();
}
