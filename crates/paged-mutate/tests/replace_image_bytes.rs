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

//! C-1 Stage B (pixel save-back) — `ReplaceImageBytes` kernel round-trips:
//! commit inline bytes onto a plain frame (undo restores was-absent),
//! replace bytes on a frame that already carries some (undo restores the
//! prior bytes + flag), clear, the oval lane, and the non-graphic reject.

use std::io::Write;

use paged_mutate::{apply, NodeId, Operation};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One spread: an image-bearing rectangle (`imgR`, linked, no inline
/// bytes), a plain colour rectangle (`plainR`), and a plain oval
/// (`oval1`). Mirrors the `place_image.rs` fixture.
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
    paged_parse::import_idml_doc(&idml_with_frames()).expect("open synthetic IDML")
}

fn rect<'a>(doc: &'a Document, id: &str) -> &'a paged_model::Rectangle {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(id))
        .expect("rectangle present")
}

fn replace(frame: NodeId, bytes: Option<&[u8]>) -> Operation {
    Operation::ReplaceImageBytes {
        frame,
        bytes: bytes.map(<[u8]>::to_vec),
        // Forward op: the apply layer captures the prior flag itself.
        prior_has_image_element: None,
    }
}

#[test]
fn commit_bytes_onto_plain_rectangle_sets_bytes_and_flag_and_undo_restores_absent() {
    let mut doc = open_doc();
    let frame = NodeId::Rectangle("plainR".to_string());
    assert_eq!(rect(&doc, "plainR").image_bytes, None);
    assert!(!rect(&doc, "plainR").has_image_element);

    let applied = apply(&mut doc, &replace(frame.clone(), Some(&[9, 8, 7, 6]))).expect("commit");
    let r = rect(&doc, "plainR");
    assert_eq!(r.image_bytes.as_deref(), Some(&[9u8, 8, 7, 6][..]));
    // Installing bytes makes the frame an image element so the renderer
    // paints the inline payload even though no <Image> was parsed.
    assert!(r.has_image_element);
    // image_link untouched (bytes outrank the link).
    assert_eq!(r.image_link, None);

    // Undo restores was-absent: no bytes, flag back to false.
    apply(&mut doc, &applied.inverse).expect("undo");
    let r = rect(&doc, "plainR");
    assert_eq!(r.image_bytes, None);
    assert!(!r.has_image_element);
}

#[test]
fn replace_over_existing_bytes_inverse_restores_prior_and_redo_reapplies() {
    let mut doc = open_doc();
    let frame = NodeId::Rectangle("plainR".to_string());

    // First commit establishes some inline bytes (+ the image-element flag).
    apply(&mut doc, &replace(frame.clone(), Some(&[1, 1, 1, 1]))).expect("first commit");
    assert_eq!(
        rect(&doc, "plainR").image_bytes.as_deref(),
        Some(&[1u8, 1, 1, 1][..])
    );

    // Second commit replaces them; the inverse must restore the prior bytes.
    let applied = apply(&mut doc, &replace(frame.clone(), Some(&[2, 2, 2, 2]))).expect("replace");
    assert_eq!(
        rect(&doc, "plainR").image_bytes.as_deref(),
        Some(&[2u8, 2, 2, 2][..])
    );

    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(
        rect(&doc, "plainR").image_bytes.as_deref(),
        Some(&[1u8, 1, 1, 1][..]),
        "undo restores the prior inline bytes"
    );
    assert!(rect(&doc, "plainR").has_image_element);

    // Redo via the undo's inverse reproduces the second commit.
    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(
        rect(&doc, "plainR").image_bytes.as_deref(),
        Some(&[2u8, 2, 2, 2][..])
    );
}

#[test]
fn clear_bytes_undo_restores_them() {
    let mut doc = open_doc();
    let frame = NodeId::Rectangle("plainR".to_string());
    apply(&mut doc, &replace(frame.clone(), Some(&[5, 5, 5, 5]))).expect("seed bytes");

    // Clear: bytes None. The frame stays an image element (a cleared
    // image frame is still an image frame, like an unreachable link).
    let applied = apply(&mut doc, &replace(frame.clone(), None)).expect("clear");
    assert_eq!(rect(&doc, "plainR").image_bytes, None);
    assert!(rect(&doc, "plainR").has_image_element);

    apply(&mut doc, &applied.inverse).expect("undo clear");
    assert_eq!(
        rect(&doc, "plainR").image_bytes.as_deref(),
        Some(&[5u8, 5, 5, 5][..]),
        "undo of a clear restores the cleared bytes"
    );
}

#[test]
fn oval_takes_inline_bytes() {
    let mut doc = open_doc();
    let applied = apply(
        &mut doc,
        &replace(NodeId::Oval("oval1".to_string()), Some(&[3, 3, 3, 3])),
    )
    .expect("commit on oval");
    let oval = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.ovals.iter())
        .find(|o| o.self_id.as_deref() == Some("oval1"))
        .expect("oval present");
    assert_eq!(oval.image_bytes.as_deref(), Some(&[3u8, 3, 3, 3][..]));
    assert!(oval.has_image_element);
    apply(&mut doc, &applied.inverse).expect("undo");
}

#[test]
fn text_frame_target_is_rejected() {
    let mut doc = open_doc();
    let err = apply(
        &mut doc,
        &replace(NodeId::TextFrame("tf1".to_string()), Some(&[1, 2])),
    );
    assert!(
        err.is_err(),
        "ReplaceImageBytes on a TextFrame must be rejected"
    );
}
