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

//! Real-TTF coverage for the typography features the plan calls out
//! as "glyph count = 0" today: per-run font switching, story
//! threading, underline, vertical justify, bullet lists.
//!
//! Each test builds a synthetic IDML, registers real font bytes from
//! `corpus/fonts/`, runs the full pipeline, and asserts on properties
//! that only hold when shaping → outlining → rasterisation actually
//! ran (glyph counts, frame-chain ink distribution, underline ink,
//! VJ band offsets, bullet character coverage).

use std::io::Write;
use std::path::PathBuf;

use paged_compose::Color;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

fn write_zip<F: FnOnce(&mut ZipWriter<std::io::Cursor<Vec<u8>>>)>(f: F) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    f(&mut zip);
    zip.finish().unwrap().into_inner()
}

fn put(zip: &mut ZipWriter<std::io::Cursor<Vec<u8>>>, path: &str, body: &[u8]) {
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(path, deflated).unwrap();
    zip.write_all(body).unwrap();
}

fn count_dark_pixels(img: &image::RgbaImage, threshold: u8) -> usize {
    img.pixels()
        .filter(|p| p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold)
        .count()
}

fn count_dark_in_band(img: &image::RgbaImage, y0: u32, y1: u32, threshold: u8) -> usize {
    let mut n = 0usize;
    for y in y0..y1.min(img.height()) {
        for x in 0..img.width() {
            let p = img.get_pixel(x, y);
            if p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold {
                n += 1;
            }
        }
    }
    n
}

// ─────────────────────────── per-run fonts ──────────────────────────────

