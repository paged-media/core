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

use idml_renderer::PageId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "cpu")]
use crate::model::CanvasModel;
#[cfg(feature = "cpu")]
use idml_renderer::render_built_page;
#[cfg(feature = "cpu")]
use image::{codecs::png::PngEncoder, ImageEncoder};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPng {
    pub page_id: PageId,
    pub width_px: u32,
    pub height_px: u32,
    pub layout_generation: u64,
    pub numbering_generation: u64,
    pub png_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
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
    if target_width_px == 0 {
        return Err(SnapshotError::InvalidWidth(target_width_px));
    }
    let page = model
        .page(page_id)
        .ok_or_else(|| SnapshotError::UnknownPage {
            page_id: page_id.clone(),
        })?;
    // DPI to produce exactly `target_width_px` columns. A Letter
    // page (612 pt) at 256 px wide gives ~30 dpi; the spec calls
    // for 256–512 px snapshots, which lands in the 30–60 dpi
    // range — well below print resolutions, just enough for a
    // recognisable thumbnail.
    let dpi = (target_width_px as f32) / page.width_pt * 72.0;
    let img = render_built_page(page, dpi, idml_compose::Color::WHITE);

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
            let opts: zip::write::SimpleFileOptions =
                zip::write::SimpleFileOptions::default()
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
        assert_eq!(snap.rgba.len(), (snap.width_px * snap.height_px * 4) as usize);
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
