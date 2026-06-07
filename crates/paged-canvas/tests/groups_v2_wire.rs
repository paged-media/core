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

//! W1.20 (groups v2) — end-to-end WIRE test of nested create, group
//! transforms, and nested dissolve through `Mutation` (the surface the
//! editor sends). Exercises the full chain the matrix calls out:
//!
//!   create-nested → tree read-back → SetGroupTransform → hit-test at
//!   the TRANSFORMED position (renderer + hit-tester agree) → dissolve
//!   → effective geometry unchanged.

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, ElementId, PageId};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}
fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Spread with TWO top-level Rectangles plus a Group hosting two more
/// Rectangles. Lets us nest the existing group with a top-level rect
/// (group-of-groups) and hit-test the group's leaf after a transform.
fn build_idml() -> Vec<u8> {
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
    // A "groupLeaf" rectangle sits at GeometricBounds top=100 left=100
    // bottom=150 right=150 with an identity transform (no group
    // transform yet). After we translate the group by (200, 50), its
    // member's effective transform moves it to roughly (300..350,
    // 150..200) in spread space — which is where we'll hit-test.
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 700 700"/>
    <Rectangle Self="topA" GeometricBounds="0 0 40 40" StrokeWeight="0"/>
    <Group Self="g1">
      <Rectangle Self="groupLeaf" GeometricBounds="100 100 150 150" StrokeWeight="0"/>
      <Rectangle Self="groupLeaf2" GeometricBounds="100 200 150 250" StrokeWeight="0"/>
    </Group>
    <Rectangle Self="topB" GeometricBounds="0 500 40 540" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

fn load_model() -> CanvasModel {
    let bytes = build_idml();
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("doc", &bytes, opts).expect("load + build")
}

/// Effective `item_transform` of a leaf rectangle.
fn rect_transform(model: &CanvasModel, self_id: &str) -> Option<[f32; 6]> {
    for parsed in &model.scene().spreads {
        for r in &parsed.spread.rectangles {
            if r.self_id.as_deref() == Some(self_id) {
                return r.item_transform;
            }
        }
    }
    panic!("rect {self_id} not found");
}

/// Walk the scene tree to the node with the given Group id; return its
/// children's element ids (for the tree read-back assertion).
fn group_children_ids(
    nodes: &[paged_canvas::channel::SceneTreeNode],
    group_id: &str,
) -> Option<Vec<ElementId>> {
    for n in nodes {
        if let Some(ElementId::Group(gid)) = &n.id {
            if gid == group_id {
                return Some(n.children.iter().filter_map(|c| c.id.clone()).collect());
            }
        }
        if let Some(found) = group_children_ids(&n.children, group_id) {
            return Some(found);
        }
    }
    None
}

#[test]
fn nested_create_tree_transform_hit_dissolve_roundtrip() {
    let mut model = load_model();
    let page = PageId("p1".to_string());

    // --- NESTED CREATE: group the existing group g1 with top rect A.
    let outcome = model
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![
                ElementId::Group("g1".to_string()),
                ElementId::Rectangle("topA".to_string()),
            ],
        })
        .expect("create nested group");
    let outer_id = match outcome.created_id {
        Some(ElementId::Group(id)) => id,
        other => panic!("expected a created Group id, got {other:?}"),
    };

    // --- TREE READ-BACK: the outer group nests g1 (which itself nests
    // groupLeaf / groupLeaf2) plus topA.
    let tree = model.scene_tree();
    let outer_children = group_children_ids(&tree, &outer_id).expect("outer group in tree");
    assert!(
        outer_children
            .iter()
            .any(|c| matches!(c, ElementId::Group(g) if g == "g1")),
        "outer group must nest g1: {outer_children:?}"
    );
    assert!(
        outer_children
            .iter()
            .any(|c| matches!(c, ElementId::Rectangle(r) if r == "topA")),
        "outer group must contain topA: {outer_children:?}"
    );
    // g1 still nests its two original leaves.
    let g1_children = group_children_ids(&tree, "g1").expect("g1 still in tree");
    assert_eq!(
        g1_children.len(),
        2,
        "g1 keeps its two leaves: {g1_children:?}"
    );

    // Pre-transform: the group leaf sits at its bounds (identity).
    assert_eq!(rect_transform(&model, "groupLeaf"), None);
    // A hit at the leaf's ORIGINAL centre (~125, 125) lands the leaf.
    let before_hit = model.hit_test(&page, (125.0, 125.0));
    assert_eq!(
        before_hit.element,
        Some(ElementId::Rectangle("groupLeaf".to_string())),
        "pre-transform hit at the leaf's original position"
    );

    // --- GROUP TRANSFORM: translate the OUTER group by (200, 50) as a
    // unit. The leaf (nested two levels deep) must follow.
    let g_new = [1.0, 0.0, 0.0, 1.0, 200.0, 50.0];
    model
        .apply_mutation(&Mutation::SetGroupTransform {
            group_id: outer_id.clone(),
            transform: Some(g_new),
        })
        .expect("set group transform");

    // The deeply-nested leaf's EFFECTIVE transform now carries (200,50).
    let leaf_t =
        rect_transform(&model, "groupLeaf").expect("leaf has a transform after group move");
    assert!((leaf_t[4] - 200.0).abs() < 1e-3, "leaf tx={}", leaf_t[4]);
    assert!((leaf_t[5] - 50.0).abs() < 1e-3, "leaf ty={}", leaf_t[5]);

    // --- HIT-TEST at the TRANSFORMED position: the leaf moved from
    // (100..150, 100..150) to (300..350, 150..200). A hit at the OLD
    // centre misses the leaf; a hit at the NEW centre lands it. This is
    // the renderer/hit-tester parity proof — both read the same
    // effective transform.
    let new_centre = (125.0 + 200.0, 125.0 + 50.0); // (325, 175)
    let hit_new = model.hit_test(&page, new_centre);
    assert_eq!(
        hit_new.element,
        Some(ElementId::Rectangle("groupLeaf".to_string())),
        "post-transform hit at the leaf's NEW position must land the leaf"
    );
    // The hit reports the full group chain (outer → g1).
    assert!(
        hit_new.group_chain.contains(&outer_id) && hit_new.group_chain.contains(&"g1".to_string()),
        "hit group_chain must include both ancestors: {:?}",
        hit_new.group_chain
    );
    // And the OLD centre no longer hits the leaf.
    let hit_old = model.hit_test(&page, (125.0, 125.0));
    assert_ne!(
        hit_old.element,
        Some(ElementId::Rectangle("groupLeaf".to_string())),
        "the leaf vacated its original position"
    );

    // --- DISSOLVE the INNER group g1: its members splice into the outer
    // group. Effective geometry is unchanged (the leaf stays put).
    let leaf_before_dissolve = rect_transform(&model, "groupLeaf");
    model
        .apply_mutation(&Mutation::DissolveGroup {
            group_id: "g1".to_string(),
        })
        .expect("dissolve inner group");
    // g1 is gone from the tree; its leaves are now direct children of
    // the outer group.
    let tree = model.scene_tree();
    assert!(
        group_children_ids(&tree, "g1").is_none(),
        "inner group g1 must be gone after dissolve"
    );
    let outer_children = group_children_ids(&tree, &outer_id).expect("outer group still present");
    assert!(
        outer_children
            .iter()
            .any(|c| matches!(c, ElementId::Rectangle(r) if r == "groupLeaf")),
        "groupLeaf spliced into the outer group: {outer_children:?}"
    );
    // GEOMETRY INVARIANT: the leaf's effective transform is unchanged.
    assert_eq!(
        rect_transform(&model, "groupLeaf"),
        leaf_before_dissolve,
        "nested dissolve must not move the leaf"
    );
    // Hit-test at the transformed position still lands the leaf.
    let hit_after = model.hit_test(&page, new_centre);
    assert_eq!(
        hit_after.element,
        Some(ElementId::Rectangle("groupLeaf".to_string())),
        "post-dissolve hit at the transformed position still lands the leaf"
    );

    // --- UNDO the dissolve: g1 re-nests inside the outer group.
    model.undo().expect("undo dissolve");
    let tree = model.scene_tree();
    let g1_children = group_children_ids(&tree, "g1").expect("g1 restored after undo");
    assert_eq!(g1_children.len(), 2, "undo restores g1's two leaves");
    assert_eq!(
        rect_transform(&model, "groupLeaf"),
        leaf_before_dissolve,
        "undo keeps the leaf geometry"
    );

    // --- UNDO the transform: the leaf returns to its original position.
    model.undo().expect("undo group transform");
    assert_eq!(
        rect_transform(&model, "groupLeaf"),
        None,
        "undo of the group transform clears the leaf's transform"
    );
    let hit_restored = model.hit_test(&page, (125.0, 125.0));
    assert_eq!(
        hit_restored.element,
        Some(ElementId::Rectangle("groupLeaf".to_string())),
        "after undo the leaf is back at its original position"
    );

    // --- UNDO the nested create: the outer group dissolves, g1 + topA
    // return to the spread root.
    model.undo().expect("undo nested create");
    let tree = model.scene_tree();
    assert!(
        group_children_ids(&tree, &outer_id).is_none(),
        "outer group gone after undoing the nested create"
    );
    assert!(
        group_children_ids(&tree, "g1").is_some(),
        "g1 back at the spread root"
    );
}

