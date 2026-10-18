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

//! `CanvasModel::idml_export_losses` is MEASURED: it exports the
//! document, re-parses the bytes with the load-path importer, and diffs
//! the twin against the scene. This test authors one of every construct
//! InDesign 2025 was measured to drop from an engine-authored book —
//! a section, a guide, a table in a minted story, a hyperlink, a
//! condition flip, an image placed from bytes — and pins what the loss
//! list says about each.
//!
//! The assertions first documented the measured truth of 2026-09-05
//! morning (every construct lost); each exporter lane that landed the
//! same day flipped its assertion to "the loss is gone". A line that
//! re-appears is a lane that regressed; the image-from-bytes line is the
//! one honest, permanent loss (IDML cannot embed pixels).

use std::io::{Cursor, Write};

use paged_canvas::{
    channel::{ByteBuf, Mutation},
    CanvasModel, CanvasOptions, ElementId, PageId,
};
use paged_mutate::operation::GuideOrientationSpec;

/// One spread / one story / one empty rectangle, plus a `Resources/Styles.xml`
/// that carries a `<Condition>` in the generator's invented
/// `<RootConditionalTextGroup>` wrapper (the spelling every engine-built
/// fixture ships today).
fn fixture() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let parts: [(&str, &str); 4] = [
            (
                "designmap.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<Layer Self="ub6" Name="Layer 1" Visible="true" Locked="false" Printable="true"/>
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
<idPkg:Story src="Stories/Story_story1.xml"/>
</Document>"#,
            ),
            (
                "Resources/Styles.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<RootCharacterStyleGroup Self="u9d"><CharacterStyle Self="CharacterStyle/$ID/[No character style]" Name="$ID/[No character style]"/></RootCharacterStyleGroup>
<RootParagraphStyleGroup Self="u9e"><ParagraphStyle Self="ParagraphStyle/$ID/[No paragraph style]" Name="$ID/[No paragraph style]"/></RootParagraphStyleGroup>
<RootConditionalTextGroup><Condition Self="Condition/Draft" Name="Draft" Visible="true" IndicatorMethod="UseHighlight"/></RootConditionalTextGroup>
</idPkg:Styles>"#,
            ),
            (
                "Spreads/Spread_s1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="450 100 600 300" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            ),
            (
                "Stories/Story_story1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
            ),
        ];
        for (name, body) in parts {
            zip.start_file(name, deflated).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn tiny_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn minted_story_of(model: &CanvasModel, frame: &str) -> String {
    model.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame))
        .and_then(|f| f.parent_story.clone())
        .expect("minted frame has a parent story")
}

/// Author one of each construct; return the model + the ids the test
/// needs to name in its assertions.
struct Authored {
    model: CanvasModel,
    minted_story: String,
    table_id: String,
    hyperlink_id: String,
}

fn author() -> Authored {
    let mut model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");

    // Section: folio "A-iii…" starting on page p1.
    model
        .apply_mutation(&Mutation::InsertSection {
            at_page: PageId("p1".into()),
            prefix: Some("A-".into()),
            numbering_style: Some("LowerRoman".into()),
            start_at: Some(3),
        })
        .expect("section");
    // Guide: vertical, 100pt, page 0.
    model
        .apply_mutation(&Mutation::InsertGuide {
            spread_id: "s1".into(),
            orientation: GuideOrientationSpec::Vertical,
            position: 100.0,
            page_index: 0,
        })
        .expect("guide");
    // A minted text frame (and so a minted story) carrying a 2×3 table.
    let out = model
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId("p1".into()),
            bounds: (420.0, 320.0, 560.0, 560.0),
        })
        .expect("frame");
    let frame = match out.created_id {
        Some(ElementId::TextFrame(id)) => id,
        other => panic!("expected a text frame, got {other:?}"),
    };
    let minted_story = minted_story_of(&model, &frame);
    let out = model
        .apply_mutation(&Mutation::InsertTable {
            story_id: minted_story.clone(),
            rows: 2,
            cols: 3,
            header_rows: 1,
            footer_rows: 0,
            column_widths: vec![60.0, 60.0, 60.0],
            row_heights: vec![20.0, 20.0],
        })
        .expect("table");
    let table_id = match out.created_id {
        Some(ElementId::Table { table_id, .. }) => table_id,
        other => panic!("expected a table, got {other:?}"),
    };
    // Hyperlink over "Hello" in the source story.
    model
        .apply_mutation(&Mutation::InsertHyperlink {
            story_id: "story1".into(),
            start: 0,
            end: 5,
            url: "https://paged.media".into(),
            page: None,
        })
        .expect("hyperlink");
    let hyperlink_id = model.scene().designmap.hyperlinks[0].self_id.clone();
    // Condition flipped off.
    model
        .apply_mutation(&Mutation::SetConditionVisible {
            condition: "Condition/Draft".into(),
            visible: false,
        })
        .expect("condition");
    // An image placed from BYTES on the rectangle.
    model
        .apply_mutation(&Mutation::ReplaceImageBytes {
            element_id: "r1".into(),
            bytes: Some(ByteBuf(tiny_png())),
        })
        .expect("image bytes");

    Authored {
        model,
        minted_story,
        table_id,
        hyperlink_id,
    }
}

