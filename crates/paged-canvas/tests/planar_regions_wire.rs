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

//! B-22 (protocol v57) — the planar-region READ door and the region
//! Pathfinder mutations across the canvas wire: the faces two
//! overlapping polygons resolve to, the Shape Builder point query, the
//! honest refusals, and the mutations' wire shapes.

use std::io::Write;

use paged_canvas::channel::Mutation;
use paged_canvas::{CanvasModel, CanvasOptions, ElementId};

/// Two overlapping straight-edged quads on one spread: `polyA` =
/// [0,20]², `polyB` = [10,30]².
fn two_quads_idml() -> Vec<u8> {
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
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Polygon Self="polyA" GeometricBounds="0 0 20 20" ItemTransform="1 0 0 1 0 0" FillColor="Color/Cyan">
  <Properties><PathGeometry><GeometryPathType pathOpen="false"><PathPointArray>
    <PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
    <PathPointType Anchor="20 0" LeftDirection="20 0" RightDirection="20 0"/>
    <PathPointType Anchor="20 20" LeftDirection="20 20" RightDirection="20 20"/>
    <PathPointType Anchor="0 20" LeftDirection="0 20" RightDirection="0 20"/>
  </PathPointArray></GeometryPathType></PathGeometry></Properties>
</Polygon>
<Polygon Self="polyB" GeometricBounds="10 10 30 30" ItemTransform="1 0 0 1 0 0" FillColor="Color/Magenta">
  <Properties><PathGeometry><GeometryPathType pathOpen="false"><PathPointArray>
    <PathPointType Anchor="10 10" LeftDirection="10 10" RightDirection="10 10"/>
    <PathPointType Anchor="30 10" LeftDirection="30 10" RightDirection="30 10"/>
    <PathPointType Anchor="30 30" LeftDirection="30 30" RightDirection="30 30"/>
    <PathPointType Anchor="10 30" LeftDirection="10 30" RightDirection="10 30"/>
  </PathPointArray></GeometryPathType></PathGeometry></Properties>
</Polygon>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("doc1", &two_quads_idml(), CanvasOptions::default()).expect("load")
}

fn both() -> Vec<ElementId> {
    vec![
        ElementId::Polygon("polyA".to_string()),
        ElementId::Polygon("polyB".to_string()),
    ]
}

fn polygon_count(m: &CanvasModel) -> usize {
    m.scene().spreads[0].spread.polygons.len()
}

// ---------------------------------------------------------------------------
// The read door
// ---------------------------------------------------------------------------

#[test]
fn planar_regions_enumerates_every_face() {
    let m = model();
    let reply = m.planar_regions(&both(), None);
    assert!(reply.found, "{:?}", reply.reason);
    assert!(reply.complete, "the faces must tile the union");
    assert_eq!(reply.input_count, 2);
    assert_eq!(reply.faces.len(), 3, "A-only, A∩B, B-only");

    let ids: Vec<&str> = reply.faces.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["0#0", "0-1#0", "1#0"]);
    let sigs: Vec<Vec<u32>> = reply.faces.iter().map(|f| f.signature.clone()).collect();
    assert_eq!(sigs, vec![vec![0], vec![0, 1], vec![1]]);

    // The overlap is the 10×10 square; the two L-shapes are 300 pt² each.
    let overlap = &reply.faces[1];
    assert!((overlap.area - 100.0).abs() < 0.01, "{}", overlap.area);
    assert_eq!(overlap.anchors.len(), 4);
    assert!((reply.faces[0].area - 300.0).abs() < 0.01);
    assert!((reply.faces[2].area - 300.0).abs() < 0.01);

    // Every face reports an interior point, and it really is interior.
    for face in &reply.faces {
        let inside = face.inside;
        let hit = m.planar_regions(&both(), Some(inside));
        assert!(hit.found);
        assert_eq!(hit.faces.len(), 1, "face {} inside {inside:?}", face.id);
        assert_eq!(hit.faces[0].id, face.id);
    }
}

#[test]
fn planar_regions_point_query_answers_one_face() {
    let m = model();
    // Inside both quads.
    let reply = m.planar_regions(&both(), Some([15.0, 15.0]));
    assert!(reply.found);
    assert_eq!(reply.faces.len(), 1);
    assert_eq!(reply.faces[0].id, "0-1#0");
    assert_eq!(reply.faces[0].signature, vec![0, 1]);

    // Inside the first only.
    let reply = m.planar_regions(&both(), Some([5.0, 5.0]));
    assert_eq!(reply.faces[0].signature, vec![0]);

    // Outside everything: found, but no face.
    let reply = m.planar_regions(&both(), Some([500.0, 500.0]));
    assert!(reply.found);
    assert!(reply.faces.is_empty());
}

#[test]
fn planar_regions_refuses_rather_than_guessing() {
    let m = model();
    // An id that doesn't resolve.
    let reply = m.planar_regions(&[ElementId::Polygon("nope".to_string())], None);
    assert!(!reply.found);
    assert!(reply.reason.is_some());
    assert!(reply.faces.is_empty());

    // Past the input cap: a refusal with a reason, never a truncation.
    let many: Vec<ElementId> = (0..20)
        .map(|i| ElementId::Polygon(format!("ghost{i}")))
        .collect();
    let reply = m.planar_regions(&many, None);
    assert!(!reply.found);

    // No elements at all.
    let reply = m.planar_regions(&[], None);
    assert!(!reply.found);
}

// ---------------------------------------------------------------------------
// The mutations across the bridge
// ---------------------------------------------------------------------------

