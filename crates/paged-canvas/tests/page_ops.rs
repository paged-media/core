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

//! Editor-ops — Page tool integration tests: InsertPage (new
//! single-page spread), DeletePage (whole-spread lossless capture),
//! ResizePage, the v1 guards (multi-page spread, last page), and the
//! `page_structure_changed` reply flag.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, PageId};

fn idml(spread_xml: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(spread_xml.as_bytes()).unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn single_page_doc() -> CanvasModel {
    let spread = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#;
    CanvasModel::load("doc1", &idml(spread), CanvasOptions::default()).expect("load")
}

fn two_page_spread_doc() -> CanvasModel {
    let spread = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="2">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Page Self="p2" Name="2" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 612 0"/>
</Spread></idPkg:Spread>"#;
    CanvasModel::load("doc1", &idml(spread), CanvasOptions::default()).expect("load")
}

#[test]
fn insert_page_appends_a_single_page_spread() {
    let mut model = single_page_doc();
    assert_eq!(model.page_count(), 1);
    let outcome = model
        .apply_mutation(&Mutation::InsertPage {
            after_page_id: Some(PageId("p1".into())),
            master_id: None,
        })
        .expect("insert page");
    assert!(outcome.page_structure_changed, "page list changed");
    assert_eq!(model.page_count(), 2, "a page was added");
    assert_eq!(model.scene().spreads.len(), 2, "as its own spread");
    let new_spread = &model.scene().spreads[1].spread;
    assert_eq!(new_spread.pages.len(), 1);
    // Size cloned from the reference page.
    let p = &new_spread.pages[0];
    assert!((p.bounds.bottom - 792.0).abs() < 1e-3);
    assert!((p.bounds.right - 612.0).abs() < 1e-3);
    // Stacked below the existing content on the pasteboard.
    let ty = new_spread.item_transform.map(|m| m[5]).unwrap_or(0.0);
    assert!(ty > 792.0, "new spread stacks below (ty={ty})");
}

#[test]
fn insert_page_undo_redo_round_trips() {
    let mut model = single_page_doc();
    let before = format!("{:?}", model.scene().spreads);
    model
        .apply_mutation(&Mutation::InsertPage {
            after_page_id: Some(PageId("p1".into())),
            master_id: None,
        })
        .expect("insert page");
    let after = format!("{:?}", model.scene().spreads);
    model.undo().expect("undo");
    assert_eq!(format!("{:?}", model.scene().spreads), before);
    assert_eq!(model.page_count(), 1, "built pages re-derived");
    model.redo().expect("redo");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        after,
        "redo recreates the spread with the same minted ids"
    );
}

#[test]
fn delete_page_round_trips_with_its_items() {
    let mut model = single_page_doc();
    // Add a second page so p1 isn't the only one.
    model
        .apply_mutation(&Mutation::InsertPage {
            after_page_id: Some(PageId("p1".into())),
            master_id: None,
        })
        .expect("insert page");
    let before = format!("{:?}", model.scene().spreads);
    // Delete p1 — its spread hosts tf1 + r1; the capture must be
    // lossless.
    let outcome = model
        .apply_mutation(&Mutation::DeletePage {
            page_id: PageId("p1".into()),
        })
        .expect("delete page");
    assert!(outcome.page_structure_changed);
    assert_eq!(model.page_count(), 1);
    assert!(
        !model.scene().spreads.iter().any(|s| s
            .spread
            .text_frames
            .iter()
            .any(|f| f.self_id.as_deref() == Some("tf1"))),
        "tf1 went with its page"
    );
    model.undo().expect("undo");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        before,
        "undo restores the spread + every page item byte-identically"
    );
}

#[test]
fn delete_last_page_is_rejected() {
    let mut model = single_page_doc();
    let err = model
        .apply_mutation(&Mutation::DeletePage {
            page_id: PageId("p1".into()),
        })
        .expect_err("deleting the only page");
    let msg = format!("{err:?}");
    assert!(msg.contains("only page"), "got: {msg}");
}

#[test]
fn delete_page_in_multi_page_spread_is_rejected() {
    let mut model = two_page_spread_doc();
    let err = model
        .apply_mutation(&Mutation::DeletePage {
            page_id: PageId("p1".into()),
        })
        .expect_err("multi-page spread deletion");
    let msg = format!("{err:?}");
    assert!(msg.contains("multi-page"), "got: {msg}");
}

#[test]
fn resize_page_round_trips_and_rebuilds_size() {
    let mut model = single_page_doc();
    let outcome = model
        .apply_mutation(&Mutation::ResizePage {
            page_id: PageId("p1".into()),
            bounds: (0.0, 0.0, 600.0, 400.0),
        })
        .expect("resize page");
    assert!(outcome.page_structure_changed);
    let built = model.page(&PageId("p1".into())).expect("page");
    assert!(
        (built.width_pt - 400.0).abs() < 1e-3,
        "w={}",
        built.width_pt
    );
    assert!(
        (built.height_pt - 600.0).abs() < 1e-3,
        "h={}",
        built.height_pt
    );
    model.undo().expect("undo");
    let built = model.page(&PageId("p1".into())).expect("page");
    assert!((built.width_pt - 612.0).abs() < 1e-3);
    assert!((built.height_pt - 792.0).abs() < 1e-3);
}