#[test]
fn the_loss_list_is_measured_against_a_reparsed_export() {
    let a = author();
    let losses = a.model.idml_export_losses();
    for l in &losses {
        eprintln!("LOSS: {l}");
    }
    let has = |needle: &str| losses.iter().any(|l| l.contains(needle));

    // --- What the exporter now carries (each lane landed 2026-09-05,
    // measured against InDesign 20.0.1's own spelling): a line that
    // re-appears here is a lane that regressed. ---
    for closed in [
        "section `",
        "guide (",
        "table `",
        "hyperlink `Hyperlink/",
        "hyperlink destination `",
        "text source `",
        "condition `",
        "exports with different text",
    ] {
        assert!(
            !has(closed),
            "the `{closed}` lane regressed — this loss was closed: {losses:#?}"
        );
    }

    // The elements themselves are never named in a loss line any more.
    for id in [&a.minted_story, &a.table_id, &a.hyperlink_id] {
        assert!(
            !has(id),
            "`{id}` must not appear in the loss list: {losses:#?}"
        );
    }

    // --- No image loss either: a base-less export EMBEDS the bytes ---
    // (InDesign's `<Contents>` + `StoredState="Embedded"` spelling, read
    // back by the importer), so the pixels survive the re-parse.
    assert!(
        !has("image placed from bytes on `r1`"),
        "image loss: {losses:#?}"
    );
    assert!(losses.is_empty(), "nothing remains: {losses:#?}");
}

#[test]
fn an_unmutated_document_measures_no_loss() {
    let model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");
    let losses = model.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
}

/// With a LINK BASE the image placed from bytes is no longer a loss:
/// the export points its `<Link>` at `file:<base>/<name>` and hands the
/// bytes back under that name, so the caller writes the file and
/// InDesign reads exactly these pixels.
#[test]
fn with_a_link_base_the_bytes_placed_image_is_handed_back_and_is_no_loss() {
    let a = author();
    let (bytes, links) = a
        .model
        .export_idml_with_links(Some("/Users/me/Book Links/Links"))
        .expect("export");
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name, "r1.png");
    assert_eq!(links[0].bytes.as_deref(), Some(&tiny_png()[..]));
    assert_eq!(links[0].source_uri, "", "minted from bytes: no source URI");
    let spread = {
        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut e = zip.by_name("Spreads/Spread_s1.xml").unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut e, &mut s).unwrap();
        s
    };
    assert!(
        spread.contains(r#"LinkResourceURI="file:/Users/me/Book%20Links/Links/r1.png""#),
        "{spread}"
    );
    let losses = a
        .model
        .idml_export_losses_with_links(Some("/Users/me/Book Links/Links"));
    assert!(losses.is_empty(), "{losses:#?}");
    // Without a base the bytes are embedded instead — no loss either.
    let losses = a.model.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
    // `export_idml` is the base-less export, byte for byte.
    assert_eq!(
        a.model.export_idml().unwrap(),
        a.model.export_idml_with_links(None).unwrap().0
    );
    assert!(a.model.export_idml_with_links(None).unwrap().1.is_empty());
}

