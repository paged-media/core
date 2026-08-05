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

//! A transparency group's `bounds` must cover what the group PAINTS,
//! not just the frame's geometry.
//!
//! The shape under test is `corpus/generated/transparency` page 12
//! (`combo · shadow+opacity`) in miniature: a 50 %-opacity rectangle
//! with a drop shadow offset 8 pt down-right and blurred by 6 pt. The
//! opacity opens a transparency group; the shadow paints
//! `8 + 3σ + 1 = 27 pt` past the frame's right and bottom edges. Both
//! rasterizers clip a group's contents to its `bounds`, so before
//! `fit_transparency_group_bounds` the shadow was cut dead at the frame
//! rect + 0.5 pt — while `paged-export-pdf` gave the same group's form
//! the media box and kept it. Raster and PDF disagreed about one
//! document.

use paged_renderer::pipeline::{self, PipelineOptions};

/// Frame geometry, in page pt. Page is 200 × 200.
const FRAME_X0: f32 = 40.0;
const FRAME_Y0: f32 = 40.0;
const FRAME_X1: f32 = 140.0;
const FRAME_Y1: f32 = 140.0;
/// `<DropShadowSetting XOffset="8" YOffset="8" Size="6">`.
const SHADOW_OFFSET: f32 = 8.0;
const SHADOW_BLUR: f32 = 6.0;

fn shadowed_translucent_rect_idml() -> Vec<u8> {
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="40 40 140 140" FillColor="Color/Black">
      <TransparencySetting>
        <BlendingSetting Opacity="50"/>
        <DropShadowSetting Mode="Drop" XOffset="8" YOffset="8" Size="6" Opacity="75"
                           EffectColor="Color/Black"/>
      </TransparencySetting>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn a_drop_shadow_fits_inside_its_own_transparency_group() {
    let bytes = shadowed_translucent_rect_idml();
    let doc = idml_import::import_idml_doc(&bytes).expect("open IDML");
    let built = pipeline::build_document(&doc, &PipelineOptions::default()).expect("build");
    let page = &built.pages[0];

    let groups: Vec<paged_compose::Rect> = page
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            paged_compose::DisplayCommand::BeginBlendGroup { bounds, .. } => Some(*bounds),
            _ => None,
        })
        .collect();
    assert_eq!(
        groups.len(),
        1,
        "the 50 % opacity should open exactly one transparency group"
    );
    let b = groups[0];

    // The rasterizer stamps the shadow at `path + offset` and pads its
    // scratch by `3σ + 1`; every pixel of that has to be inside the
    // group or it is cut with a hard edge.
    let reach = SHADOW_OFFSET + 3.0 * SHADOW_BLUR + 1.0;
    assert!(
        b.x + b.w >= FRAME_X1 + reach,
        "group must reach {} pt to the right of the frame; bounds {b:?}",
        reach
    );
    assert!(
        b.y + b.h >= FRAME_Y1 + reach,
        "group must reach {} pt below the frame; bounds {b:?}",
        reach
    );
    // The shadow moves down-RIGHT, so the leading edges only need the
    // blur tail, not the offset.
    let tail = 3.0 * SHADOW_BLUR + 1.0 - SHADOW_OFFSET;
    assert!(
        b.x <= FRAME_X0 - tail && b.y <= FRAME_Y0 - tail,
        "group must cover the shadow's leading blur tail; bounds {b:?}"
    );

    assert!(
        paged_compose::transparency_group_overflow(&page.list).is_none(),
        "no group on this page may paint outside its own bounds"
    );
}

/// The pixel-level half of the same claim: shadow ink must actually
/// land outside the frame rect. Asserting only on `bounds` would pass
/// if the group were sized correctly and the shadow still never drawn.
#[cfg(feature = "cpu")]
#[test]
fn the_shadow_paints_past_the_frame_rect() {
    let bytes = shadowed_translucent_rect_idml();
    let doc = idml_import::import_idml_doc(&bytes).expect("open IDML");
    let built = pipeline::build_document(&doc, &PipelineOptions::default()).expect("build");
    let page = &built.pages[0];

    let dpi = 144.0f32;
    let scale = dpi / 72.0;
    let mut opts = paged_gpu::RasterOptions::new(page.width_pt, page.height_pt);
    opts.dpi = dpi;
    opts.background = paged_compose::Color::rgba(1.0, 1.0, 1.0, 1.0);
    let img = paged_gpu::rasterize(&page.list, &opts);

    let at = |x_pt: f32, y_pt: f32| -> [u8; 4] {
        let x = (x_pt * scale) as u32;
        let y = (y_pt * scale) as u32;
        let p = img.get_pixel(x, y);
        [p[0], p[1], p[2], p[3]]
    };

    // Well past the frame's right edge but inside the offset shadow
    // rect (40+8 .. 140+8). Paper here means the shadow was clipped.
    let outside = at(145.0, 100.0);
    assert!(
        outside[0] < 240 && outside[1] < 240 && outside[2] < 240,
        "shadow ink must land past the frame's right edge, got {outside:?}"
    );
    // …and below it.
    let below = at(100.0, 145.0);
    assert!(
        below[0] < 240,
        "shadow ink must land below the frame, got {below:?}"
    );
    // Far outside everything is still paper — the group has not
    // smeared, it has only stopped cutting.
    let paper = at(190.0, 190.0);
    assert_eq!(
        paper,
        [255, 255, 255, 255],
        "the page outside the shadow must stay paper"
    );
}
