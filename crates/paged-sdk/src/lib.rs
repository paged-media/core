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

//! wasm-bindgen surface.
//!
//! Wraps `paged-renderer` behind a small browser-facing API:
//!
//! ```ts
//! import init, { render_to_png, parse_summary } from 'paged-sdk';
//! await init();
//! const png = render_to_png(idmlBytes, fontBytes, 144);
//! ```
//!
//! Native builds expose a plain library target so the crate can still
//! participate in `cargo check --workspace`.

#[cfg(target_arch = "wasm32")]
mod wasm {
    use paged_compose::Color;
    use paged_renderer::{pipeline, Document, PipelineOptions};
    use image::{codecs::png::PngEncoder, ImageEncoder};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"paged-sdk: init".into());
    }

    /// Render an IDML to a PNG.
    ///
    /// `idml` is the container bytes. `font` is optional — when absent,
    /// text is skipped and only frame rectangles are drawn. `dpi`
    /// controls output resolution (72 = 1 px per pt, 300 = print).
    #[wasm_bindgen]
    pub fn render_to_png(
        idml: &[u8],
        font: Option<Box<[u8]>>,
        dpi: f32,
    ) -> Result<Vec<u8>, JsError> {
        let mut pngs = render_pages_inner(idml, font, dpi)?;
        if pngs.is_empty() {
            return Err(JsError::new("document has no pages"));
        }
        Ok(pngs.swap_remove(0))
    }

    /// Render every page of an IDML and return the PNG bytes as a
    /// JS `Array<Uint8Array>`. Hosts iterate this to display the
    /// document page-by-page; the array order matches body-page
    /// order in the IDML manifest.
    #[wasm_bindgen]
    pub fn render_pages(
        idml: &[u8],
        font: Option<Box<[u8]>>,
        dpi: f32,
    ) -> Result<js_sys::Array, JsError> {
        let pngs = render_pages_inner(idml, font, dpi)?;
        let arr = js_sys::Array::new();
        for png in pngs {
            let u8a = js_sys::Uint8Array::new_with_length(png.len() as u32);
            u8a.copy_from(&png);
            arr.push(&u8a);
        }
        Ok(arr)
    }

    /// Lightweight structural report — page sizes + pipeline stats —
    /// without rasterising. Hosts can size canvases and show counts
    /// before kicking off a full render.
    #[wasm_bindgen]
    pub fn render_report(idml: &[u8]) -> Result<String, JsError> {
        let document =
            Document::open(idml).map_err(|e| JsError::new(&format!("open IDML: {e}")))?;
        let opts = PipelineOptions::default();
        let built = pipeline::build_document(&document, &opts)
            .map_err(|e| JsError::new(&format!("build: {e}")))?;
        let mut pages = String::from("[");
        for (i, page) in built.pages.iter().enumerate() {
            if i > 0 {
                pages.push(',');
            }
            pages.push_str(&format!(
                "{{\"index\":{},\"width_pt\":{:.3},\"height_pt\":{:.3}}}",
                i, page.width_pt, page.height_pt,
            ));
        }
        pages.push(']');
        Ok(format!(
            "{{\"pages\":{pages},\"stats\":{{\"spreads\":{},\"pages\":{},\
             \"frames\":{},\"stories\":{},\"paragraphs\":{},\"runs\":{}}}}}",
            built.stats.spreads,
            built.stats.pages,
            built.stats.frames,
            built.stats.stories,
            built.stats.paragraphs,
            built.stats.runs,
        ))
    }

    fn render_pages_inner(
        idml: &[u8],
        font: Option<Box<[u8]>>,
        dpi: f32,
    ) -> Result<Vec<Vec<u8>>, JsError> {
        let document =
            Document::open(idml).map_err(|e| JsError::new(&format!("open IDML: {e}")))?;
        let font_slice = font.as_deref();
        let opts = PipelineOptions {
            font: font_slice,
            ..PipelineOptions::default()
        };
        let (_built, images) = pipeline::render_document(&document, &opts, dpi, Color::WHITE)
            .map_err(|e| JsError::new(&format!("render: {e}")))?;
        let mut out = Vec::with_capacity(images.len());
        for img in images {
            let mut buf = Vec::with_capacity((img.width() * img.height() * 4) as usize);
            PngEncoder::new(&mut buf)
                .write_image(
                    img.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| JsError::new(&format!("png encode: {e}")))?;
            out.push(buf);
        }
        Ok(out)
    }

    /// Report parse + pipeline stats as a JSON string. Useful for the
    /// host to display counts without running a full raster.
    #[wasm_bindgen]
    pub fn parse_summary(idml: &[u8]) -> Result<String, JsError> {
        let document =
            Document::open(idml).map_err(|e| JsError::new(&format!("open IDML: {e}")))?;
        let opts = PipelineOptions::default();
        let built = pipeline::build_document(&document, &opts)
            .map_err(|e| JsError::new(&format!("build: {e}")))?;
        let total_cmds: usize = built.pages.iter().map(|p| p.list.commands.len()).sum();
        let total_paths: usize = built.pages.iter().map(|p| p.list.paths.len()).sum();
        Ok(format!(
            "{{\"page_count\":{},\"commands\":{},\"paths\":{},\
             \"spreads\":{},\"pages\":{},\"frames\":{},\
             \"stories\":{},\"paragraphs\":{},\"runs\":{}}}",
            built.pages.len(),
            total_cmds,
            total_paths,
            built.stats.spreads,
            built.stats.pages,
            built.stats.frames,
            built.stats.stories,
            built.stats.paragraphs,
            built.stats.runs,
        ))
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

// Non-wasm builds keep the library buildable — important for
// `cargo check --workspace` on native hosts and for `cargo doc`.
#[cfg(not(target_arch = "wasm32"))]
pub mod native_shim {
    //! Stub surface that makes the crate compile on native targets.
    //! The real API is only available when built for wasm32.

    pub fn is_wasm() -> bool {
        false
    }
}