/// A story whose frame was deleted is an ORPHAN: InDesign discards it
/// on open, so the writer drops its part and its designmap reference,
/// and the loss ledger says so in one line naming the story.
#[test]
fn an_orphan_story_reaches_no_part_and_one_loud_ledger_line() {
    let mut model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");
    let out = model
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId("p1".into()),
            bounds: (420.0, 320.0, 560.0, 560.0),
        })
        .expect("frame");
    let frame = match out.created_id {
        Some(ElementId::TextFrame(id)) => id,
        other => panic!("expected a text frame, got {other:?}"),
    };
    let story = minted_story_of(&model, &frame);
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "Orphaned text".into(),
            cell: None,
        })
        .expect("text");
    model
        .apply_mutation(&Mutation::DeleteFrame {
            frame_id: frame.clone(),
        })
        .expect("delete frame");
    assert!(
        model.scene().stories.iter().any(|s| s.self_id == story),
        "the engine keeps the story after its frame is deleted"
    );

    let bytes = model.export_idml().expect("export");
    let names: Vec<String> = {
        let zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        zip.file_names().map(|n| n.unwrap().into_owned()).collect()
    };
    let part = format!("Stories/Story_{}.xml", story.replace('/', "_"));
    assert!(!names.contains(&part), "no part for the orphan: {names:?}");
    let designmap = {
        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut e = zip.by_name("designmap.xml").unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut e, &mut s).unwrap();
        s
    };
    assert!(
        !designmap.contains(&part),
        "no reference either: {designmap}"
    );

    let losses = model.idml_export_losses();
    assert_eq!(losses.len(), 1, "{losses:#?}");
    assert!(
        losses[0].contains(&format!(
            "story `{story}` (13 chars, 0 tables) is referenced by no frame"
        )) && losses[0].contains("dropped from the export; delete it or place it"),
        "{losses:#?}"
    );
}

fn story_part(package: &[u8], story: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).unwrap();
    let mut e = zip
        .by_name(&format!("Stories/Story_{}.xml", story.replace('/', "_")))
        .unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut e, &mut s).unwrap();
    s
}

fn minted_frame(model: &mut CanvasModel) -> String {
    let out = model
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId("p1".into()),
            bounds: (420.0, 320.0, 560.0, 560.0),
        })
        .expect("frame");
    match out.created_id {
        Some(ElementId::TextFrame(id)) => minted_story_of(model, &id),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

fn insert_table(model: &mut CanvasModel, story: &str) -> String {
    let out = model
        .apply_mutation(&Mutation::InsertTable {
            story_id: story.into(),
            rows: 3,
            cols: 2,
            header_rows: 0,
            footer_rows: 0,
            column_widths: vec![60.0, 60.0],
            row_heights: vec![20.0, 20.0, 20.0],
        })
        .expect("table");
    match out.created_id {
        Some(ElementId::Table { table_id, .. }) => table_id,
        other => panic!("expected a table, got {other:?}"),
    }
}

/// A jump line: "Continued on page" + the NEXT page number, then the
/// current one. The next page number is `<?ACE 18?>` in a range of its
/// own naming `PageNumberType="NextPageNumber"` (InDesign 20.0.1); a bare
/// one reads back as the current page number.
#[test]
fn a_next_page_number_inserted_through_the_engine_exports_without_loss() {
    use paged_mutate::operation::FieldKind;
    let mut model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");
    let story = minted_frame(&mut model);
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "Continued on page ; this is page .".into(),
            cell: None,
        })
        .expect("text");
    for (offset, field) in [(33, FieldKind::PageNumber), (18, FieldKind::NextPageNumber)] {
        model
            .apply_mutation(&Mutation::InsertField {
                story_id: story.clone(),
                offset,
                field,
                content_offset: None,
            })
            .expect("field");
    }
    let losses = model.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
    let part = story_part(&model.export_idml().unwrap(), &story);
    assert!(
        part.contains(r#"PageNumberType="NextPageNumber"><Content><?ACE 18?></Content>"#),
        "{part}"
    );

    // The same after a `.paged` round trip, which reloads the native model
    // and exports over the part the save wrote.
    let reopened = CanvasModel::load(
        "doc",
        &model.export_paged(PROTOCOL).unwrap(),
        CanvasOptions::default(),
    )
    .expect("reopen");
    let losses = reopened.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
}

const PROTOCOL: u32 = paged_canvas::channel::PROTOCOL_VERSION.0;