#[test]
fn pathfinder_divide_across_the_wire_makes_one_element_per_face() {
    let mut m = model();
    assert_eq!(polygon_count(&m), 2);
    m.apply_mutation(&Mutation::PathfinderDivide {
        element_ids: both(),
    })
    .expect("divide");
    assert_eq!(polygon_count(&m), 3, "three faces ⇒ three elements");
    m.undo().expect("undo");
    assert_eq!(polygon_count(&m), 2, "one undo restores the originals");
}

#[test]
fn pathfinder_faces_across_the_wire_unites_the_named_faces() {
    let mut m = model();
    let ids: Vec<String> = m
        .planar_regions(&both(), None)
        .faces
        .into_iter()
        .map(|f| f.id)
        .collect();
    // Keep the two faces the FIRST quad covers: the result is that quad.
    m.apply_mutation(&Mutation::PathfinderFaces {
        element_ids: both(),
        faces: vec![ids[0].clone(), ids[1].clone()],
        mode: paged_mutate::FaceSelectMode::Keep,
    })
    .expect("shape builder");
    assert_eq!(polygon_count(&m), 1);
    let anchors = &m.scene().spreads[0].spread.polygons[0].anchors;
    let max_x = anchors
        .iter()
        .fold(f32::NEG_INFINITY, |a, p| a.max(p.anchor.0));
    assert!((max_x - 20.0).abs() < 0.01, "united back to the first quad");
    m.undo().expect("undo");
    assert_eq!(polygon_count(&m), 2);
}

#[test]
fn pathfinder_outline_across_the_wire_makes_open_segments() {
    let mut m = model();
    m.apply_mutation(&Mutation::PathfinderOutline {
        element_ids: both(),
    })
    .expect("outline");
    assert_eq!(polygon_count(&m), 0, "the fills are consumed");
    let lines = &m.scene().spreads[0].spread.graphic_lines;
    assert_eq!(lines.len(), 12, "4 sides each, split at the 2 crossings");
    assert!(lines.iter().all(|l| l.subpath_open == vec![true]));
    m.undo().expect("undo");
    assert_eq!(polygon_count(&m), 2);
    assert!(m.scene().spreads[0].spread.graphic_lines.is_empty());
}

#[test]
fn every_region_verb_applies_and_undoes_across_the_wire() {
    for mutation in [
        Mutation::PathfinderTrim {
            element_ids: both(),
        },
        Mutation::PathfinderMerge {
            element_ids: both(),
        },
        Mutation::PathfinderCrop {
            element_ids: both(),
        },
        Mutation::PathfinderMinusBack {
            element_ids: both(),
        },
    ] {
        let mut m = model();
        let name = mutation.discriminant();
        m.apply_mutation(&mutation)
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"));
        m.undo().unwrap_or_else(|| panic!("{name} undo failed"));
        assert_eq!(polygon_count(&m), 2, "{name} must round-trip");
    }
}

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

#[test]
fn v57_region_mutation_wire_shapes_round_trip() {
    let divide = Mutation::PathfinderDivide {
        element_ids: both(),
    };
    let json = serde_json::to_string(&divide).unwrap();
    assert!(
        json.contains("\"op\":\"pathfinderDivide\""),
        "tag missing: {json}"
    );
    assert!(json.contains("\"elementIds\""), "camelCase: {json}");
    serde_json::from_str::<Mutation>(&json).expect("round-trips");

    let faces = Mutation::PathfinderFaces {
        element_ids: both(),
        faces: vec!["0-1#0".to_string()],
        mode: paged_mutate::FaceSelectMode::Remove,
    };
    let json = serde_json::to_string(&faces).unwrap();
    assert!(json.contains("\"op\":\"pathfinderFaces\""), "{json}");
    assert!(json.contains("\"mode\":\"remove\""), "{json}");
    serde_json::from_str::<Mutation>(&json).expect("round-trips");

    for (mutation, tag) in [
        (
            Mutation::PathfinderTrim {
                element_ids: both(),
            },
            "pathfinderTrim",
        ),
        (
            Mutation::PathfinderMerge {
                element_ids: both(),
            },
            "pathfinderMerge",
        ),
        (
            Mutation::PathfinderCrop {
                element_ids: both(),
            },
            "pathfinderCrop",
        ),
        (
            Mutation::PathfinderOutline {
                element_ids: both(),
            },
            "pathfinderOutline",
        ),
        (
            Mutation::PathfinderMinusBack {
                element_ids: both(),
            },
            "pathfinderMinusBack",
        ),
    ] {
        let json = serde_json::to_string(&mutation).unwrap();
        assert!(json.contains(&format!("\"op\":\"{tag}\"")), "{json}");
        serde_json::from_str::<Mutation>(&json).expect("round-trips");
    }
}

#[test]
fn v57_planar_regions_request_and_reply_wire_shapes() {
    use paged_canvas::channel::{MainToWorkerKind, WorkerToMainKind};

    let request = MainToWorkerKind::RequestPlanarRegions {
        element_ids: both(),
        point: Some([15.0, 15.0]),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("requestPlanarRegions"), "{json}");
    assert!(json.contains("\"elementIds\""), "{json}");
    serde_json::from_str::<MainToWorkerKind>(&json).expect("round-trips");

    // `point` is optional — an omitted field means "enumerate them all".
    let without_point = json.replace(",\"point\":[15.0,15.0]", "");
    serde_json::from_str::<MainToWorkerKind>(&without_point)
        .expect("point is optional on the wire");

    let reply = WorkerToMainKind::PlanarRegions {
        result: model().planar_regions(&both(), None),
    };
    let json = serde_json::to_string(&reply).unwrap();
    assert!(json.contains("planarRegions"), "{json}");
    assert!(json.contains("\"subpathStarts\""), "camelCase: {json}");
    assert!(json.contains("\"inputCount\":2"), "{json}");
    serde_json::from_str::<WorkerToMainKind>(&json).expect("round-trips");
}
