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

//! Editor-ops — structural-insert integration tests.
//!
//! Drives `Mutation::InsertFrame` / `InsertLine` / `InsertPath` /
//! `SetDocumentDefaults` through `apply_mutation` against a real
//! parsed IDML and asserts scene state, `frames_in_order` upkeep,
//! minted-id uniqueness, `created_id` reporting, document-default
//! consultation, and undo/redo round-trips.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, PageId};
use paged_mutate::operation::PathAnchorSpec;

fn small_idml() -> Vec<u8> {
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
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load")
}

#[test]
fn insert_frame_creates_rectangle_and_reports_created_id() {
    let mut model = load();
    let outcome = model
        .apply_mutation(&Mutation::InsertFrame {
            page_id: PageId("p1".into()),
            bounds: (10.0, 20.0, 110.0, 220.0),
        })
        .expect("insert frame");
    assert!(outcome.applied_seq > 0);
    let created = outcome.created_id.expect("created id reported");
    let id = match &created {
        paged_canvas::ElementId::Rectangle(id) => id.clone(),
        other => panic!("expected Rectangle created_id, got {other:?}"),
    };
    let spread = &model.scene().spreads[0].spread;
    let rect = spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(id.as_str()))
        .expect("rectangle inserted");
    // Page p1 sits at spread origin (0, 0) in this fixture, so spread
    // coords equal the page-local bounds.
    assert!((rect.bounds.top - 10.0).abs() < 1e-3);
    assert!((rect.bounds.left - 20.0).abs() < 1e-3);
    assert!((rect.bounds.bottom - 110.0).abs() < 1e-3);
    assert!((rect.bounds.right - 220.0).abs() < 1e-3);
    // Registered in the z-order table (on top).
    assert!(matches!(
        spread.frames_in_order.last(),
        Some(paged_parse::FrameRef::Rectangle(_)),
    ));
}

#[test]
fn insert_frame_consults_document_defaults_and_defaults_are_not_undoable() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: Some("Color/Red".into()),
            stroke_color: Some("Color/Black".into()),
            stroke_weight: Some(2.0),
        })
        .expect("set defaults");
    // App-level state: no undo entry was created.
    assert!(model.applied_log_back().is_none());
    // …and the meta reply surfaces the triple.
    let meta = model.document_meta();
    assert_eq!(meta.default_fill_color.as_deref(), Some("Color/Red"));
    assert_eq!(meta.default_stroke_color.as_deref(), Some("Color/Black"));
    assert_eq!(meta.default_stroke_weight, Some(2.0));

    let outcome = model
        .apply_mutation(&Mutation::InsertFrame {
            page_id: PageId("p1".into()),
            bounds: (0.0, 0.0, 50.0, 50.0),
        })
        .expect("insert frame");
    let id = match outcome.created_id.expect("created id") {
        paged_canvas::ElementId::Rectangle(id) => id,
        other => panic!("expected Rectangle, got {other:?}"),
    };
    let rect = model.scene().spreads[0]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(id.as_str()))
        .expect("rectangle");
    assert_eq!(rect.fill_color.as_deref(), Some("Color/Red"));
    assert_eq!(rect.stroke_color.as_deref(), Some("Color/Black"));
    assert_eq!(rect.stroke_weight, Some(2.0));
}

#[test]
fn insert_frame_undo_redo_round_trips() {
    let mut model = load();
    let before = format!("{:?}", model.scene().spreads);
    let outcome = model
        .apply_mutation(&Mutation::InsertFrame {
            page_id: PageId("p1".into()),
            bounds: (10.0, 10.0, 60.0, 60.0),
        })
        .expect("insert");
    let after_insert = format!("{:?}", model.scene().spreads);
    assert_ne!(before, after_insert);

    model.undo().expect("undo");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        before,
        "undo of insert must restore the scene byte-identically"
    );
    model.redo().expect("redo");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        after_insert,
        "redo must re-insert identically (same minted id, same z slot)"
    );
    let _ = outcome;
}

#[test]
fn minted_ids_are_unique_across_inserts() {
    let mut model = load();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let outcome = model
            .apply_mutation(&Mutation::InsertFrame {
                page_id: PageId("p1".into()),
                bounds: (0.0, 0.0, 10.0, 10.0),
            })
            .expect("insert");
        match outcome.created_id.expect("created id") {
            paged_canvas::ElementId::Rectangle(id) => ids.push(id),
            other => panic!("expected Rectangle, got {other:?}"),
        }
    }
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "ids must be unique: {ids:?}");
    for id in &ids {
        assert!(id.starts_with('u'), "minted ids use the bare u<hex> style");
    }
}

