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

//! Raster / vector image decoding: format sniffing (JPEG, SVG, EPS),
//! DCT-scaled JPEG decode for oversized rasters, SVG rasterisation,
//! and lazy dimension peeking. RGBA conversion lives in
//! [`super::image_convert`].

use super::*;

/// Track 1a: longest-edge cap for raster decode. JPEGs whose declared
/// dimensions exceed this on either axis are decoded through
/// `jpeg-decoder`'s DCT scaling (1/2, 1/4, or 1/8) so we never
/// materialise the full RGBA8 buffer — the annual-report-template
/// cover JPEG is 5760×9000 ≈ 198MB at RGBA8, which the previous
/// `image::load_from_memory` path allocated in one shot. 4096px keeps
/// us safely under one rasteriser tile target while still hitting
/// 300dpi for any frame up to ~13.6" on the longest edge.
const DECODE_MAX_RASTER_PX: u32 = 4096;

/// Detect PostScript / EPS magic in the first few bytes of a resolved
/// image buffer. EPS streams start with `%!PS-Adobe` (or `%!PS`); the
/// `image` crate can't decode them, so the caller falls back to the
/// missing-image placeholder rather than emit nothing (P-14).
fn is_eps_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%!PS")
}

/// Decode raw image bytes to RGBA8. Routes oversized JPEGs through
/// `jpeg-decoder`'s DCT scaling so we never materialise a multi-
/// hundred-MB RGBA8 buffer; everything else (PNG / WebP / small JPEGs)
/// goes through `image::load_from_memory`. Returns `None` for any
/// decode or buffer-shape failure — including EPS / PostScript
/// streams, which would need a Ghostscript sidecar to rasterise
/// (deferred, see `docs/plan.md` Phase 4).
pub(super) fn decode_image_bytes(bytes: &[u8]) -> Option<paged_compose::DecodedImage> {
    // wasm32 has a hard 4 GB address-space cap. Eagerly decoding
    // every embedded image to RGBA8 during `build_document` bloats
    // the heap (envato megapacks ship 50+ MB of images that expand
    // to 1-2 GB decoded). Defer the decode to render time — the
    // rasterizer materialises one image at a time and drops after.
    //
    // Native targets have 64-bit addressing; eager decode is cheap
    // there and avoids decoding the same image twice when a page
    // re-renders.
    #[cfg(target_arch = "wasm32")]
    {
        peek_image_lazy(bytes, DECODE_MAX_RASTER_PX)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        decode_image_bytes_with_target_max(bytes, DECODE_MAX_RASTER_PX)
    }
}

/// Lazy variant: returns a `DecodedImage` whose `rgba` is empty and
/// whose `encoded` carries the original bytes. The rasterizer
/// detects the empty `rgba` and runs the full decoder on demand.
/// Width / height come from header peeks where possible (PNG, JPEG,
/// SVG) so the display-list transform stays accurate without paying
/// for the pixel decode.
fn peek_image_lazy(bytes: &[u8], max_px: u32) -> Option<paged_compose::DecodedImage> {
    if is_eps_magic(bytes) {
        return None;
    }
    // SVGs always go through the eager path even on wasm32: usvg's
    // parse is fast (XML-only, no pixel buffer) and we'd parse twice
    // anyway in lazy mode. The cost of the actual rasterize stays
    // bounded — vector → bitmap at the configured max_px.
    if is_svg_magic(bytes) {
        return decode_svg_bytes(bytes, max_px);
    }
    let (width, height) = peek_image_dimensions(bytes, max_px)?;
    Some(paged_compose::DecodedImage {
        width,
        height,
        encoded: bytes::Bytes::copy_from_slice(bytes),
        rgba: bytes::Bytes::new(),
    })
}

/// Sniff for SVG: skip BOM + leading whitespace, then look for the
/// XML preamble or a bare `<svg` root. Adobe-embedded SVGs always
/// carry the XML declaration; loose-format SVGs in arbitrary IDMLs
/// may not.
fn is_svg_magic(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let Ok(s) = std::str::from_utf8(head) else { return false; };
    let s = s.trim_start_matches('\u{FEFF}').trim_start();
    s.starts_with("<?xml") || s.starts_with("<svg") || s.starts_with("<!DOCTYPE svg")
}

/// Decode an SVG via resvg → tiny-skia → RGBA8. The image is scaled
/// so its longest edge lands at most `max_px` (matching the DCT-scale
/// cap for raster images), preserving aspect.
fn decode_svg_bytes(bytes: &[u8], max_px: u32) -> Option<paged_compose::DecodedImage> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let svg_w: f32 = tree.size().width().ceil().max(1.0);
    let svg_h: f32 = tree.size().height().ceil().max(1.0);
    let longest = svg_w.max(svg_h);
    let scale: f32 = if (longest as u32) > max_px {
        (max_px as f32) / longest
    } else {
        1.0
    };
    let w = ((svg_w * scale).ceil() as u32).max(1);
    let h = ((svg_h * scale).ceil() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(paged_compose::DecodedImage {
        width: w,
        height: h,
        encoded: bytes::Bytes::copy_from_slice(bytes),
        rgba: bytes::Bytes::from(pixmap.take()),
    })
}

