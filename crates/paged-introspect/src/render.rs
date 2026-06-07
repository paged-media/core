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

//! Render a single page of a [`Document`] to PNG bytes. The inspector's
//! render pane fetches this on selection/mutation and displays the
//! returned bytes via `<img src="data:image/png;base64,...">`.
//!
//! Reuses `paged-renderer::pipeline::render_document` to produce the
//! full document, then PNG-encodes the requested page. There is no
//! incremental-render optimisation yet — every render call re-builds
//! the whole document. For the inspector's interactive cadence on
//! moderately-sized fixtures this is acceptable; profiling will tell
//! us when it isn't.

use image::ImageEncoder;
use paged_compose::Color;
use paged_renderer::pipeline;
use paged_scene::Document;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("page index {0} out of range (document has {1} pages)")]
    PageOutOfRange(usize, usize),
    #[error("render pipeline failure: {0}")]
    Pipeline(#[from] anyhow::Error),
    #[error("png encode failure: {0}")]
    PngEncode(#[from] image::ImageError),
}

pub fn render_page_png(
    document: &Document,
    page_index: usize,
    dpi: f32,
) -> Result<Vec<u8>, RenderError> {
    let opts = pipeline::PipelineOptions::default();
    let (_built, images) = pipeline::render_document(document, &opts, dpi, Color::WHITE)?;
    if page_index >= images.len() {
        return Err(RenderError::PageOutOfRange(page_index, images.len()));
    }
    let img = &images[page_index];
    let mut buf = Vec::with_capacity((img.width() * img.height() * 4) as usize);
    image::codecs::png::PngEncoder::new(&mut buf).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(buf)
}
