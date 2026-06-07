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

//! Snapshot tier of the LOD cache.
//!
//! The lowest-resolution rendered output a page can have. The page
//! navigator, the minimap, and the canvas at overview zoom all draw
//! from here. Per the canvas spec (§4.4):
//!
//! - Target width: 256–512 px per page.
//! - Lifetime: never evicted.
//! - Regeneration: only when `(layout_generation, numbering_generation)`
//!   advance for the page.
//!
//! Phase 1 produces snapshots on demand through this module; future
//! work adds the atlas packing + LRU bookkeeping that turn the
//! per-page output into a single GPU texture.
//!
//! The wire-format types (`SnapshotPng`, `SnapshotError`) are
//! always compiled so the typed message channel stays wire-
//! compatible across feature configs; the actual rendering
//! functions live behind the `cpu` feature.

use paged_renderer::PageId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tsify_next::Tsify;

#[cfg(feature = "cpu")]
use crate::model::CanvasModel;
#[cfg(feature = "cpu")]
use image::{codecs::png::PngEncoder, ImageEncoder};
#[cfg(feature = "cpu")]
use paged_renderer::render_built_page;

/// A rendered page bitmap. RGBA8, tightly packed (no row padding),
/// row-major from top to bottom. Caller decides whether to encode to
/// PNG, hand to a `<canvas>`, or pack into a WebGPU texture.
#[cfg(feature = "cpu")]
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub page_id: PageId,
    pub width_px: u32,
    pub height_px: u32,
    /// Generation pair from the source `BuiltPage` at the time of
    /// snapshotting. Callers compare against the current page state
    /// to detect staleness before regenerating.
    pub layout_generation: u64,
    pub numbering_generation: u64,
    pub rgba: Vec<u8>,
}

/// Lightweight serialisable variant — the canvas worker hands this
/// (encoded as a `WorkerToMain` message) to the main thread. The
/// `rgba` payload becomes a PNG so the main thread can stash it in
/// an `<img>` or `ImageBitmap` without per-byte serialisation cost.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPng {
    pub page_id: PageId,
    pub width_px: u32,
    pub height_px: u32,
    pub layout_generation: u64,
    pub numbering_generation: u64,
    #[tsify(type = "number[]")]
    pub png_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Error, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase", tag = "kind", content = "details")]
pub enum SnapshotError {
    #[error("unknown page id: {page_id}")]
    UnknownPage { page_id: PageId },
    #[error("png encoding failed: {0}")]
    PngEncode(String),
    #[error("invalid target width: {0}")]
    InvalidWidth(u32),
}

/// Render `page_id` at `target_width_px` wide. Height is derived
/// from the page's aspect ratio. Background is white (matching the
/// renderer's default for `render_document`).
#[cfg(feature = "cpu")]
pub fn render_snapshot(
    model: &CanvasModel,
    page_id: &PageId,
    target_width_px: u32,
) -> Result<Snapshot, SnapshotError> {
    render_snapshot_inner(model, page_id, SnapshotSize::WidthPx(target_width_px))
}

/// Render `page_id` at a specific DPI. Both width and height fall out
/// of the renderer's `RasterOptions` rounding (`ceil(width_pt × dpi / 72)`),
/// so the resulting PNG matches `paged-inspect --dpi <dpi>` exactly —
/// crucial for fidelity diffs against `pdftoppm -r <dpi>`.
#[cfg(feature = "cpu")]
pub fn render_snapshot_at_dpi(
    model: &CanvasModel,
    page_id: &PageId,
    dpi: f32,
) -> Result<Snapshot, SnapshotError> {
    if !(dpi.is_finite() && dpi > 0.0) {
        return Err(SnapshotError::InvalidWidth(0));
    }
    render_snapshot_inner(model, page_id, SnapshotSize::Dpi(dpi))
}

#[cfg(feature = "cpu")]
enum SnapshotSize {
    WidthPx(u32),
    Dpi(f32),
}

#[cfg(feature = "cpu")]
fn render_snapshot_inner(
    model: &CanvasModel,
    page_id: &PageId,
    size: SnapshotSize,
) -> Result<Snapshot, SnapshotError> {
    let page = model
        .page(page_id)
        .ok_or_else(|| SnapshotError::UnknownPage {
            page_id: page_id.clone(),
        })?;
    let dpi = match size {
        SnapshotSize::WidthPx(0) => return Err(SnapshotError::InvalidWidth(0)),
        SnapshotSize::WidthPx(w) => (w as f32) / page.width_pt * 72.0,
        SnapshotSize::Dpi(d) => d,
    };
    let img = render_built_page(page, dpi, paged_compose::Color::WHITE);

    Ok(Snapshot {
        page_id: page_id.clone(),
        width_px: img.width(),
        height_px: img.height(),
        layout_generation: page.layout_generation,
        numbering_generation: page.numbering_generation,
        rgba: img.into_raw(),
    })
}

