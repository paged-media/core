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

//! Raw pixel-format conversions to RGBA: greyscale (L8/L16), RGB24,
//! and CMYK32 (ICC-managed via lcms2 when a profile is present,
//! naive Adobe-style multiplicative fallback otherwise).

pub(super) fn l8_to_rgba(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let expected = (w as usize).checked_mul(h as usize)?;
    if src.len() != expected {
        return None;
    }
    let mut rgba = Vec::with_capacity(expected.checked_mul(4)?);
    for &g in src {
        rgba.extend_from_slice(&[g, g, g, 255]);
    }
    Some(rgba)
}

pub(super) fn l16_to_rgba(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(2)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    for chunk in src.chunks_exact(2) {
        // jpeg-decoder writes L16 big-endian.
        let g16 = u16::from_be_bytes([chunk[0], chunk[1]]);
        let g8 = (g16 >> 8) as u8;
        rgba.extend_from_slice(&[g8, g8, g8, 255]);
    }
    Some(rgba)
}

pub(super) fn rgb24_to_rgba(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(3)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    for chunk in src.chunks_exact(3) {
        rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
    }
    Some(rgba)
}

/// Track 1b dispatcher: route a CMYK JPEG buffer through the embedded
/// ICC profile when present (and the platform supports lcms2),
/// falling back to the Adobe-naive multiplicative form on
/// missing/invalid profiles or wasm32 targets.
pub(super) fn cmyk32_to_rgba(src: &[u8], w: u32, h: u32, icc_profile: Option<&[u8]>) -> Option<Vec<u8>> {
    if let Some(profile) = icc_profile {
        match paged_color::IccTransform::cmyk_to_linear_rgb(profile) {
            Ok(xform) => {
                if let Some(rgba) = cmyk32_to_rgba_via_icc(src, w, h, &xform) {
                    tracing::debug!(
                        target: "paged_renderer::icc",
                        profile_bytes = profile.len(),
                        w,
                        h,
                        "CMYK JPEG decoded via embedded ICC profile"
                    );
                    return Some(rgba);
                }
                tracing::warn!("CMYK JPEG ICC transform produced wrong-shape output; using naive");
            }
            Err(err) => {
                tracing::warn!(error = %err, "CMYK JPEG ICC profile rejected; using naive");
            }
        }
    } else {
        tracing::debug!(
            target: "paged_renderer::icc",
            w,
            h,
            "CMYK JPEG carries no embedded ICC profile; naive multiplicative"
        );
    }
    cmyk32_to_rgba_naive(src, w, h)
}

/// Batch CMYK-8 → sRGB-byte transform via lcms2. Chunked so peak
/// intermediate memory stays bounded (the largest legal output at
/// the decode cap is 4096×4096 ≈ 64MB CMYK input + ~48MB lcms2
/// scratch + 64MB RGBA output; chunking drops the scratch to ~28KB).
fn cmyk32_to_rgba_via_icc(
    src: &[u8],
    w: u32,
    h: u32,
    xform: &paged_color::IccTransform,
) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(4)? {
        return None;
    }
    const CHUNK: usize = 4096;
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    let mut cmyk_buf: Vec<[u8; 4]> = vec![[0; 4]; CHUNK];
    let mut rgb_buf: Vec<[u8; 3]> = vec![[0; 3]; CHUNK];
    for src_chunk in src.chunks(CHUNK * 4) {
        let n = src_chunk.len() / 4;
        for i in 0..n {
            cmyk_buf[i] = [
                src_chunk[i * 4],
                src_chunk[i * 4 + 1],
                src_chunk[i * 4 + 2],
                src_chunk[i * 4 + 3],
            ];
        }
        xform.cmyk_bytes_to_rgb_bytes(&cmyk_buf[..n], &mut rgb_buf[..n]);
        for i in 0..n {
            rgba.extend_from_slice(&[rgb_buf[i][0], rgb_buf[i][1], rgb_buf[i][2], 255]);
        }
    }
    Some(rgba)
}

/// Naive Adobe-style CMYK → sRGB. The Adobe CMYK-JPEG convention
/// stores channels inverted (byte 255 = no ink) so the multiplicative
/// form simplifies to `R = C_byte * K_byte / 255` etc. Used as the
/// fallback when no ICC profile is present (or on wasm32 where lcms2
/// is unavailable).
fn cmyk32_to_rgba_naive(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(4)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    for chunk in src.chunks_exact(4) {
        let c = chunk[0] as u32;
        let m = chunk[1] as u32;
        let y = chunk[2] as u32;
        let k = chunk[3] as u32;
        let r = (c * k / 255) as u8;
        let g = (m * k / 255) as u8;
        let b = (y * k / 255) as u8;
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    Some(rgba)
}
