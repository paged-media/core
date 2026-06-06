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

//! Emit-cache correctness across undo/redo and page-structure shifts —
//! the engine side of the editor-suite findings AC-E2E-TEXT-5 (stale
//! text pixels after undo), PAGE-4 (insertPage mid-set panic), and
//! AC-E2E-STYLE-1 (style edit must repaint dependent text).
//!
//! The scene-hash determinism tests (determinism.rs) can't see these
//! bugs: the scene round-trips correctly while the *built* display
//! lists splice stale cached emissions. These tests assert on the
//! rendered output (`display_list_for_page`) instead.

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::channel::Mutation;
use paged_canvas::element_selection::ElementId;
use paged_canvas::{CanvasModel, CanvasOptions, PageId};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Minimal IDML: `spreads` are (self_id, spread_xml_body) pairs written
/// as Spreads/Spread_<id>.xml; one story `u10` whose paragraphs carry
/// real Inter glyphs so text edits change actual display commands.
fn idml(spreads: &[(&str, String)], story_xml: &str) -> Vec<u8> {
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();

    let mut designmap = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
"#,
    );
    for (id, _) in spreads {
        designmap.push_str(&format!(
            "  <idPkg:Spread src=\"Spreads/Spread_{id}.xml\"/>\n"
        ));
    }
    designmap.push_str("  <idPkg:Story src=\"Stories/Story_u10.xml\"/>\n</Document>");
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(designmap.as_bytes()).unwrap();

    for (id, body) in spreads {
        zip.start_file(format!("Spreads/Spread_{id}.xml"), deflated)
            .unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
{body}
</idPkg:Spread>"#
            )
            .as_bytes(),
        )
        .unwrap();
    }

    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story_xml.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

fn story(content_xml: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
{content_xml}
  </Story>
</idPkg:Story>"#
    )
}

fn load(spreads: &[(&str, String)], story_xml: &str) -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("doc", &idml(spreads, story_xml), opts).expect("load + build")
}

/// Semantic snapshot of a page's display list: paths (in id order),
/// commands, and resource pools — but NOT `PathBuffer`'s interning
/// memo, whose HashMap iteration order is nondeterministic and whose
/// keys depend on build history rather than rendered output.
fn page_dl(model: &CanvasModel, page: &str) -> String {
    let dl = model
        .display_list_for_page(&PageId(page.into()))
        .unwrap_or_else(|| panic!("page {page} has a display list"));
    let paths: Vec<_> = (0..dl.paths.len() as u32)
        .map(|i| {
            dl.paths
                .get(paged_compose::PathId(i))
                .expect("dense path ids")
        })
        .collect();
    format!(
        "{paths:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        dl.commands, dl.gradients, dl.radial_gradients, dl.images, dl.spot_inks
    )
}

fn single_page_with_story() -> CanvasModel {
    let spread = (
        "s1",
        r#"  <Spread Self="s1">
    <Page Self="p1" GeometricBounds="0 0 800 612"/>
    <TextFrame Self="tf1" ParentStory="u10" GeometricBounds="40 40 760 572" StrokeWeight="0"/>
  </Spread>"#
            .to_string(),
    );
    let story_xml = story(
        r#"    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>The quick brown fox jumps over the lazy dog.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>"#,
    );
    load(&[spread], &story_xml)
}

/// Two single-page spreads; the story's frame lives on the SECOND page
/// so a mid-set page insert shifts its absolute page index.
fn two_pages_story_on_second() -> CanvasModel {
    let s1 = (
        "s1",
        r#"  <Spread Self="s1">
    <Page Self="p1" GeometricBounds="0 0 800 612"/>
    <Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
  </Spread>"#
            .to_string(),
    );
    let s2 = (
        "s2",
        r#"  <Spread Self="s2" ItemTransform="1 0 0 1 0 900">
    <Page Self="p2" GeometricBounds="0 0 800 612"/>
    <TextFrame Self="tf2" ParentStory="u10" GeometricBounds="40 40 760 572" StrokeWeight="0"/>
  </Spread>"#
            .to_string(),
    );
    let story_xml = story(
        r#"    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Body story that must keep rendering on its own page.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>"#,
    );
    load(&[s1, s2], &story_xml)
}