/// The Group read-side descriptor surfaces the group's own transform +
/// its content union AABB (the editor's inspector/layers panel reads
/// this).
#[test]
fn group_descriptor_reports_transform_and_union_bounds() {
    use paged_mutate::{PropertyPath, Value};
    let mut model = load_model();

    // Move g1 so it carries a non-identity own transform.
    model
        .apply_mutation(&Mutation::SetGroupTransform {
            group_id: "g1".to_string(),
            transform: Some([1.0, 0.0, 0.0, 1.0, 10.0, 20.0]),
        })
        .expect("set group transform");

    let props = model
        .element_properties(&ElementId::Group("g1".to_string()))
        .expect("group descriptor present");
    assert_eq!(props.kind, "Group");

    let transform = props
        .entries
        .iter()
        .find(|e| e.path == PropertyPath::FrameTransform)
        .and_then(|e| e.value.clone());
    assert!(
        matches!(transform, Some(Value::Transform(Some(m))) if (m[4] - 10.0).abs() < 1e-3 && (m[5] - 20.0).abs() < 1e-3),
        "group descriptor reports its own transform: {transform:?}"
    );

    let bounds = props
        .entries
        .iter()
        .find(|e| e.path == PropertyPath::FrameBounds)
        .and_then(|e| e.value.clone());
    assert!(
        matches!(bounds, Some(Value::Bounds(_))),
        "group descriptor reports a union AABB: {bounds:?}"
    );
}
