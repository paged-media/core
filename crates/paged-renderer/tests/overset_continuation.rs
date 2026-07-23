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

//! Migration slice **S3** — the live render path adopts the flow protocol's
//! **first-class overset**. `StoryEmitter` still clips overflow past the last
//! frame (rendering is byte-unchanged), but the `OversetTextDropped`
//! diagnostic now carries *where* the flow overran — the continuation cursor
//! (`paged_flow::Overset::Remains`), surfaced via
//! `RenderDiagnostics::overset_continuations()`.

use std::io::Write;
use std::path::PathBuf;

use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Single-frame IDML: one story of `text` in a frame `frame_h` pt tall. A
/// short frame clips the trailing lines (overset); a tall one fits everything.
fn build_single_frame_idml(text: &str, frame_h: f32) -> Vec<u8> {
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

    // A single frame (no NextTextFrame → chain length 1) so any overflow has
    // nowhere to go and clips = overset. GeometricBounds is `top left bottom
    // right`; height = bottom - top.
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 900 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="0 0 {frame_h} 400" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread.as_bytes()).unwrap();

    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="18">
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

fn build(bytes: &[u8]) -> pipeline::BuiltDocument {
    let document = idml_import::import_idml_doc(bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

const LONG: &str = "The quick brown fox jumps over the lazy dog. \
Sphinx of black quartz, judge my vow. Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump. The five boxing wizards jump quickly.";

#[test]
fn overset_records_a_first_class_continuation() {
    // A 40pt-tall frame holds only the first line or two of the 18pt story;
    // the rest overruns and is clipped (overset).
    let built = build(&build_single_frame_idml(LONG, 40.0));

    // The plain per-story flag still fires.
    assert!(
        built.diagnostics.overset_story_ids().contains("u10"),
        "story u10 should be reported overset"
    );

    // NEW: the continuation cursor is recorded — where the flow overran.
    let conts = built.diagnostics.overset_continuations();
    let cont = conts
        .get("u10")
        .expect("overset continuation recorded for u10");
    // Overset begins in the first (only) paragraph, past the line(s) that fit.
    assert_eq!(cont.paragraph_idx, 0);
    assert!(
        cont.line_idx >= 1,
        "at least the first line fit before overset; got line_idx {}",
        cont.line_idx
    );
}

#[test]
fn content_that_fits_has_no_overset_continuation() {
    // A generously tall frame fits the whole story — no overset at all.
    let built = build(&build_single_frame_idml(LONG, 800.0));
    assert!(
        built.diagnostics.overset_story_ids().is_empty(),
        "a fitting story must not report overset"
    );
    assert!(
        built.diagnostics.overset_continuations().is_empty(),
        "a fitting story must have no continuation"
    );
}
