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

//! v43 (D-14) — `PlaceImage` kernel round-trips: place onto an empty
//! rectangle, place over an existing parsed link (inverse restores),
//! redo, oval/polygon link lane, and the Rectangle-only fit guard.

use std::io::Write;

use paged_mutate::{apply, NodeId, Operation};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One spread: an image-bearing rectangle (`imgR`, linked to
/// `file:///placeholder.jpg`), a plain colour rectangle (`plainR`),
/// and a plain oval (`oval1`).
fn idml_with_frames() -> Vec<u8> {
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
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" Self="d1">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1">
    <Page Self="p1" GeometricBounds="0 0 600 612" ItemTransform="1 0 0 1 0 0"/>
    <Rectangle Self="imgR" GeometricBounds="50 50 250 250" ItemTransform="1 0 0 1 0 0">
      <Image Self="img1" ItemTransform="1 0 0 1 0 0">
        <Link Self="link1" LinkResourceURI="file:///placeholder.jpg"/>
      </Image>
    </Rectangle>
    <Rectangle Self="plainR" GeometricBounds="300 50 500 250" ItemTransform="1 0 0 1 0 0" FillColor="Color/Red"/>
    <Oval Self="oval1" GeometricBounds="50 300 250 500" ItemTransform="1 0 0 1 0 0" FillColor="Color/Blue"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

fn open_doc() -> Document {
    Document::open(&idml_with_frames()).expect("open synthetic IDML")
}

fn rect<'a>(doc: &'a Document, id: &str) -> &'a paged_model::Rectangle {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(id))
        .expect("rectangle present")
}

fn place(frame: NodeId, uri: Option<&str>, fit: Option<&str>) -> Operation {
    Operation::PlaceImage {
        frame,
        image_uri: uri.map(str::to_string),
        fit: fit.map(str::to_string),
    }
}

#[test]
fn place_onto_empty_rectangle_sets_link_and_fit_and_undo_clears() {
    let mut doc = open_doc();
    let frame = NodeId::Rectangle("plainR".to_string());
    assert_eq!(rect(&doc, "plainR").image_link, None);

    let applied = apply(
        &mut doc,
        &place(frame, Some("file:///cover.png"), Some("FillProportionally")),
    )
    .expect("place");
    let r = rect(&doc, "plainR");
    assert_eq!(r.image_link.as_deref(), Some("file:///cover.png"));
    assert_eq!(
        r.frame_fitting
            .as_ref()
            .and_then(|f| f.fitting_on_empty_frame.as_deref()),
        Some("FillProportionally")
    );
    // The honest-miss contract: placement does not fake an <Image>
    // element, so an unreachable uri renders the frame as before
    // (fill, no missing-image badge).
    assert!(!r.has_image_element);

    apply(&mut doc, &applied.inverse).expect("undo");
    let r = rect(&doc, "plainR");
    assert_eq!(r.image_link, None);
    assert_eq!(
        r.frame_fitting
            .as_ref()
            .and_then(|f| f.fitting_on_empty_frame.as_deref()),
        None
    );
}

#[test]
fn place_over_existing_link_inverse_restores_and_redo_reapplies() {
    let mut doc = open_doc();
    let frame = NodeId::Rectangle("imgR".to_string());
    assert_eq!(
        rect(&doc, "imgR").image_link.as_deref(),
        Some("file:///placeholder.jpg")
    );

    let applied = apply(&mut doc, &place(frame, Some("file:///swap.png"), None)).expect("place");
    assert_eq!(
        rect(&doc, "imgR").image_link.as_deref(),
        Some("file:///swap.png")
    );

    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(
        rect(&doc, "imgR").image_link.as_deref(),
        Some("file:///placeholder.jpg")
    );

    // Redo via the undo's inverse reproduces the placement.
    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(
        rect(&doc, "imgR").image_link.as_deref(),
        Some("file:///swap.png")
    );
}

#[test]
fn oval_takes_a_link_but_rejects_fit() {
    let mut doc = open_doc();
    let applied = apply(
        &mut doc,
        &place(
            NodeId::Oval("oval1".to_string()),
            Some("file:///o.png"),
            None,
        ),
    )
    .expect("place on oval");
    let oval = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.ovals.iter())
        .find(|o| o.self_id.as_deref() == Some("oval1"))
        .expect("oval present");
    assert_eq!(oval.image_link.as_deref(), Some("file:///o.png"));
    apply(&mut doc, &applied.inverse).expect("undo");

    // `fit` rides IDML's <FrameFittingOption>, which only nests in
    // Rectangles.
    let err = apply(
        &mut doc,
        &place(
            NodeId::Oval("oval1".to_string()),
            Some("file:///o.png"),
            Some("Proportionally"),
        ),
    );
    assert!(err.is_err(), "fit on an oval must be rejected");
}

#[test]
fn text_frame_target_is_rejected() {
    let mut doc = open_doc();
    let err = apply(
        &mut doc,
        &place(
            NodeId::TextFrame("tf1".to_string()),
            Some("file:///x.png"),
            None,
        ),
    );
    assert!(err.is_err(), "PlaceImage on a TextFrame must be rejected");
}