fn build_per_run_idml() -> Vec<u8> {
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(
            zip,
            "Spreads/Spread_sp1.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        );
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36">
        <Content>Sans</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Lora" PointSize="36">
        <Content> Serif</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

#[test]
fn per_run_font_switch_uses_both_fonts() {
    let document = idml_import::import_idml_doc(&build_per_run_idml()).unwrap();

    // Variant A: Inter for run 1, Lora for run 2.
    let mut a = BytesResolver::new();
    a.add_font("Inter", None, read_font("Inter.ttf"));
    a.add_font("Lora", None, read_font("Lora.ttf"));
    let opts_a = PipelineOptions {
        assets: Some(&a),
        ..PipelineOptions::default()
    };
    let (built_a, imgs_a) =
        pipeline::render_document(&document, &opts_a, 144.0, Color::WHITE).unwrap();

    // Variant B: Inter for run 1, RobotoSlab for run 2 — same first
    // run, different second run. If the second run is genuinely
    // shaped against its tagged font, the rendered pages must
    // diverge in the second-run region.
    let mut b = BytesResolver::new();
    b.add_font("Inter", None, read_font("Inter.ttf"));
    b.add_font("Lora", None, read_font("RobotoSlab.ttf"));
    let opts_b = PipelineOptions {
        assets: Some(&b),
        ..PipelineOptions::default()
    };
    let (_built_b, imgs_b) =
        pipeline::render_document(&document, &opts_b, 144.0, Color::WHITE).unwrap();

    // Both runs shape end-to-end.
    assert!(
        built_a.stats.glyphs >= 8,
        "expected per-run shaping to produce both runs' glyphs, got {}",
        built_a.stats.glyphs,
    );

    // The two renders must differ — proves Lora and RobotoSlab
    // bytes both reached the rasterizer for run 2.
    assert_ne!(
        imgs_a[0].as_raw(),
        imgs_b[0].as_raw(),
        "second-run font swap (Lora vs RobotoSlab) produced identical bytes — per-run font is not actually used",
    );
}

// ─────────────────────────── threading ──────────────────────────────────

fn build_threaded_text_idml() -> Vec<u8> {
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        // Two narrow short frames side by side. Frame A's height
        // (60 pt → ~3 lines at 14 pt) cannot hold the whole story,
        // forcing overflow into frame B.
        put(
            zip,
            "Spreads/Spread_sp1.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 600"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="40 40 100 280"
               NextTextFrame="frameB" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u10"
               GeometricBounds="40 320 360 560" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        );
        // Long enough to overflow frameA and spill into frameB.
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>This sentence is intentionally long so that the text composer must break it across many lines. After enough lines accumulate, the first frame fills up and the remainder spills into the second, threaded frame to its right. Every visible glyph proves both frames received composed text.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

#[test]
fn threaded_story_inks_both_frames() {
    let bytes = build_threaded_text_idml();
    let document = idml_import::import_idml_doc(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let (built, images) = pipeline::render_document(&document, &opts, 144.0, Color::WHITE).unwrap();
    assert!(built.stats.glyphs > 50, "story should shape many glyphs");
    assert!(
        built.stats.lines >= 4,
        "story should compose multiple lines"
    );

    let img = &images[0];
    // Frame A occupies x = 40..280 pt (= 80..560 px at 144 dpi).
    // Frame B occupies x = 320..560 pt (= 640..1120 px).
    let left = count_dark_in_band_x(img, 80, 560, 80);
    let right = count_dark_in_band_x(img, 640, 1120, 80);
    assert!(
        left > 200,
        "frame A should carry text ink, got {left} dark px"
    );
    assert!(
        right > 200,
        "frame B should carry overflow ink, got {right} dark px"
    );
}

fn count_dark_in_band_x(img: &image::RgbaImage, x0: u32, x1: u32, threshold: u8) -> usize {
    let mut n = 0usize;
    for y in 0..img.height() {
        for x in x0..x1.min(img.width()) {
            let p = img.get_pixel(x, y);
            if p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold {
                n += 1;
            }
        }
    }
    n
}

// ─────────────────────────── underline ──────────────────────────────────

fn build_underline_idml(underline: bool) -> Vec<u8> {
    let attr = if underline {
        r#" Underline="true""#
    } else {
        ""
    };
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36"{attr}>
        <Content>Underline</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(
            zip,
            "Spreads/Spread_sp1.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        );
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

#[test]
fn underline_adds_ink_relative_to_plain_run() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let plain_doc = idml_import::import_idml_doc(&build_underline_idml(false)).unwrap();
    let underlined_doc = idml_import::import_idml_doc(&build_underline_idml(true)).unwrap();

    let (_, plain_imgs) =
        pipeline::render_document(&plain_doc, &opts, 144.0, Color::WHITE).unwrap();
    let (_, under_imgs) =
        pipeline::render_document(&underlined_doc, &opts, 144.0, Color::WHITE).unwrap();

    let plain_ink = count_dark_pixels(&plain_imgs[0], 80);
    let under_ink = count_dark_pixels(&under_imgs[0], 80);
    assert!(
        under_ink > plain_ink + 200,
        "underline must stroke more ink than the bare glyphs: under={under_ink}, plain={plain_ink}",
    );
}

// ─────────────────────────── vertical justify ───────────────────────────

fn build_vj_idml(vj: &str) -> Vec<u8> {
    let frame = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 360 572" StrokeWeight="0">
      <TextFramePreference VerticalJustification="{vj}"/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
    );
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(zip, "Spreads/Spread_sp1.xml", frame.as_bytes());
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>VJ</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

/// Find the centroid Y of the inked pixels in `img`. Used as a
/// rough "where did the text land vertically" probe — robust to any
/// specific glyph metric assumption.
fn ink_centroid_y(img: &image::RgbaImage, threshold: u8) -> Option<f32> {
    let mut sum: u64 = 0;
    let mut count: u64 = 0;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let p = img.get_pixel(x, y);
            if p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold {
                sum += y as u64;
                count += 1;
            }
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum as f32 / count as f32)
    }
}

#[test]
fn vertical_justify_top_center_bottom_lands_in_distinct_bands() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let render = |vj: &str| -> f32 {
        let bytes = build_vj_idml(vj);
        let doc = idml_import::import_idml_doc(&bytes).unwrap();
        let (_, imgs) = pipeline::render_document(&doc, &opts, 144.0, Color::WHITE).unwrap();
        ink_centroid_y(&imgs[0], 80).expect("VJ render must produce ink")
    };

    let top = render("TopAlign");
    let center = render("CenterAlign");
    let bottom = render("BottomAlign");

    // Frame is 320 pt tall = 640 px at 144 dpi, sitting at y = 40 pt = 80 px.
    // Strict ordering: Top < Center < Bottom by a wide margin.
    assert!(
        top + 100.0 < center,
        "expected center centroid >> top, got top={top}, center={center}",
    );
    assert!(
        center + 100.0 < bottom,
        "expected bottom centroid >> center, got center={center}, bottom={bottom}",
    );
}

/// Find the topmost / bottommost inked rows. Used to bracket the
/// vertical extent of inked content rather than the centroid.
fn ink_extent_y(img: &image::RgbaImage, threshold: u8) -> Option<(u32, u32)> {
    let mut top = None;
    let mut bot = None;
    for y in 0..img.height() {
        let mut row_inked = false;
        for x in 0..img.width() {
            let p = img.get_pixel(x, y);
            if p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold {
                row_inked = true;
                break;
            }
        }
        if row_inked {
            if top.is_none() {
                top = Some(y);
            }
            bot = Some(y);
        }
    }
    top.zip(bot)
}

fn build_vj_three_paragraph_idml(vj: &str) -> Vec<u8> {
    let frame = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 560 572" StrokeWeight="0">
      <TextFramePreference VerticalJustification="{vj}"/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
    );
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(zip, "Spreads/Spread_sp1.xml", frame.as_bytes());
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Alpha</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Beta</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Gamma</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

#[test]
fn vertical_justify_distribute_spans_top_to_bottom() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let render = |vj: &str| -> (u32, u32) {
        let bytes = build_vj_three_paragraph_idml(vj);
        let doc = idml_import::import_idml_doc(&bytes).unwrap();
        let (_, imgs) = pipeline::render_document(&doc, &opts, 144.0, Color::WHITE).unwrap();
        ink_extent_y(&imgs[0], 80).expect("VJ render must produce ink")
    };

    let (top_first, top_last) = render("TopAlign");
    let (justify_first, justify_last) = render("JustifyAlign");
    let (_bottom_first, bottom_last) = render("BottomAlign");

    // Justify must keep the first paragraph at the top (same row band
    // as TopAlign) and push the last paragraph to the bottom (same
    // row band as BottomAlign). Allow a small tolerance for AA / row
    // rounding.
    let tol = 6i32;
    assert!(
        (justify_first as i32 - top_first as i32).abs() <= tol,
        "Justify first paragraph should track Top: justify_first={justify_first}, top_first={top_first}",
    );
    assert!(
        (justify_last as i32 - bottom_last as i32).abs() <= tol,
        "Justify last paragraph should track Bottom: justify_last={justify_last}, bottom_last={bottom_last}",
    );
    // And Justify must span more rows than TopAlign — the slack is
    // distributed, not parked at the top.
    assert!(
        (justify_last - justify_first) > (top_last - top_first) + 100,
        "Justify spread should exceed Top spread: justify=({justify_first}..{justify_last}), top=({top_first}..{top_last})",
    );
}

#[test]
fn vertical_justify_distribute_single_paragraph_matches_top() {
    // Single-paragraph stories have no inter-paragraph gap to grow,
    // so JustifyAlign must behave identically to TopAlign.
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let render = |vj: &str| -> f32 {
        let bytes = build_vj_idml(vj);
        let doc = idml_import::import_idml_doc(&bytes).unwrap();
        let (_, imgs) = pipeline::render_document(&doc, &opts, 144.0, Color::WHITE).unwrap();
        ink_centroid_y(&imgs[0], 80).expect("VJ render must produce ink")
    };

    let top = render("TopAlign");
    let justify = render("JustifyAlign");
    assert!(
        (top - justify).abs() < 1.0,
        "JustifyAlign with a single paragraph must match TopAlign: top={top}, justify={justify}",
    );
}

fn build_vj_overflow_idml(vj: &str) -> Vec<u8> {
    // Frame is intentionally tiny (40 pt tall) — the three 24-pt
    // paragraphs cannot fit, so JustifyAlign has no slack to
    // distribute and must fall back to TopAlign.
    let frame = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 80 572" StrokeWeight="0">
      <TextFramePreference VerticalJustification="{vj}"/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
    );
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(zip, "Spreads/Spread_sp1.xml", frame.as_bytes());
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Alpha</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Beta</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Gamma</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

#[test]
fn vertical_justify_distribute_overflow_matches_top() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let render = |vj: &str| -> f32 {
        let bytes = build_vj_overflow_idml(vj);
        let doc = idml_import::import_idml_doc(&bytes).unwrap();
        let (_, imgs) = pipeline::render_document(&doc, &opts, 144.0, Color::WHITE).unwrap();
        ink_centroid_y(&imgs[0], 80).expect("VJ render must produce ink")
    };

    let top = render("TopAlign");
    let justify = render("JustifyAlign");
    assert!(
        (top - justify).abs() < 1.0,
        "JustifyAlign with overflow must match TopAlign: top={top}, justify={justify}",
    );
}