/// AC-E2E-TEXT-5 — undo of a text edit must repaint: the body-story
/// emit cache matches on content-only edits (the signature hashes the
/// chain, not the content), so an undo that doesn't clear it splices
/// the stale post-edit emission back into the page.
#[test]
fn text_undo_restores_the_display_list() {
    let mut model = single_page_with_story();
    let baseline = page_dl(&model, "p1");

    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 4,
            text: "INSERTED ".into(),
        })
        .expect("insert text");
    let edited = page_dl(&model, "p1");
    assert_ne!(
        baseline, edited,
        "the text edit must change the page's display list"
    );

    model.undo().expect("undo");
    assert_eq!(
        page_dl(&model, "p1"),
        baseline,
        "undo must repaint the pre-edit text (stale emit-cache splice)"
    );

    model.redo().expect("redo");
    assert_eq!(
        page_dl(&model, "p1"),
        edited,
        "redo must repaint the post-edit text"
    );
}

/// PAGE-4 — inserting a page in the MIDDLE shifts the trailing story's
/// absolute page index. Before the fix the body-story cache survived
/// undo with stale per-page indices: undoing the insert panicked at
/// the splice (`index out of bounds: the len is N but the index is N`,
/// pipeline/mod.rs) and mutate() never resolved.
#[test]
fn insert_page_middle_undo_redo_round_trips_built_pages() {
    let mut model = two_pages_story_on_second();
    assert_eq!(model.page_count(), 2);
    let baseline_p1 = page_dl(&model, "p1");
    let baseline_p2 = page_dl(&model, "p2");

    model
        .apply_mutation(&Mutation::InsertPage {
            after_page_id: Some(PageId("p1".into())),
            master_id: None,
        })
        .expect("insert page mid-set");
    assert_eq!(model.page_count(), 3, "page inserted between p1 and p2");

    model
        .undo()
        .expect("undo of the mid-set insert must not panic");
    assert_eq!(model.page_count(), 2);
    assert_eq!(
        page_dl(&model, "p1"),
        baseline_p1,
        "page 1 repaints to baseline"
    );
    assert_eq!(
        page_dl(&model, "p2"),
        baseline_p2,
        "page 2 repaints to baseline"
    );

    model.redo().expect("redo");
    assert_eq!(model.page_count(), 3);
}

/// Forward direction of the same family: after a mid-set insert the
/// story must render on its page at the NEW index — a stale cache hit
/// would splice its commands into the wrong page silently.
#[test]
fn insert_page_middle_keeps_story_on_its_page() {
    let mut model = two_pages_story_on_second();
    let baseline_p2 = page_dl(&model, "p2");

    model
        .apply_mutation(&Mutation::InsertPage {
            after_page_id: Some(PageId("p1".into())),
            master_id: None,
        })
        .expect("insert page mid-set");

    // The story's page kept its content (now at page index 2)...
    assert_eq!(
        page_dl(&model, "p2"),
        baseline_p2,
        "the story still renders on its own page after the shift"
    );
    // ...and another text edit + undo on the shifted indices stays sound.
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "X".into(),
        })
        .expect("text edit after the shift");
    model.undo().expect("undo after the shift");
    assert_eq!(page_dl(&model, "p2"), baseline_p2);
}

