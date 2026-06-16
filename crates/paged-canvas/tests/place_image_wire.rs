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

//! v43 (D-14) — `Mutation::PlaceImage` through the canvas model: the
//! element id resolves to its frame kind, the op applies + undoes on
//! the worker undo log, bad targets fail cleanly, and the wire shape
//! is pinned. (Link/fit model truth is pinned by the paged-mutate
//! kernel tests; an unreachable uri renders the frame as before —
//! the build here has no asset resolver, exercising exactly that
//! honest-miss lane.)

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};

/// One page: a plain rectangle `plainR` and an oval `oval1`.
fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="plainR" GeometricBounds="50 50 250 250" ItemTransform="1 0 0 1 0 0" FillColor="Color/Red"/>
<Oval Self="oval1" GeometricBounds="300 50 500 250" ItemTransform="1 0 0 1 0 0" FillColor="Color/Blue"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("d14", &small_idml(), CanvasOptions::default()).expect("load")
}

#[test]
fn place_image_applies_and_undoes_through_the_worker_log() {
    let mut m = model();
    let out = m
        .apply_mutation(&Mutation::PlaceImage {
            element_id: "plainR".into(),
            uri: "file:///unreachable/cover.png".into(),
            fit: Some("FillProportionally".into()),
        })
        .expect("place image applies (and renders the frame as before — no resolver)");
    assert!(out.applied_seq > 0);

    // ONE undo step restores the empty frame.
    m.undo().expect("undo place image");

    // Oval lane (no fit — IDML fitting is Rectangle-only).
    m.apply_mutation(&Mutation::PlaceImage {
        element_id: "oval1".into(),
        uri: "file:///o.png".into(),
        fit: None,
    })
    .expect("place image on oval applies");
    m.undo().expect("undo oval place");
}

/// PlaceImage must flip the editor-side `has_image` geometry flag (which
/// drives the Properties panel's Image inspector / Frame Fitting), for
/// both Rectangles and Ovals — even though it sets only `image_link`
/// (not the parse-time `has_image_element`). Without this a from-scratch
/// placed image never surfaces the Image context.
#[test]
fn place_image_makes_geometry_report_has_image() {
    use paged_canvas::element_selection::ElementId;

    let mut m = model();
    let rect = ElementId::Rectangle("plainR".to_string());
    let oval = ElementId::Oval("oval1".to_string());

    // Before: neither frame carries a placed image.
    let before = m.element_geometry(&[rect.clone(), oval.clone()]);
    assert_eq!(before.len(), 2, "both frames resolve geometry");
    assert!(
        before.iter().all(|g| !g.has_image),
        "no image before PlaceImage"
    );

    // Place an image link on each (the link path — no resolver needed).
    m.apply_mutation(&Mutation::PlaceImage {
        element_id: "plainR".into(),
        uri: "file:///cover.png".into(),
        fit: Some("FillProportionally".into()),
    })
    .expect("place on rect");
    m.apply_mutation(&Mutation::PlaceImage {
        element_id: "oval1".into(),
        uri: "file:///o.png".into(),
        fit: None,
    })
    .expect("place on oval");

    // After: both report has_image — the Image inspector lights up.
    let after = m.element_geometry(&[rect, oval]);
    assert_eq!(after.len(), 2);
    assert!(
        after.iter().all(|g| g.has_image),
        "PlaceImage flips has_image for the Image inspector"
    );
}

#[test]
fn bad_targets_fail_cleanly() {
    let mut m = model();
    // Unknown element id.
    assert!(m
        .apply_mutation(&Mutation::PlaceImage {
            element_id: "nope".into(),
            uri: "file:///x.png".into(),
            fit: None,
        })
        .is_err());
    // Fit on an oval — rejected by the apply layer.
    assert!(m
        .apply_mutation(&Mutation::PlaceImage {
            element_id: "oval1".into(),
            uri: "file:///x.png".into(),
            fit: Some("Proportionally".into()),
        })
        .is_err());
}

#[test]
fn place_image_wire_shape_is_pinned() {
    let mutation = Mutation::PlaceImage {
        element_id: "plainR".into(),
        uri: "file:///assets/cover.png".into(),
        fit: Some("FillProportionally".into()),
    };
    assert_eq!(
        serde_json::to_value(&mutation).unwrap(),
        serde_json::json!({
            "op": "placeImage",
            "args": {
                "elementId": "plainR",
                "uri": "file:///assets/cover.png",
                "fit": "FillProportionally",
            },
        })
    );
    // `fit` omitted ⇒ leave fitting untouched.
    let bare: Mutation = serde_json::from_value(serde_json::json!({
        "op": "placeImage",
        "args": { "elementId": "plainR", "uri": "file:///a.png" },
    }))
    .expect("fit is optional on the wire");
    match bare {
        Mutation::PlaceImage { fit, .. } => assert_eq!(fit, None),
        other => panic!("unexpected: {other:?}"),
    }
}