fn build_vj_threaded_idml(vj: &str) -> Vec<u8> {
    // Two threaded frames side by side on one page. Frame A is short
    // (~80 pt tall) so it can hold only two of the four 24-pt
    // paragraphs; the remainder spills into the taller frame B.
    // After threading, each frame ends up with two paragraphs and
    // its own positive slack to distribute.
    let frame = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 612"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="40 40 100 280"
               NextTextFrame="frameB" StrokeWeight="0">
      <TextFramePreference VerticalJustification="{vj}"/>
    </TextFrame>
    <TextFrame Self="frameB" ParentStory="u10"
               GeometricBounds="40 320 280 560" StrokeWeight="0">
      <TextFramePreference VerticalJustification="{vj}"/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
    );
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(zip, "Spreads/Spread_sp1.xml", frame.as_bytes());
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Alpha</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Beta</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Gamma</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Delta</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

fn ink_extent_y_in_band_x(
    img: &image::RgbaImage,
    x0: u32,
    x1: u32,
    threshold: u8,
) -> Option<(u32, u32)> {
    let x1 = x1.min(img.width());
    let mut top: Option<u32> = None;
    let mut bot: Option<u32> = None;
    for y in 0..img.height() {
        let mut row_inked = false;
        for x in x0..x1 {
            let p = img.get_pixel(x, y);
            if p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold {
                row_inked = true;
                break;
            }
        }
        if row_inked {
            if top.is_none() {
                top = Some(y);
            }
            bot = Some(y);
        }
    }
    top.zip(bot)
}