/// AC-E2E-STYLE-1 — editing an in-use paragraph style's
/// characterFontSize must reach the rendered document. The text below
/// carries NO direct point size; it cascades from the applied
/// paragraph style, so the style edit must relayout + repaint.
#[test]
fn set_style_property_repaints_styled_text() {
    let spread = (
        "s1",
        r#"  <Spread Self="s1">
    <Page Self="p1" GeometricBounds="0 0 800 612"/>
    <TextFrame Self="tf1" ParentStory="u10" GeometricBounds="40 40 760 572" StrokeWeight="0"/>
  </Spread>"#
            .to_string(),
    );
    let story_xml = story(
        r#"    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/body">
      <CharacterStyleRange AppliedFont="Inter">
        <Content>Styled purely by the paragraph style.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>"#,
    );
    let mut model = load(&[spread], &story_xml);

    // Declare the referenced style via the same wire path the editor
    // uses, then size it — the cascade resolves live from doc.styles.
    model
        .apply_mutation(&Mutation::CreateParagraphStyle {
            self_id: Some("ParagraphStyle/body".into()),
            name: Some("Body".into()),
            based_on: None,
        })
        .expect("create style");
    let baseline = page_dl(&model, "p1");

    model
        .apply_mutation(&Mutation::SetStyleProperty {
            collection: paged_mutate::StyleCollection::Paragraph,
            style_id: "ParagraphStyle/body".into(),
            path: paged_mutate::PropertyPath::CharacterFontSize,
            value: paged_mutate::Value::Length(Some(36.0)),
        })
        .expect("set style point size");
    let resized = page_dl(&model, "p1");
    assert_ne!(
        baseline, resized,
        "style→text cascade must reach the rendered document (repaint)"
    );

    model.undo().expect("undo style edit");
    assert_eq!(
        page_dl(&model, "p1"),
        baseline,
        "undo repaints the original size"
    );
}

/// AC-E2E-CHAR-skew-undo — the per-paragraph layout cache key
/// (`paged-text/src/cache.rs::layout_runs_key`) must include every
/// `StyledRun` field that reaches the laid-out glyphs. `skew_deg` was
/// threaded through `StyledRun` (render-honor batch) AFTER the key was
/// written, so two runs that differ ONLY in skew hashed to the same key.
/// On a SetProperty(CharacterSkew) → undo, the model restores correctly
/// but the cache returns the SKEWED `LaidOutParagraph` for the unchanged
/// text, splicing a stale skew into the repainted page (a ~58px residual
/// the editor op-sandwich byte-identity check caught).
///
/// Each property below is a recently-threaded `StyledRun` field that
/// affects per-glyph output; the loop applies it as a text-range
/// SetProperty, asserts the forward paint changed, then asserts undo
/// repaints byte-identically to the baseline. Because the layout cache
/// is persistent across rebuilds, an unkeyed field also poisons the
/// FORWARD render (the cache returns the stale pre-edit layout), so the
/// `assert_ne!` below is itself a guard against the collision.
#[test]
fn character_property_undo_restores_display_list_no_stale_cache() {
    use paged_mutate::{PropertyPath, Value};

    // (label, path, forward value) — Length-valued character properties
    // whose StyledRun field must be folded into the layout cache key.
    let cases: &[(&str, PropertyPath, Value)] = &[
        // The prime suspect: skew was unkeyed.
        (
            "skew",
            PropertyPath::CharacterSkew,
            Value::Length(Some(15.0)),
        ),
        // Siblings threaded in the same render-honor batch — audit that
        // they are keyed too (vertical scale + horizontal scale).
        (
            "vertical_scale",
            PropertyPath::CharacterVerticalScale,
            Value::Length(Some(160.0)),
        ),
        (
            "horizontal_scale",
            PropertyPath::CharacterHorizontalScale,
            Value::Length(Some(160.0)),
        ),
        (
            "baseline_shift",
            PropertyPath::CharacterBaselineShift,
            Value::Length(Some(4.0)),
        ),
    ];

    for (label, path, value) in cases {
        let mut model = single_page_with_story();
        let baseline = page_dl(&model, "p1");

        // Cover the whole single-paragraph story (end past the content
        // is clamped per-run) — the editor's "apply to text selection"
        // path.
        let range = ElementId::StoryRange {
            story_id: "u10".into(),
            start: 0,
            end: 1000,
        };
        model
            .apply_mutation(&Mutation::SetElementProperty {
                element_id: range,
                path: *path,
                value: value.clone(),
            })
            .unwrap_or_else(|e| panic!("apply {label}: {e:?}"));
        let edited = page_dl(&model, "p1");
        assert_ne!(
            baseline, edited,
            "{label}: forward paint must change the page's display list",
        );

        model
            .undo()
            .unwrap_or_else(|| panic!("undo {label}: nothing to undo"));
        assert_eq!(
            page_dl(&model, "p1"),
            baseline,
            "{label}: undo must repaint the pre-edit layout — a stale cache \
             hit (field missing from the layout-cache key) leaves the {label} \
             baked into the unchanged text",
        );
    }
}
