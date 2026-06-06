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

//! The viewer's single load path — target-independent so the native
//! digest-equivalence test exercises EXACTLY the code
//! `ViewerSession::load` runs in the browser ("same code, same
//! scene"). Any viewer-specific divergence from the engine's stock
//! `build_document` must happen here, where the test will see it.

use paged_renderer::{pipeline, BuiltDocument, BytesResolver, Document, PipelineOptions};

/// Parse-and-build as the viewer does it: stock `PipelineOptions`
/// plus the session's font fallback + registered-face resolver. No
/// exporter side-channels (`collect_glyph_runs` /
/// `collect_link_regions` stay off — the viewer only paints).
pub fn viewer_build(
    document: &Document,
    font: Option<&[u8]>,
    fonts: &BytesResolver,
) -> Result<BuiltDocument, String> {
    let opts = PipelineOptions {
        font,
        assets: Some(fonts),
        ..PipelineOptions::default()
    };
    pipeline::build_document(document, &opts).map_err(|e| format!("build: {e}"))
}
