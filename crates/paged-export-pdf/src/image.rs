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

fn flate(data: &[u8]) -> Vec<u8> {
    let mut enc =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
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
    diagnostics: &mut Vec<ExportDiagnostic>,
) -> Option<Ref> {
    // DCT passthrough — the original bytes, verbatim.
    if !img.encoded.is_empty() && is_jpeg(&img.encoded) {
        // ICC stream first (separate borrow), then the XObject.
        let icc_ref = img
            .icc
            .as_ref()
            .filter(|icc| !icc.is_empty())
            .map(|icc| write_icc(state, icc, 3));
        let img_ref = state.refs.alloc();
        let mut x = state.pdf.image_xobject(img_ref, &img.encoded);
        x.width(img.width as i32);
        x.height(img.height as i32);
        x.bits_per_component(8);
        // Baseline JPEG from placed assets is RGB (CMYK JPEGs were
        // converted at decode). ICC tagging when retained.
        match icc_ref {
            Some(r) => {
                x.color_space().icc_based(r);
            }
            None => {
                x.color_space().device_rgb();
            }
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
        x.width(img.width as i32);
        x.height(img.height as i32);
        x.bits_per_component(8);
        x.color_space().device_gray();
        x.filter(pdf_writer::Filter::FlateDecode);
        x.finish();
        Some(r)
    } else {
        None
    };

    let icc_ref = img
        .icc
        .as_ref()
        .filter(|icc| !icc.is_empty())
        .map(|icc| write_icc(state, icc, 3));
    let img_ref = state.refs.alloc();
    let data = flate(&rgb);
    let mut x = state.pdf.image_xobject(img_ref, &data);
    x.width(img.width as i32);
    x.height(img.height as i32);
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

fn write_icc(state: &mut DocState, icc: &[u8], n: i32) -> Ref {
    let r = state.refs.alloc();
    let mut s = state.pdf.icc_profile(r, icc);
    s.n(n);
    s.finish();
    r
}

/// Intern the image XObject name on the page resources.
pub fn image_resource_name(index: u32) -> String {
    format!("Im{index}")
}

#[allow(unused)]
fn _name_check(_: Name) {}
