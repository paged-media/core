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

    // --- The one honest, permanent loss: IDML cannot embed pixels. ---
    assert!(
        has("image placed from bytes on `r1`") && has("no IDML link to point at"),
        "image loss: {losses:#?}"
    );
    assert_eq!(
        losses.len(),
        1,
        "exactly the image line remains: {losses:#?}"
    );
}

#[test]
fn an_unmutated_document_measures_no_loss() {
    let model = CanvasModel::load("doc", &fixture(), CanvasOptions::default()).expect("load");
    let losses = model.idml_export_losses();
    assert!(losses.is_empty(), "{losses:#?}");
}