#[test]
fn batch_of_insert_frames_mints_distinct_ids_and_single_undo_removes_all() {
    // FINDING #6 — the editor's gridify N×M sends ONE
    // `Mutation::Batch { ops: [InsertFrame; N] }`. Pre-fix, every child
    // translated against the same unmutated scene and minted the SAME
    // `u<max+1>`, so `paged_mutate::apply` rejected the 2nd insert with
    // "duplicate self_id … IDML node IDs must be unique".
    let mut model = load();
    let rects_before = model.scene().spreads[0].spread.rectangles.len();
    let before = format!("{:?}", model.scene().spreads);

    let batch = Mutation::Batch {
        ops: vec![
            Mutation::InsertFrame {
                page_id: PageId("p1".into()),
                bounds: (0.0, 0.0, 10.0, 10.0),
            },
            Mutation::InsertFrame {
                page_id: PageId("p1".into()),
                bounds: (20.0, 0.0, 30.0, 10.0),
            },
            Mutation::InsertFrame {
                page_id: PageId("p1".into()),
                bounds: (40.0, 0.0, 50.0, 10.0),
            },
        ],
    };
    let outcome = model.apply_mutation(&batch).expect("gridify batch applies");
    assert!(outcome.applied_seq > 0);

    // Three new rectangles, each with a DISTINCT self_id.
    let spread = &model.scene().spreads[0].spread;
    assert_eq!(
        spread.rectangles.len(),
        rects_before + 3,
        "batch of 3 inserts must create 3 frames"
    );
    let new_ids: Vec<String> = spread.rectangles[rects_before..]
        .iter()
        .map(|r| r.self_id.clone().expect("minted id"))
        .collect();
    let unique: std::collections::HashSet<_> = new_ids.iter().collect();
    assert_eq!(
        unique.len(),
        3,
        "batch inserts must mint distinct ids, got {new_ids:?}"
    );

    // Single undo removes ALL three (one undoable Batch op) and restores
    // the scene byte-identically.
    model.undo().expect("undo");
    assert_eq!(
        model.scene().spreads[0].spread.rectangles.len(),
        rects_before,
        "single undo of the batch must remove all 3 frames"
    );
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        before,
        "undo of the batch must restore the scene byte-identically"
    );
}

#[test]
fn insert_line_creates_two_anchor_open_graphic_line() {
    let mut model = load();
    let outcome = model
        .apply_mutation(&Mutation::InsertLine {
            page_id: PageId("p1".into()),
            start: (30.0, 40.0),
            end: (130.0, 90.0),
        })
        .expect("insert line");
    let id = match outcome.created_id.expect("created id") {
        paged_canvas::ElementId::GraphicLine(id) => id,
        other => panic!("expected GraphicLine, got {other:?}"),
    };
    let line = model.scene().spreads[0]
        .spread
        .graphic_lines
        .iter()
        .find(|l| l.self_id.as_deref() == Some(id.as_str()))
        .expect("line inserted");
    assert_eq!(line.anchors.len(), 2);
    assert_eq!(line.anchors[0].anchor, (30.0, 40.0));
    assert_eq!(line.anchors[1].anchor, (130.0, 90.0));
    assert_eq!(line.subpath_open, vec![true]);
    // Lines must be visible: a stroke fallback applies when no
    // document default is set.
    assert!(line.stroke_color.is_some());
    assert_eq!(line.stroke_weight, Some(1.0));
    // Bounds cover the segment.
    assert!((line.bounds.top - 40.0).abs() < 1e-3);
    assert!((line.bounds.left - 30.0).abs() < 1e-3);
}

#[test]
fn insert_path_polyline_and_smooth_fit() {
    let mut model = load();
    let corner = |x: f32, y: f32| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    };
    // Plain polyline (smooth: false) keeps the corner anchors.
    let outcome = model
        .apply_mutation(&Mutation::InsertPath {
            page_id: PageId("p1".into()),
            anchors: vec![corner(0.0, 0.0), corner(40.0, 10.0), corner(80.0, 0.0)],
            open: true,
            smooth: false,
        })
        .expect("insert path");
    let id = match outcome.created_id.expect("created id") {
        paged_canvas::ElementId::Polygon(id) => id,
        other => panic!("expected Polygon, got {other:?}"),
    };
    let poly = model.scene().spreads[0]
        .spread
        .polygons
        .iter()
        .find(|p| p.self_id.as_deref() == Some(id.as_str()))
        .expect("polygon inserted");
    assert_eq!(poly.anchors.len(), 3);
    assert_eq!(poly.subpath_open, vec![true]);

    // A densely-sampled arc with smooth: true comes back FITTED:
    // fewer anchors than samples, with real curve handles.
    let samples: Vec<PathAnchorSpec> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let x = t * 120.0;
            let y = 40.0 * (std::f32::consts::PI * t).sin();
            corner(x, y)
        })
        .collect();
    let outcome = model
        .apply_mutation(&Mutation::InsertPath {
            page_id: PageId("p1".into()),
            anchors: samples.clone(),
            open: true,
            smooth: true,
        })
        .expect("insert smooth path");
    let id = match outcome.created_id.expect("created id") {
        paged_canvas::ElementId::Polygon(id) => id,
        other => panic!("expected Polygon, got {other:?}"),
    };
    let poly = model.scene().spreads[0]
        .spread
        .polygons
        .iter()
        .find(|p| p.self_id.as_deref() == Some(id.as_str()))
        .expect("fitted polygon");
    assert!(
        poly.anchors.len() < samples.len(),
        "fit should compress {} samples (got {})",
        samples.len(),
        poly.anchors.len()
    );
    // At least one interior anchor carries genuine curve handles.
    assert!(
        poly.anchors
            .iter()
            .any(|a| a.left != a.anchor || a.right != a.anchor),
        "smooth fit should produce Bezier handles"
    );
}
