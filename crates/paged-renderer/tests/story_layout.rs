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

//! Phase 3 Item A — StoryLayout capture verification.
//!
//! Asserts that the renderer captures per-cluster page-local
//! positions during paragraph emission, so the canvas can later
//! hit-test by character offset, place the caret, and compute
//! selection geometry.
//!
//! Uses real font shaping (Inter) so cluster positions reflect
//! actual glyph metrics, not stubs.

use std::io::Write;
use std::path::PathBuf;

use paged_renderer::{pipeline, BytesResolver, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name))
        .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Single-page IDML with one paragraph hosted in a TextFrame whose
/// `ParentStory="u10"`. Stories filename mirrors the Self id so the
/// scene's `derive_story_id` produces the correct lookup key.
fn build_single_paragraph_idml(text: &str, applied_font: &str, point_size: f32) -> Vec<u8> {
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
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 380 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="{applied_font}" PointSize="{point_size}">
        <Content>{text}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn single_line_story_captures_one_line_with_clusters() {
    let bytes = build_single_paragraph_idml("Hello, IDML world.", "Inter", 36.0);
    let document = Document::open(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).unwrap();

    let layout = built.story_layout("u10");
    assert!(
        !layout.is_empty(),
        "expected story_layout to capture at least one line; got empty"
    );

    // "Hello, IDML world." should fit on one line at 36pt in a
    // 532pt-wide frame. The captured line is paragraph 0, line 0.
    let first = layout[0];
    assert_eq!(first.story_id, "u10");
    assert_eq!(first.paragraph_idx, 0);
    assert_eq!(first.line_idx, 0);
    assert_eq!(first.byte_range.start, 0, "line starts at paragraph offset 0");

    // Clusters: 17 source chars (counting the period); allowing for
    // any ligature coalescing, we expect at least 12 distinct
    // clusters. Each one's x_pt should be monotonically increasing —
    // that's the property hit-testing bisects over.
    assert!(
        first.clusters.len() >= 12,
        "expected ≥ 12 clusters from \"Hello, IDML world.\", got {}",
        first.clusters.len()
    );
    for win in first.clusters.windows(2) {
        assert!(
            win[1].x_pt > win[0].x_pt,
            "cluster x_pt must increase: {:?} → {:?}",
            win[0],
            win[1]
        );
        assert!(
            win[1].byte > win[0].byte,
            "cluster byte must increase (no duplicates after coalesce): {:?} → {:?}",
            win[0],
            win[1]
        );
    }

    // Page-local baseline: frame at y=40, text origin at y≈40, baseline
    // at ~36pt × 0.8 ≈ 28pt below frame top. So baseline_y_pt should
    // be in the 60–90 pt range. Loose because line height has spec'd
    // slack we don't pin here.
    assert!(
        first.baseline_y_pt > 50.0 && first.baseline_y_pt < 120.0,
        "baseline_y_pt out of expected range for 36pt text in frame at y=40: {}",
        first.baseline_y_pt
    );
    assert!(first.ascent_pt > 0.0);
    assert!(first.descent_pt > 0.0);
}

#[test]
fn story_layout_is_empty_when_no_font_loaded() {
    // No font in the resolver → shaping skipped → glyphs vector
    // empty → no per-line capture. story_layout returns nothing.
    let bytes = build_single_paragraph_idml("Hello.", "Inter", 12.0);
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default(); // no assets
    let built = pipeline::build_document(&document, &opts).unwrap();
    assert!(
        built.story_layout("u10").is_empty(),
        "expected no capture without a font resolver"
    );
}

#[test]
fn story_layout_lookup_unknown_story_returns_empty() {
    let bytes = build_single_paragraph_idml("Hello.", "Inter", 12.0);
    let document = Document::open(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).unwrap();
    assert!(built.story_layout("nonexistent").is_empty());
}

#[test]
fn story_layout_frame_id_matches_self_attribute() {
    let bytes = build_single_paragraph_idml("Hi.", "Inter", 20.0);
    let document = Document::open(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).unwrap();
    let layout = built.story_layout("u10");
    assert!(!layout.is_empty());
    assert_eq!(
        layout[0].frame_id.as_deref(),
        Some("frameA"),
        "captured frame_id should match the TextFrame's Self attribute"
    );
}