/// Convenience: render + PNG-encode in one call. The worker uses
/// this on a `RequestSnapshot` message so the PNG bytes can ride
/// directly into the `SnapshotReady` reply.
#[cfg(feature = "cpu")]
pub fn render_snapshot_png(
    model: &CanvasModel,
    page_id: &PageId,
    target_width_px: u32,
) -> Result<SnapshotPng, SnapshotError> {
    let snap = render_snapshot(model, page_id, target_width_px)?;
    encode_snapshot_png(snap)
}

/// Same as `render_snapshot_png` but takes DPI directly. Use this
/// when the caller already has an explicit DPI (e.g. a fidelity
/// suite matching `pdftoppm -r <dpi>` output) and wants to avoid the
/// round-trip through `target_width_px` that drifts by sub-pixel.
#[cfg(feature = "cpu")]
pub fn render_snapshot_png_at_dpi(
    model: &CanvasModel,
    page_id: &PageId,
    dpi: f32,
) -> Result<SnapshotPng, SnapshotError> {
    let snap = render_snapshot_at_dpi(model, page_id, dpi)?;
    encode_snapshot_png(snap)
}

#[cfg(feature = "cpu")]
fn encode_snapshot_png(snap: Snapshot) -> Result<SnapshotPng, SnapshotError> {
    let mut png_bytes = Vec::with_capacity((snap.width_px * snap.height_px) as usize);
    PngEncoder::new(&mut png_bytes)
        .write_image(
            &snap.rgba,
            snap.width_px,
            snap.height_px,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| SnapshotError::PngEncode(e.to_string()))?;
    Ok(SnapshotPng {
        page_id: snap.page_id,
        width_px: snap.width_px,
        height_px: snap.height_px,
        layout_generation: snap.layout_generation,
        numbering_generation: snap.numbering_generation,
        png_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CanvasModel, CanvasOptions};

    // Re-use the minimal IDML fixture from model.rs's tests.
    fn minimal_idml_bytes() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();

            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
            )
            .unwrap();

            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<?aid style="50" type="document" readerVersion="13.0" featureSet="513" product="13.1(255)"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            )
            .unwrap();

            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            )
            .unwrap();

            zip.finish().unwrap();
        }
        buf
    }

    fn load_model() -> CanvasModel {
        let bytes = minimal_idml_bytes();
        CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap()
    }

    #[test]
    fn snapshot_dimensions_track_target_width() {
        let model = load_model();
        let pid = PageId("p1".into());
        let snap = render_snapshot(&model, &pid, 256).expect("snapshot ok");
        assert_eq!(snap.width_px, 256);
        // Letter aspect: 612 / 792. Height should be ≈ 256 * 792/612 = 331.3.
        // Renderer rounds; expect 331 or 332.
        assert!(
            (snap.height_px as i32 - 331).abs() <= 1,
            "expected ~331 height, got {}",
            snap.height_px
        );
        assert_eq!(
            snap.rgba.len(),
            (snap.width_px * snap.height_px * 4) as usize
        );
        assert_eq!(snap.page_id.as_str(), "p1");
        assert_eq!(snap.layout_generation, 0);
        assert_eq!(snap.numbering_generation, 0);
    }

    #[test]
    fn snapshot_background_is_white_for_empty_page() {
        let model = load_model();
        let pid = PageId("p1".into());
        let snap = render_snapshot(&model, &pid, 64).unwrap();
        // No frames on the page -> background fill should be the
        // entire bitmap. Each pixel = (255, 255, 255, 255).
        let centre = ((snap.height_px / 2 * snap.width_px + snap.width_px / 2) * 4) as usize;
        assert_eq!(&snap.rgba[centre..centre + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn unknown_page_returns_typed_error() {
        let model = load_model();
        let err = render_snapshot(&model, &PageId("nope".into()), 256).unwrap_err();
        match err {
            SnapshotError::UnknownPage { page_id } => assert_eq!(page_id.as_str(), "nope"),
            other => panic!("expected UnknownPage, got {other:?}"),
        }
    }

    #[test]
    fn zero_width_rejected() {
        let model = load_model();
        let err = render_snapshot(&model, &PageId("p1".into()), 0).unwrap_err();
        assert!(matches!(err, SnapshotError::InvalidWidth(0)));
    }

    #[test]
    fn png_encoding_produces_decodable_bytes() {
        let model = load_model();
        let snap = render_snapshot_png(&model, &PageId("p1".into()), 128).unwrap();
        assert!(snap.png_bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        // Round-trip: image::load_from_memory should produce the same dimensions.
        let decoded = image::load_from_memory(&snap.png_bytes).unwrap();
        assert_eq!(decoded.width(), snap.width_px);
        assert_eq!(decoded.height(), snap.height_px);
    }
}
