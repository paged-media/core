//! Phase 3 Item 2 — hit-test returns `offset_within_story`.
//!
//! Verifies that clicking on a text frame produces a story-local byte
//! offset by bisecting the StoryLayout's clusters.

use std::io::Write;
use std::path::PathBuf;

use idml_canvas::{CanvasModel, CanvasOptions, PageId};
use idml_renderer::BytesResolver;

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name))
        .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

fn build_idml(text: &str) -> Vec<u8> {
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
      <CharacterStyleRange AppliedFont="Inter" PointSize="36">
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

fn load_model(text: &str) -> CanvasModel {
    let bytes = build_idml(text);
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("doc", &bytes, opts).expect("load + build")
}

#[test]
fn click_in_text_frame_returns_offset() {
    let model = load_model("Hello world.");
    let page_id = PageId("p1".into());
    // Click at (x=100, y=70) — somewhere inside the line. The exact
    // offset depends on Inter's metrics at 36pt; we assert only that
    // it's a small non-None value within the line's byte range.
    let hit = model.hit_test(&page_id, (100.0, 70.0));
    assert_eq!(hit.frame_id.as_deref(), Some("frameA"));
    assert_eq!(hit.story_id.as_deref(), Some("u10"));
    assert!(hit.offset_within_story.is_some(), "should return an offset");
    let off = hit.offset_within_story.unwrap();
    // "Hello world." is 12 bytes; offsets 0..=12 are all valid line
    // positions (12 = byte_range.end for end-of-line clicks).
    assert!(off <= 12, "offset {off} should fall within story length 12");
}

#[test]
fn click_near_left_edge_snaps_to_start_of_line() {
    let model = load_model("Hello world.");
    let page_id = PageId("p1".into());
    // Frame is at x = 40..572 in page-local pt. Click just inside the
    // left edge — the story_offset path snaps to byte 0 (the leading
    // cluster is at text_origin ≈ x=40).
    let hit = model.hit_test(&page_id, (42.0, 70.0));
    assert_eq!(hit.frame_id.as_deref(), Some("frameA"));
    assert_eq!(hit.offset_within_story, Some(0));
}

#[test]
fn click_near_right_edge_snaps_to_end_of_line() {
    let model = load_model("Hi.");
    let page_id = PageId("p1".into());
    // Click just inside the right edge — past the last glyph, the
    // bisector lands at byte_range.end (= source length 3 for "Hi.").
    let hit = model.hit_test(&page_id, (570.0, 70.0));
    assert_eq!(hit.frame_id.as_deref(), Some("frameA"));
    assert_eq!(hit.offset_within_story, Some(3));
}

#[test]
fn click_outside_any_frame_returns_no_offset() {
    let model = load_model("Hello.");
    let page_id = PageId("p1".into());
    // Click in the page background, away from any frame.
    let hit = model.hit_test(&page_id, (5.0, 5.0));
    assert!(hit.frame_id.is_none());
    assert!(hit.offset_within_story.is_none());
}