#[test]
fn vertical_justify_distribute_threaded_per_frame() {
    // Threaded frames each get an independent distribute pass: their
    // paragraphs spread top-to-bottom within their own frame, not
    // across the chain as a whole. The probe: under JustifyAlign,
    // every frame's last paragraph must drop toward its own frame's
    // bottom (= TopAlign's bottom + that frame's local slack), while
    // the first paragraph in each frame stays anchored at the top.
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    // Frame A occupies x = 40..280 pt = 80..560 px at 144 dpi.
    // Frame B occupies x = 320..560 pt = 640..1120 px at 144 dpi.
    let render = |vj: &str| -> ((u32, u32), (u32, u32)) {
        let bytes = build_vj_threaded_idml(vj);
        let doc = idml_import::import_idml_doc(&bytes).unwrap();
        let (_, imgs) = pipeline::render_document(&doc, &opts, 144.0, Color::WHITE).unwrap();
        let a = ink_extent_y_in_band_x(&imgs[0], 80, 560, 80).expect("frame A must carry ink");
        let b = ink_extent_y_in_band_x(&imgs[0], 640, 1120, 80)
            .expect("frame B must carry overflow ink");
        (a, b)
    };

    let ((top_a_top, top_a_bot), (top_b_top, top_b_bot)) = render("TopAlign");
    let ((just_a_top, just_a_bot), (just_b_top, just_b_bot)) = render("JustifyAlign");

    // The first paragraph in each frame must stay near the top of
    // that frame (matches TopAlign within a small tolerance) —
    // distribute only grows the inter-paragraph gaps, it doesn't
    // shift the head baseline.
    let tol = 6i32;
    assert!(
        (just_a_top as i32 - top_a_top as i32).abs() <= tol,
        "frame A: first paragraph should stay at top: top={top_a_top}, just={just_a_top}",
    );
    assert!(
        (just_b_top as i32 - top_b_top as i32).abs() <= tol,
        "frame B: first paragraph should stay at top: top={top_b_top}, just={just_b_top}",
    );
    // Per-frame distribute must push the last paragraph of EACH
    // frame strictly below where TopAlign left it. Without per-frame
    // bookkeeping (e.g. if distribute treated the chain as a single
    // span), one frame would receive all the slack and the other
    // would still look like TopAlign.
    assert!(
        just_a_bot > top_a_bot,
        "frame A: distribute should push last paragraph down: top_bot={top_a_bot}, just_bot={just_a_bot}",
    );
    assert!(
        just_b_bot > top_b_bot,
        "frame B: distribute should push last paragraph down: top_bot={top_b_bot}, just_bot={just_b_bot}",
    );
    // Frame B has much more slack than frame A (it's a much taller
    // frame holding the same number of paragraphs), so the drop in
    // frame B must dwarf the drop in frame A — that's the signature
    // of an independent per-frame pass.
    let drop_a = just_a_bot.saturating_sub(top_a_bot);
    let drop_b = just_b_bot.saturating_sub(top_b_bot);
    assert!(
        drop_b > drop_a + 50,
        "frame B's local slack should produce a much bigger drop than frame A's: drop_a={drop_a} px, drop_b={drop_b} px",
    );
}