/// Read width / height from the image header without materialising
/// the pixel buffer. For JPEGs we apply the same DCT-scale logic as
/// the eager decoder so the displayed dimensions match what the
/// rasterizer will actually produce.
fn peek_image_dimensions(bytes: &[u8], max_px: u32) -> Option<(u32, u32)> {
    if is_jpeg_magic(bytes) {
        let (src_w, src_h) = peek_jpeg_dimensions(bytes)?;
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let longest = src_w.max(src_h);
        // Mirror decode_jpeg_scaled's DCT-factor pick (1..8 / 8).
        let k = if longest <= max_px {
            8u32
        } else {
            let mut best: u32 = 1;
            for k in 1..=8u32 {
                if longest * k / 8 <= max_px {
                    best = k;
                }
            }
            best
        };
        let w = (src_w * k / 8).max(1);
        let h = (src_h * k / 8).max(1);
        return Some((w, h));
    }
    // PNG / WebP / etc — header read via the image crate's lazy
    // dimension probe, which doesn't materialise pixels.
    let cursor = std::io::Cursor::new(bytes);
    let reader = image::ImageReader::new(cursor).with_guessed_format().ok()?;
    reader.into_dimensions().ok()
}

/// Same as [`decode_image_bytes`] but with a caller-supplied
/// longest-edge cap. Used by the streaming JPEG path and by the
/// fallback retry on decode failure. JPEGs above the cap are
/// decoded via `jpeg-decoder` with DCT scaling chosen so the longest
/// edge ends up ≤ `max_px`; other formats and small JPEGs fall
/// through to `image::load_from_memory`.
pub(super) fn decode_image_bytes_with_target_max(
    bytes: &[u8],
    max_px: u32,
) -> Option<paged_compose::DecodedImage> {
    if is_eps_magic(bytes) {
        tracing::warn!("EPS / PostScript image detected; emitting missing-image placeholder");
        return None;
    }
    if is_svg_magic(bytes) {
        return decode_svg_bytes(bytes, max_px);
    }
    if is_jpeg_magic(bytes) {
        if let Some((src_w, src_h)) = peek_jpeg_dimensions(bytes) {
            if src_w.max(src_h) > max_px {
                if let Some(d) = decode_jpeg_scaled(bytes, max_px) {
                    return Some(d);
                }
                tracing::debug!(
                    src_w,
                    src_h,
                    max_px,
                    "streaming JPEG decoder rejected oversized payload; falling back to image crate"
                );
            }
        }
    }
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(paged_compose::DecodedImage {
        width,
        height,
        encoded: bytes::Bytes::copy_from_slice(bytes),
        rgba: bytes::Bytes::from(rgba.into_raw()),
    })
}

fn is_jpeg_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

/// Cheap pre-flight: read just the JPEG headers via `jpeg-decoder` to
/// discover declared dimensions without decoding pixel data. Returns
/// `None` if the headers are malformed or the file isn't a JPEG.
fn peek_jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    Some((info.width as u32, info.height as u32))
}

/// Decode a JPEG via `jpeg-decoder`, using its DCT-scaled output mode
/// to land the longest edge ≤ `max_px`. Scaling is restricted to the
/// JPEG-native factors (1, 1/2, 1/4, 1/8); the decoder picks the
/// smallest factor whose output is ≥ the requested target.
fn decode_jpeg_scaled(bytes: &[u8], max_px: u32) -> Option<paged_compose::DecodedImage> {
    use jpeg_decoder::{Decoder, PixelFormat};
    let mut decoder = Decoder::new(bytes);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    let src_w = info.width as u32;
    let src_h = info.height as u32;
    if src_w == 0 || src_h == 0 {
        return None;
    }
    let longest = src_w.max(src_h);
    // jpeg-decoder picks the smallest DCT scale `k` (1..=8 in 1/8ths
    // of full resolution) whose output ≥ the requested dimensions,
    // so requesting `max_px` directly would round UP past the cap.
    // Instead, pick the largest `k` where `longest * k / 8 ≤ max_px`
    // ourselves and request the resulting size verbatim — the
    // decoder then returns exactly that scale. When no scale fits
    // (`max_px` < `longest / 8`) we fall back to `k = 1` (1/8 — the
    // smallest DCT-supported output) and accept the cap overshoot.
    let k = if longest <= max_px {
        8
    } else {
        let mut best: u32 = 1;
        for k in 1..=8u32 {
            if longest * k / 8 <= max_px {
                best = k;
            }
        }
        best
    };
    let target_w = (src_w * k / 8).max(1).min(u16::MAX as u32) as u16;
    let target_h = (src_h * k / 8).max(1).min(u16::MAX as u32) as u16;
    let (final_w, final_h) = decoder.scale(target_w, target_h).ok()?;
    let pixels = decoder.decode().ok()?;
    let info_after = decoder.info()?;
    let icc_profile = decoder.icc_profile();
    let w = final_w as u32;
    let h = final_h as u32;
    let rgba = match info_after.pixel_format {
        PixelFormat::L8 => l8_to_rgba(&pixels, w, h)?,
        PixelFormat::L16 => l16_to_rgba(&pixels, w, h)?,
        PixelFormat::RGB24 => rgb24_to_rgba(&pixels, w, h)?,
        PixelFormat::CMYK32 => cmyk32_to_rgba(&pixels, w, h, icc_profile.as_deref())?,
    };
    Some(paged_compose::DecodedImage {
        width: w,
        height: h,
        encoded: bytes::Bytes::copy_from_slice(bytes),
        rgba: bytes::Bytes::from(rgba),
    })
}