/// Reopen from `.paged`, delete the story's table, insert a different
/// one: the export holds the new table, not the deleted one. (The same
/// replace before any reopen always exported right.)
#[test]
fn a_table_replaced_after_a_paged_reopen_exports_without_loss() {
    let mut model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");
    let story = minted_frame(&mut model);
    let old = insert_table(&mut model, &story);
    let paged = model.export_paged(PROTOCOL).expect("save");

    let mut model = CanvasModel::load("doc", &paged, CanvasOptions::default()).expect("reopen");
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::DeleteTable {
                    story_id: story.clone(),
                    table_id: old.clone(),
                },
                Mutation::InsertTable {
                    story_id: story.clone(),
                    rows: 3,
                    cols: 2,
                    header_rows: 0,
                    footer_rows: 0,
                    column_widths: vec![60.0, 60.0],
                    row_heights: vec![20.0, 20.0, 20.0],
                },
            ],
        })
        .expect("replace");
    let new = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .and_then(|s| s.story.paragraphs.iter().find_map(|p| p.table.as_ref()))
        .and_then(|t| t.self_id.clone())
        .expect("the new table");
    assert_ne!(old, new, "a different table");

    let losses = model.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
    let part = story_part(&model.export_idml().unwrap(), &story);
    assert!(part.contains(&format!(r#"<Table Self="{new}""#)), "{part}");
    assert!(!part.contains(&format!(r#"<Table Self="{old}""#)), "{part}");
}

/// Reopen from `.paged`, pour text into a cell of the engine's table (a
/// plugin refreshing a placed table does this): the export carries the
/// text. The model's cells have no `Self` of their own, so none of them
/// met a cell of the saved part, and every cell exported what the save
/// held — with no loss reported, the table's id and shape being right.
#[test]
fn cell_text_poured_after_a_paged_reopen_exports_without_loss() {
    let mut model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");
    let story = minted_frame(&mut model);
    let table = insert_table(&mut model, &story);
    let paged = model.export_paged(PROTOCOL).expect("save");

    let mut model = CanvasModel::load("doc", &paged, CanvasOptions::default()).expect("reopen");
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "Opening balance".into(),
            cell: Some(paged_canvas::TextCellAddr {
                table_id: table.clone(),
                row: 1,
                col: 1,
            }),
        })
        .expect("cell text");
    let losses = model.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
    let part = story_part(&model.export_idml().unwrap(), &story);
    assert!(
        part.contains("<Content>Opening balance</Content>"),
        "{part}"
    );
}

/// One story in InDesign's shape for paragraphs of different styles — a
/// `<ParagraphStyleRange>` each, an empty one holding only its mark, the
/// last empty one a bare `<CharacterStyleRange/>`: Heading (empty) ·
/// Body "One" · Quote (empty) · Body "Three" · Body "Four" · Heading
/// (empty).
fn paragraphs_fixture() -> Vec<u8> {
    const STORY: &str = r#"<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Heading"><CharacterStyleRange><Br/></CharacterStyleRange></ParagraphStyleRange><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body"><CharacterStyleRange><Content>One</Content><Br/></CharacterStyleRange></ParagraphStyleRange><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Quote"><CharacterStyleRange><Br/></CharacterStyleRange></ParagraphStyleRange><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body"><CharacterStyleRange><Content>Three</Content><Br/></CharacterStyleRange></ParagraphStyleRange><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body"><CharacterStyleRange PointSize="14"><Content>Four</Content><Br/></CharacterStyleRange></ParagraphStyleRange><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Heading"><CharacterStyleRange/></ParagraphStyleRange>"#;
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let story = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><Story Self="story1">{STORY}</Story></idPkg:Story>"#
        );
        let parts: [(&str, &str); 4] = [
            (
                "designmap.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<Layer Self="ub6" Name="Layer 1" Visible="true" Locked="false" Printable="true"/>
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
<idPkg:Story src="Stories/Story_story1.xml"/>
</Document>"#,
            ),
            (
                "Resources/Styles.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<RootCharacterStyleGroup Self="u9d"><CharacterStyle Self="CharacterStyle/$ID/[No character style]" Name="$ID/[No character style]"/></RootCharacterStyleGroup>
<RootParagraphStyleGroup Self="u9e"><ParagraphStyle Self="ParagraphStyle/$ID/[No paragraph style]" Name="$ID/[No paragraph style]"/><ParagraphStyle Self="ParagraphStyle/Body" Name="Body"/><ParagraphStyle Self="ParagraphStyle/Heading" Name="Heading"/><ParagraphStyle Self="ParagraphStyle/Quote" Name="Quote"/></RootParagraphStyleGroup>
</idPkg:Styles>"#,
            ),
            (
                "Spreads/Spread_s1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            ),
            ("Stories/Story_story1.xml", &story),
        ];
        for (name, body) in parts {
            zip.start_file(name, deflated).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

/// `(style, text)` per paragraph of a story.
fn paragraph_shape(paragraphs: &[paged_model::Paragraph]) -> Vec<(String, String)> {
    paragraphs
        .iter()
        .map(|p| {
            (
                p.paragraph_style
                    .as_deref()
                    .unwrap_or("")
                    .trim_start_matches("ParagraphStyle/")
                    .to_string(),
                p.runs.iter().map(|r| r.text.as_str()).collect(),
            )
        })
        .collect()
}

/// The everyday edits to a story the part already holds, each through the
/// engine, straight from the IDML and after a `.paged` round trip: typing
/// into an empty paragraph (first, between two — in its own style — and
/// last), a new paragraph typed between two, Enter in the middle of a
/// run, two paragraphs joined by deleting the mark. Text typed into an
/// empty paragraph was dropped (no range of it resolved to the new run),
/// and a paragraph the engine added shifted every source paragraph after
/// it onto the wrong model paragraph.
#[test]
fn everyday_paragraph_edits_to_a_source_story_export_without_loss() {
    let insert = |offset: u32, text: &str| Mutation::InsertText {
        story_id: "story1".into(),
        offset,
        text: text.into(),
        cell: None,
    };
    // Offsets: Heading "" 0 · One 1..4 · Quote "" 5 · Three 6..11 ·
    // Four 12..16 · Heading "" 17 (one per paragraph boundary).
    // (what, the engine's ops, the story after: (style, text) per paragraph)
    type Edit = (
        &'static str,
        Vec<Mutation>,
        Vec<(&'static str, &'static str)>,
    );
    let edits: Vec<Edit> = vec![
        (
            "typed into the empty first paragraph",
            vec![insert(0, "Start")],
            vec![
                ("Heading", "Start"),
                ("Body", "One"),
                ("Quote", ""),
                ("Body", "Three"),
                ("Body", "Four"),
                ("Heading", ""),
            ],
        ),
        (
            "typed into the empty Quote between two",
            vec![insert(5, "Middle")],
            vec![
                ("Heading", ""),
                ("Body", "One"),
                ("Quote", "Middle"),
                ("Body", "Three"),
                ("Body", "Four"),
                ("Heading", ""),
            ],
        ),
        (
            "typed into the empty last paragraph",
            vec![insert(17, "End")],
            vec![
                ("Heading", ""),
                ("Body", "One"),
                ("Quote", ""),
                ("Body", "Three"),
                ("Body", "Four"),
                ("Heading", "End"),
            ],
        ),
        (
            "a paragraph typed after One",
            vec![insert(4, "\nInserted")],
            vec![
                ("Heading", ""),
                ("Body", "One"),
                ("Body", "Inserted"),
                ("Quote", ""),
                ("Body", "Three"),
                ("Body", "Four"),
                ("Heading", ""),
            ],
        ),
        (
            "Enter in the middle of Four",
            vec![insert(14, "\n")],
            vec![
                ("Heading", ""),
                ("Body", "One"),
                ("Quote", ""),
                ("Body", "Three"),
                ("Body", "Fo"),
                ("Body", "ur"),
                ("Heading", ""),
            ],
        ),
        (
            "Three and Four joined",
            vec![Mutation::DeleteRange {
                story_id: "story1".into(),
                start: 11,
                end: 12,
                cell: None,
            }],
            vec![
                ("Heading", ""),
                ("Body", "One"),
                ("Quote", ""),
                ("Body", "ThreeFour"),
                ("Heading", ""),
            ],
        ),
    ];
    for via_paged in [false, true] {
        for (label, ops, want) in &edits {
            let mut model =
                CanvasModel::load("doc", &paragraphs_fixture(), CanvasOptions::default())
                    .expect("load");
            if via_paged {
                let paged = model.export_paged(PROTOCOL).expect("save");
                model = CanvasModel::load("doc", &paged, CanvasOptions::default()).expect("reopen");
            }
            for op in ops {
                model.apply_mutation(op).expect(label);
            }
            let want: Vec<(String, String)> = want
                .iter()
                .map(|(s, t)| (s.to_string(), t.to_string()))
                .collect();
            let label = format!(
                "{label}{}",
                if via_paged {
                    " (after a .paged reopen)"
                } else {
                    ""
                }
            );
            assert_eq!(
                paragraph_shape(&model.scene().stories[0].story.paragraphs),
                want,
                "{label}: the engine's own account"
            );
            let losses = model.idml_export_losses();
            assert!(losses.is_empty(), "{label}: {losses:#?}");
            let twin = idml_import::import_idml_doc(&model.export_idml().unwrap()).unwrap();
            assert_eq!(
                paragraph_shape(&twin.stories[0].story.paragraphs),
                want,
                "{label}: the export, read back"
            );
        }
    }
}
