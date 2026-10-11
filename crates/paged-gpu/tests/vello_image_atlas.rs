// Copyright the paged-media authors.
// SPDX-License-Identifier: MPL-2.0 OR LicenseRef-PMEL
#![cfg(all(feature = "vello-backend", not(target_arch = "wasm32")))]

//! A placed image must survive a frame that draws none.
//!
//! Vello keeps placed images resident in one atlas texture across
//! renders. A scene with no late-bound resources at all (no image, no
//! gradient, no glyph run — solid fills and strokes only, which is what
//! a text-as-outlines page without pictures encodes to) takes a fast
//! path that drops that atlas while the residency bookkeeping still
//! says every image is uploaded. The next frame that shows the images
//! gets a fresh, blank atlas and uploads nothing, so they paint as
//! transparent until they are evicted. In the editor that was every
//! picture on a page scrolled to past an image-free one.

use paged_compose::{
    Color, DecodedImage, DisplayCommand as Cmd, DisplayList, Paint, PathData, PathSegment,
    Transform as XF,
};
use paged_gpu::{vello_rs::VelloRasterizer, PathRasterizer, RasterOptions};

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// A page holding one opaque image, 918 × 1224 like the site's
/// thumbnails, so the atlas has to grow past its 1024 px default.
fn page_with_image() -> DisplayList {
    let (w, h) = (918u32, 1224u32);
    let mut list = DisplayList::new();
    let rgba: Vec<u8> = [200u8, 40, 40, 255].repeat((w * h) as usize);
    let image_id = list.push_image(DecodedImage {
        width: w,
        height: h,
        encoded: Vec::new().into(),
        rgba: rgba.into(),
        icc: None,
    });
    list.commands.push(Cmd::Image {
        image_id,
        transform: XF([40.0, 0.0, 0.0, 40.0, 0.0, 0.0]),
    });
    list
}

/// A page of solid fills only: nothing late-bound for Vello to resolve.
fn page_of_solid_fills() -> DisplayList {
    let mut list = DisplayList::new();
    let mut p = PathData::default();
    p.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
    p.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
    p.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
    p.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
    p.segments.push(PathSegment::Close);
    let rect = list.paths.push_anon(p);
    list.commands.push(Cmd::FillPath {
        path_id: rect,
        paint: Paint::Solid(Color::rgba(0.2, 0.6, 0.2, 1.0)),
        transform: XF([20.0, 0.0, 0.0, 20.0, 10.0, 10.0]),
    });
    list
}

#[test]
fn an_image_still_paints_after_a_frame_without_images() {
    let mut opts = RasterOptions::new(40.0, 40.0);
    opts.dpi = 72.0;
    opts.background = Color::rgba(1.0, 1.0, 1.0, 1.0);
    // ONE rasterizer, so every frame goes through the same Vello
    // renderer and its persistent image atlas — as the editor's
    // surface presenter does.
    let v = VelloRasterizer::new();
    let images = page_with_image();

    let first = v.rasterize(&images, &opts);
    if first.iter().all(|b| *b == 0) {
        eprintln!("no GPU available; skipping");
        return;
    }
    let (w, _) = opts.pixel_size();
    let red = px(&first, w, 20, 20);
    assert!(
        red[0] > 150 && red[1] < 90,
        "the image paints on the first frame: {red:?}"
    );

    let solid = v.rasterize(&page_of_solid_fills(), &opts);
    let green = px(&solid, w, 20, 20);
    assert!(green[1] > green[0], "the solid page paints: {green:?}");

    let again = v.rasterize(&images, &opts);
    assert_eq!(
        px(&again, w, 20, 20),
        red,
        "the image paints again after an image-free frame"
    );
}