// ─────────────────────────── bullet list ────────────────────────────────

fn build_bullet_idml() -> Vec<u8> {
    write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(
            zip,
            "Resources/Styles.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Bulleted"
                    Name="Bulleted"
                    BulletsAndNumberingListType="BulletList"
                    BulletsTextAfter=" ">
      <Properties>
        <BulletChar BulletCharacterValue="8226"/>
      </Properties>
    </ParagraphStyle>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#,
        );
        put(
            zip,
            "Spreads/Spread_sp1.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        );
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Bulleted">
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Item</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

#[test]
fn bulleted_paragraph_emits_extra_glyphs_and_ink() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    // Plain "Item" — no list applied.
    let plain_bytes = write_zip(|zip| {
        put(
            zip,
            "designmap.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        );
        put(
            zip,
            "Resources/Graphic.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        );
        put(
            zip,
            "Spreads/Spread_sp1.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        );
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Item</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    });
    let bullet_bytes = build_bullet_idml();

    let plain_doc = idml_import::import_idml_doc(&plain_bytes).unwrap();
    let bullet_doc = idml_import::import_idml_doc(&bullet_bytes).unwrap();

    let (plain_built, plain_imgs) =
        pipeline::render_document(&plain_doc, &opts, 144.0, Color::WHITE).unwrap();
    let (bullet_built, bullet_imgs) =
        pipeline::render_document(&bullet_doc, &opts, 144.0, Color::WHITE).unwrap();

    // The bulleted paragraph prepends "•<space>" to the run, so the
    // shaped glyph count must rise by ≥ 1 (the bullet glyph; the
    // space typically shapes to a separate cluster but won't ink).
    assert!(
        bullet_built.stats.glyphs > plain_built.stats.glyphs,
        "bulleted paragraph should add ≥1 glyph; plain={}, bullet={}",
        plain_built.stats.glyphs,
        bullet_built.stats.glyphs,
    );

    // The bullet glyph + separator means the rendered pixel
    // payload must differ from the plain "Item" render. Equality
    // would mean the bullet never reached the rasterizer.
    assert_ne!(
        plain_imgs[0].as_raw(),
        bullet_imgs[0].as_raw(),
        "plain vs bulleted paragraph rendered identically — bullet glyph not in output",
    );
    // And the bullet must add ink overall (it's "•" + " " + "Item"
    // vs plain "Item", so the inked-pixel total grows).
    let plain_ink = count_dark_pixels(&plain_imgs[0], 80);
    let bullet_ink = count_dark_pixels(&bullet_imgs[0], 80);
    assert!(
        bullet_ink > plain_ink,
        "bullet should add ink overall; plain={plain_ink}, bullet={bullet_ink}",
    );
}

// ─────────────────────────── helpers used above ─────────────────────────

#[allow(dead_code)]
fn _placate_unused(img: &image::RgbaImage) {
    let _ = count_dark_in_band(img, 0, 1, 0);
}
