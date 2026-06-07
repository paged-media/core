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

//! Image XObjects — the quality differentiators (concept E7):
//! placed JPEGs pass through as DCTDecode WITHOUT re-encoding (no
//! generational loss); everything else embeds as FlateDecode from
//! the decoded RGBA; alpha rides as an /SMask; an embedded ICC
//! profile (when the decode retained it) tags the colour space.

use std::io::Write as _;

use paged_compose::DecodedImage;
use pdf_writer::{Finish, Name, Ref};

use crate::writer::DocState;
use crate::ExportDiagnostic;

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() > 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

/// Component count from the JPEG SOF header (1 = gray, 3 = YCbCr/RGB,
/// 4 = CMYK/YCCK). Determines the /ColorSpace a DCT passthrough must
/// carry — tagging a CMYK JPEG as RGB renders garbage.
fn jpeg_components(bytes: &[u8]) -> Option<u8> {
    let mut i = 2usize;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            return None;
        }
        // SOF0..SOF15 minus DHT/JPG/DAC (C4, C8, CC).
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            // SOF payload: precision(1) h(2) w(2) components(1).
            return bytes.get(i + 9).copied();
        }
        i += 2 + len;
    }
    None
}

/// /N for an ICC stream, from the profile header's data colour
/// space field (bytes 16..20).
fn icc_component_count(icc: &[u8]) -> i32 {
    match icc.get(16..20) {
        Some(b"CMYK") => 4,
        Some(b"GRAY") => 1,
        _ => 3,
    }
}

fn flate(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

/// Write one image (+ optional alpha SMask) and return its XObject
/// ref. Returns `None` when neither encoded nor decoded pixels are
/// available (a diagnostic is pushed).
pub fn write_image(
    state: &mut DocState,
    img: &DecodedImage,
    image_index: u32,
    placed_size_pt: Option<(f32, f32)>,
    options: &crate::ImageOptions,
    diagnostics: &mut Vec<ExportDiagnostic>,
) -> Option<Ref> {
    // Downsampling (off by default): when the placed resolution
    // exceeds the target, resample and embed Flate — the DCT
    // passthrough is skipped for that image.
    let downsample_to = options.downsample_ppi.and_then(|target| {
        let (pw, ph) = placed_size_pt?;
        if pw <= 0.0 || ph <= 0.0 {
            return None;
        }
        let eff = (img.width as f32 / (pw / 72.0)).max(img.height as f32 / (ph / 72.0));
        if eff > target * 1.25 {
            // Cap the longest edge so the result lands AT target ppi.
            let scale = target / eff;
            let w = ((img.width as f32 * scale).round() as u32).max(1);
            let h = ((img.height as f32 * scale).round() as u32).max(1);
            Some((w, h))
        } else {
            None
        }
    });

    // DCT passthrough — the original bytes, verbatim.
    if downsample_to.is_none() && !img.encoded.is_empty() && is_jpeg(&img.encoded) {
        // ICC stream first (separate borrow), then the XObject.
        let icc_ref = img
            .icc
            .as_ref()
            .filter(|icc| !icc.is_empty())
            .map(|icc| write_icc(state, icc));
        let components = jpeg_components(&img.encoded).unwrap_or(3);
        let img_ref = state.refs.alloc();
        let mut x = state.pdf.image_xobject(img_ref, &img.encoded);
        x.width(img.width as i32);
        x.height(img.height as i32);
        x.bits_per_component(8);
        // Colour space from the SOF component count — CMYK JPEGs
        // pass through as DeviceCMYK (viewers honour the in-stream
        // Adobe APP14 transform), gray as DeviceGray. ICC tagging
        // when the decode retained a profile.
        match icc_ref {
            Some(r) => {
                x.color_space().icc_based(r);
            }
            None => match components {
                4 => {
                    x.color_space().device_cmyk();
                }
                1 => {
                    x.color_space().device_gray();
                }
                _ => {
                    x.color_space().device_rgb();
                }
            },
        }
        x.filter(pdf_writer::Filter::DctDecode);
        x.finish();
        return Some(img_ref);
    }

    // Flate path from decoded RGBA (lazy-decode when only encoded
    // bytes exist — mirror the rasterizer's fallback).
    let rgba: bytes::Bytes = if !img.rgba.is_empty() {
        img.rgba.clone()
    } else if !img.encoded.is_empty() {
        match image::load_from_memory(&img.encoded) {
            Ok(decoded) => bytes::Bytes::from(decoded.to_rgba8().into_raw()),
            Err(_) => {
                diagnostics.push(ExportDiagnostic::ImageMissingBytes { image_index });
                return None;
            }
        }
    } else {
        diagnostics.push(ExportDiagnostic::ImageMissingBytes { image_index });
        return None;
    };

    let n = (img.width as usize) * (img.height as usize);
    if rgba.len() < n * 4 {
        diagnostics.push(ExportDiagnostic::ImageMissingBytes { image_index });
        return None;
    }

    // Bicubic resample to the downsampling target (Catmull-Rom).
    let (out_w, out_h, rgba) = match downsample_to {
        Some((w, h)) => {
            let buf = image::RgbaImage::from_raw(img.width, img.height, rgba.to_vec())?;
            let resized =
                image::imageops::resize(&buf, w, h, image::imageops::FilterType::CatmullRom);
            (w, h, bytes::Bytes::from(resized.into_raw()))
        }
        None => (img.width, img.height, rgba),
    };

    let n = (out_w as usize) * (out_h as usize);
    let mut rgb = Vec::with_capacity(n * 3);
    let mut alpha = Vec::with_capacity(n);
    let mut has_alpha = false;
    for px in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
        alpha.push(px[3]);
        if px[3] != 255 {
            has_alpha = true;
        }
    }

    let smask_ref = if has_alpha {
        let r = state.refs.alloc();
        let data = flate(&alpha);
        let mut x = state.pdf.image_xobject(r, &data);
        x.width(out_w as i32);
        x.height(out_h as i32);
        x.bits_per_component(8);
        x.color_space().device_gray();
        x.filter(pdf_writer::Filter::FlateDecode);
        x.finish();
        Some(r)
    } else {
        None
    };

    // The pixels are RGBA from the decode — a CMYK source profile
    // would mis-tag them, so only ICC-tag when the profile is RGB.
    let icc_ref = img
        .icc
        .as_ref()
        .filter(|icc| !icc.is_empty() && icc_component_count(icc) == 3)
        .map(|icc| write_icc(state, icc));
    let img_ref = state.refs.alloc();
    let data = flate(&rgb);
    let mut x = state.pdf.image_xobject(img_ref, &data);
    x.width(out_w as i32);
    x.height(out_h as i32);
    x.bits_per_component(8);
    match icc_ref {
        Some(r) => {
            x.color_space().icc_based(r);
        }
        None => {
            x.color_space().device_rgb();
        }
    }
    x.filter(pdf_writer::Filter::FlateDecode);
    if let Some(s) = smask_ref {
        x.s_mask(s);
    }
    x.finish();
    Some(img_ref)
}

fn write_icc(state: &mut DocState, icc: &[u8]) -> Ref {
    let r = state.refs.alloc();
    let mut s = state.pdf.icc_profile(r, icc);
    s.n(icc_component_count(icc));
    s.finish();
    r
}

/// Intern the image XObject name on the page resources.
pub fn image_resource_name(index: u32) -> String {
    format!("Im{index}")
}

#[allow(unused)]
fn _name_check(_: Name) {}
