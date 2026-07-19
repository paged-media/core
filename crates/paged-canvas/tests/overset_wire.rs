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

//! Migration slice **S4** — the overset continuation crosses the wasm wire.
//!
//! `CanvasModel::stories()` (the `paged.stories()` surface) now carries the
//! first-class overset continuation (`StorySummary.overset_at`), not just the
//! boolean overset flag — so the editor can jump to the clipped text.

use std::path::PathBuf;

use paged_canvas::{channel::OversetAt, CanvasModel, CanvasOptions};
use std::io::Write;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn read_inter() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// One story of `text` in a single frame `frame_h` pt tall (chain length 1).
/// A short frame clips the trailing lines → overset; a tall one fits.
fn single_frame_idml(text: &str, frame_h: f32) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
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
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Graphic/></idPkg:Graphic>"#,
    )
    .unwrap();
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
      <CharacterStyleRange AppliedFont="Inter" PointSize="18"><Content>{text}</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

const LONG: &str = "The quick brown fox jumps over the lazy dog. \
Sphinx of black quartz, judge my vow. Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump. The five boxing wizards jump quickly.";

fn model(bytes: &[u8]) -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![read_inter()],
        ..CanvasOptions::default()
    };
    CanvasModel::load("overset-wire", bytes, opts).expect("load + build")
}

#[test]
fn stories_surface_the_overset_continuation() {
    let m = model(&single_frame_idml(LONG, 40.0));
    let story = m
        .stories()
        .into_iter()
        .find(|s| s.self_id == "u10")
        .expect("story u10");

    assert!(story.overset, "story should be flagged overset");
    let at = story.overset_at.expect("overset continuation surfaced");
    assert_eq!(at.paragraph, 0);
    assert!(
        at.line >= 1,
        "some lines fit before overset; got {}",
        at.line
    );
}

#[test]
fn a_fitting_story_has_no_continuation() {
    let m = model(&single_frame_idml(LONG, 800.0));
    let story = m
        .stories()
        .into_iter()
        .find(|s| s.self_id == "u10")
        .unwrap();
    assert!(!story.overset);
    assert_eq!(story.overset_at, None::<OversetAt>);
}
