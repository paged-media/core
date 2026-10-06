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

//! C-69 (protocol 66) — a text frame's composed glyphs read back as
//! outlines in page space, one run per fill colour, and only the glyphs
//! that frame shows.

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::{element_selection::ElementId, CanvasModel, CanvasOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn read_inter() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Frame A (100,100)–(400,160) holds "Hi" in black then "Lo" in red;
/// frame B (100,500)–(400,560) holds a second story; and a rectangle.
fn idml() -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut put = |name: &str, body: &str| {
        zip.start_file(name, o).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put("mimetype", "application/vnd.adobe.indesign-idml-package");
    put(
        "designmap.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Graphic src="Resources/Graphic.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
  <idPkg:Story src="Stories/Story_u20.xml"/>
</Document>"#,
    );
    put(
        "Resources/Graphic.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Black" Model="Process" Space="CMYK" ColorValue="0 0 0 100" Name="Black"/>
  <Color Self="Color/Red" Model="Process" Space="RGB" ColorValue="255 0 0" Name="Red"/>
</idPkg:Graphic>"#,
    );
    put(
        "Spreads/Spread_sp1.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="100 100 160 400" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u20" GeometricBounds="500 100 560 400" StrokeWeight="0"/>
    <Rectangle Self="rect" GeometricBounds="300 100 340 140" FillColor="Color/Black"/>
  </Spread>
</idPkg:Spread>"#,
    );
    put(
        "Stories/Story_u10.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10"><ParagraphStyleRange>
    <CharacterStyleRange AppliedFont="Inter" PointSize="24" FillColor="Color/Black"><Content>Hi</Content></CharacterStyleRange>
    <CharacterStyleRange AppliedFont="Inter" PointSize="24" FillColor="Color/Red"><Content>Lo</Content></CharacterStyleRange>
  </ParagraphStyleRange></Story>
</idPkg:Story>"#,
    );
    put(
        "Stories/Story_u20.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u20"><ParagraphStyleRange>
    <CharacterStyleRange AppliedFont="Inter" PointSize="24" FillColor="Color/Black"><Content>Elsewhere</Content></CharacterStyleRange>
  </ParagraphStyleRange></Story>
</idPkg:Story>"#,
    );
    zip.finish().unwrap().into_inner()
}

fn model() -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![read_inter()],
        ..CanvasOptions::default()
    };
    CanvasModel::load("d1", &idml(), opts).expect("load")
}

#[test]
fn a_frame_reads_back_its_own_glyphs_as_outlines_one_run_per_colour() {
    let m = model();
    let r = m
        .text_outlines(&ElementId::TextFrame("frameA".into()))
        .expect("a text frame answers");
    assert_eq!(r.page_id.as_str(), "p1");
    assert_eq!(r.skipped_glyphs, 0);
    assert_eq!(
        r.runs.len(),
        2,
        "black and red: {:?}",
        r.runs.iter().map(|r| r.rgb).collect::<Vec<_>>()
    );
    let glyphs: u32 = r.runs.iter().map(|r| r.glyphs).sum();
    assert_eq!(glyphs, 4, "H i L o");
    let red = r
        .runs
        .iter()
        .find(|r| r.rgb[0] > 0.9 && r.rgb[1] < 0.1)
        .expect("a red run");
    assert_eq!(red.glyphs, 2);
    for run in &r.runs {
        assert!(!run.subpath_starts.is_empty());
        assert_eq!(run.subpath_starts[0], 0);
        assert!(run.subpath_starts.windows(2).all(|w| w[0] < w[1]));
        assert!((*run.subpath_starts.last().unwrap() as usize) < run.anchors.len());
        // Every anchor sits inside frame A, in PAGE space — not frame B's
        // band 400 pt lower, and not glyph-local units near the origin.
        for a in &run.anchors {
            for p in [a.anchor, a.left, a.right] {
                assert!(p[0] > 95.0 && p[0] < 405.0, "x {p:?}");
                assert!(p[1] > 95.0 && p[1] < 165.0, "y {p:?}");
            }
        }
    }
    // "o" is a ring: it carries two contours, and the curves came through
    // as curves (some handle differs from its anchor).
    assert!(red.subpath_starts.len() >= 3, "L + the two rings of o");
    assert!(red
        .anchors
        .iter()
        .any(|a| a.left != a.anchor || a.right != a.anchor));
}

#[test]
fn the_other_frame_answers_only_its_own_text_and_non_text_answers_none() {
    let m = model();
    let b = m
        .text_outlines(&ElementId::TextFrame("frameB".into()))
        .expect("frame B answers");
    let glyphs: u32 = b.runs.iter().map(|r| r.glyphs).sum();
    assert_eq!(glyphs, "Elsewhere".len() as u32);
    for run in &b.runs {
        for a in &run.anchors {
            assert!(a.anchor[1] > 495.0 && a.anchor[1] < 565.0);
        }
    }
    assert!(m
        .text_outlines(&ElementId::Rectangle("rect".into()))
        .is_none());
    assert!(m
        .text_outlines(&ElementId::TextFrame("nope".into()))
        .is_none());
}

#[test]
fn a_minted_frame_with_typed_text_reads_back_outlines() {
    use paged_canvas::{channel::Mutation, PageId};
    let idml = paged_canvas::blank::blank_idml(612.0, 792.0);
    let opts = CanvasOptions {
        fonts: vec![read_inter()],
        ..CanvasOptions::default()
    };
    let mut m = CanvasModel::load("doc", &idml, opts).expect("load");
    let page = m.scene().spreads[0].spread.pages[0]
        .self_id
        .clone()
        .expect("page id");
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId(page),
            bounds: (300.0, 40.0, 380.0, 400.0),
        })
        .expect("frame");
    let Some(ElementId::TextFrame(frame)) = out.created_id else {
        panic!("expected a text frame");
    };
    let story = m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame.as_str()))
        .and_then(|f| f.parent_story.clone())
        .expect("minted story");
    m.apply_mutation(&Mutation::InsertText {
        story_id: story,
        offset: 0,
        text: "Ho".into(),
        cell: None,
    })
    .expect("text");
    let r = m
        .text_outlines(&ElementId::TextFrame(frame))
        .expect("answers");
    let glyphs: u32 = r.runs.iter().map(|r| r.glyphs).sum();
    assert_eq!(glyphs, 2, "{r:?}");
}
