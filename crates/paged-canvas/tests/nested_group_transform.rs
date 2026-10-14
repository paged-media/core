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

//! A nested group's transform is stored relative to the group holding it,
//! as IDML stores it, and moving the outer group leaves it alone.
//!
//! `setGroupTransform` used to rebase a nested group's STORED transform by
//! the outer group's spread-space delta, as if it were a leaf's composed
//! one: moving the outer group doubled the nested group's translation and
//! rotating it turned the nested group twice. The leaves landed right
//! (theirs is composed), so nothing showed until a group's own box was
//! drawn. The rotate gesture, duplicate and ungroup had the same mix-up.

use std::io::Write;

use paged_canvas::channel::{ElementGeometryItem, Mutation};
use paged_canvas::element_selection::ElementId;
use paged_canvas::gesture::{GestureAnchor, GestureType};
use paged_canvas::{CanvasModel, CanvasOptions, GestureModifiers, PageId};

type M = [f32; 6];
const I: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(a: M, b: M) -> M {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

fn rot(deg: f32, tx: f32, ty: f32) -> M {
    let (s, c) = deg.to_radians().sin_cos();
    [c, s, -s, c, tx, ty]
}

fn near(a: M, b: M, what: &str) {
    near_within(a, b, 1e-3, what);
}

fn near_within(a: M, b: M, tol: f32, what: &str) {
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < tol, "{what}: {a:?} vs {b:?}");
    }
}

fn idml(spread_body: &str) -> Vec<u8> {
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
{spread_body}
</Spread></idPkg:Spread>"#
    );
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
<Document DOMVersion="13.1" Self="d1">
<Layer Self="layer-body" Name="Body" Visible="true" Locked="false"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// `go` (translated to (100, 100)) holds the leaf `lo` and the group `gi`,
/// rotated 30° and placed at (50, 0) inside `go`, which holds `li`.
fn nested() -> CanvasModel {
    let [c, s, ..] = rot(30.0, 0.0, 0.0);
    let body = format!(
        r#"<Group Self="go" ItemTransform="1 0 0 1 100 100">
  <Rectangle Self="lo" GeometricBounds="0 0 20 20" FillColor="Color/Black"/>
  <Group Self="gi" ItemTransform="{c} {s} {} {c} 50 0">
    <Rectangle Self="li" GeometricBounds="0 0 20 40" FillColor="Color/Black"/>
  </Group>
</Group>"#,
        -s
    );
    CanvasModel::load("nested", &idml(&body), CanvasOptions::default()).expect("load")
}

const GO: M = [1.0, 0.0, 0.0, 1.0, 100.0, 100.0];

fn gi_raw() -> M {
    rot(30.0, 50.0, 0.0)
}

fn stored(m: &CanvasModel, id: &str) -> Option<M> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|p| p.spread.groups.iter())
        .find(|g| g.self_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("group {id}"))
        .item_transform
}

fn leaf(m: &CanvasModel, id: &str) -> M {
    m.scene()
        .spreads
        .iter()
        .flat_map(|p| p.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("rectangle {id}"))
        .item_transform
        .unwrap_or(I)
}

fn geometry(m: &CanvasModel, id: &str) -> ElementGeometryItem {
    m.element_geometry(&[ElementId::Group(id.to_string())])
        .pop()
        .unwrap_or_else(|| panic!("group {id} answers geometry"))
}

fn set_group(m: &mut CanvasModel, id: &str, t: M) {
    m.apply_mutation(&Mutation::SetGroupTransform {
        group_id: id.to_string(),
        transform: Some(t),
    })
    .expect("setGroupTransform");
}

#[test]
fn the_fixture_places_the_nested_group_in_its_parent() {
    let m = nested();
    assert_eq!(stored(&m, "go"), Some(GO));
    near(stored(&m, "gi").unwrap(), gi_raw(), "gi stored");
    near(leaf(&m, "li"), mul(GO, gi_raw()), "li composed");
    near(
        geometry(&m, "gi").item_transform.unwrap(),
        mul(GO, gi_raw()),
        "gi box",
    );
}

#[test]
fn moving_the_outer_group_leaves_the_nested_group_where_it_is_inside() {
    let mut m = nested();
    let gi_before = stored(&m, "gi");
    let box_before = geometry(&m, "gi");
    let li_before = leaf(&m, "li");
    set_group(&mut m, "go", [1.0, 0.0, 0.0, 1.0, 110.0, 105.0]);

    assert_eq!(
        stored(&m, "gi"),
        gi_before,
        "the nested group keeps its own"
    );
    let shift = |t: M| [t[0], t[1], t[2], t[3], t[4] + 10.0, t[5] + 5.0];
    near(
        leaf(&m, "li"),
        shift(li_before),
        "the leaf moves by the step",
    );
    near(
        leaf(&m, "lo"),
        shift(GO),
        "the outer leaf moves by the step",
    );
    let box_after = geometry(&m, "gi");
    near(
        box_after.item_transform.unwrap(),
        shift(box_before.item_transform.unwrap()),
        "the nested box moves by the step",
    );
    for (a, b) in box_after.bounds.iter().zip(box_before.bounds) {
        assert!((a - b).abs() < 1e-3, "the nested box keeps its size");
    }
}

#[test]
fn rotating_the_outer_group_turns_the_nested_group_once() {
    let mut m = nested();
    let go_new = rot(90.0, 100.0, 100.0);
    set_group(&mut m, "go", go_new);

    near(stored(&m, "gi").unwrap(), gi_raw(), "gi keeps its own");
    // The leaves stay rigid with their groups: each is its groups'
    // transforms composed (neither carries one of its own).
    near(leaf(&m, "lo"), go_new, "lo");
    near(leaf(&m, "li"), mul(go_new, gi_raw()), "li");
    // The nested box turns with the outer group: 90° + 30°, not 90° twice.
    let t = geometry(&m, "gi").item_transform.unwrap();
    near(t, mul(go_new, gi_raw()), "gi box");
    let r120 = rot(120.0, 0.0, 0.0);
    near(
        [t[0], t[1], t[2], t[3], 0.0, 0.0],
        r120,
        "gi box is at 120°",
    );
    // And the outer box at 90°.
    let o = geometry(&m, "go").item_transform.unwrap();
    near(
        [o[0], o[1], o[2], o[3], 0.0, 0.0],
        rot(90.0, 0.0, 0.0),
        "go box",
    );

    // Undo puts both groups and both leaves back.
    m.undo().expect("undo");
    assert_eq!(stored(&m, "go"), Some(GO));
    near(stored(&m, "gi").unwrap(), gi_raw(), "gi after undo");
    near(leaf(&m, "li"), mul(GO, gi_raw()), "li after undo");
}

/// The transform is the group's own, relative to its parent: a nested
/// group moved by `(10, 0)` in a parent turned 90° moves by `(0, 10)` on
/// the spread, along its parent's axes, exactly where the same write
/// lands in IDML.
#[test]
fn a_nested_groups_transform_is_relative_to_its_parent() {
    let mut m = nested();
    let go_new = rot(90.0, 100.0, 100.0);
    set_group(&mut m, "go", go_new);
    let li_before = leaf(&m, "li");
    let mut moved = gi_raw();
    moved[4] += 10.0;
    set_group(&mut m, "gi", moved);
    let li = leaf(&m, "li");
    near(
        li,
        [
            li_before[0],
            li_before[1],
            li_before[2],
            li_before[3],
            li_before[4],
            li_before[5] + 10.0,
        ],
        "li moves along the parent's axes",
    );
    near(li, mul(go_new, moved), "li is its groups composed");
    assert_eq!(stored(&m, "go"), Some(go_new), "the parent stays");
}

#[test]
fn the_rotate_gesture_turns_the_nested_group_once() {
    let mut m = nested();
    let anchor = GestureAnchor {
        page_id: PageId("p1".to_string()),
        point_in_page: (200.0, 100.0),
        pivot_in_page: Some((100.0, 100.0)),
    };
    let handle = m
        .begin_gesture(
            vec![ElementId::Group("go".to_string())],
            GestureType::Rotate,
            Some(anchor),
        )
        .expect("begin rotate");
    m.update_gesture(
        handle,
        (-100.0, 100.0),
        GestureModifiers {
            shift: false,
            alt: false,
            disable_snap: false,
        },
    )
    .expect("update");
    m.commit_gesture(handle).expect("commit");

    let go_new = stored(&m, "go").expect("go turned");
    assert!(go_new[1].abs() > 0.5, "the outer group turned: {go_new:?}");
    near(stored(&m, "gi").unwrap(), gi_raw(), "gi keeps its own");
    near(
        leaf(&m, "li"),
        mul(go_new, gi_raw()),
        "li rigid with its groups",
    );
    near(leaf(&m, "lo"), go_new, "lo rigid with go");
}

#[test]
fn a_duplicated_group_moves_its_own_transform_not_its_nested_ones() {
    let mut m = nested();
    let out = m
        .apply_mutation(&Mutation::DuplicateElements {
            element_ids: vec![ElementId::Group("go".to_string())],
            offset: (30.0, 0.0),
        })
        .expect("duplicate");
    let copy_go = match out.created_id {
        Some(ElementId::Group(id)) => id,
        other => panic!("expected the copied group, got {other:?}"),
    };
    // The copy's nested group: the group among the copy's members.
    let copy_gi = {
        let spread = &m.scene().spreads[0].spread;
        let g = spread
            .groups
            .iter()
            .find(|g| g.self_id.as_deref() == Some(copy_go.as_str()))
            .expect("the copy");
        g.members
            .iter()
            .find_map(|r| match r {
                paged_model::FrameRef::Group(i) => spread.groups[*i].self_id.clone(),
                _ => None,
            })
            .expect("the copy holds a group")
    };
    assert_ne!(copy_gi, "gi");
    let (copy_go, copy_gi) = (&copy_go, &copy_gi);
    assert_eq!(
        stored(&m, copy_go),
        Some([1.0, 0.0, 0.0, 1.0, 130.0, 100.0])
    );
    near(stored(&m, copy_gi).unwrap(), gi_raw(), "the nested copy");
    let t = geometry(&m, copy_gi).item_transform.unwrap();
    let mut want = mul(GO, gi_raw());
    want[4] += 30.0;
    near(
        t,
        want,
        "the nested copy's box sits 30 pt right of the original's",
    );
}

#[test]
fn ungrouping_keeps_the_nested_groups_frame_and_undo_restores_it() {
    let mut m = nested();
    let before = geometry(&m, "gi");
    m.apply_mutation(&Mutation::DissolveGroup {
        group_id: "go".to_string(),
    })
    .expect("dissolve");
    near(
        stored(&m, "gi").unwrap(),
        mul(GO, gi_raw()),
        "gi now sits on the spread",
    );
    let after = geometry(&m, "gi");
    near(
        after.item_transform.unwrap(),
        before.item_transform.unwrap(),
        "the box stays",
    );
    m.undo().expect("undo");
    near(
        stored(&m, "gi").unwrap(),
        gi_raw(),
        "gi relative to go again",
    );
    near(
        geometry(&m, "gi").item_transform.unwrap(),
        before.item_transform.unwrap(),
        "the box after undo",
    );
}

/// Groups made in the editor and turned there save to IDML with the
/// nested group where it was drawn, and reopen that way.
#[test]
fn a_turned_nested_group_reopens_where_it_was_drawn() {
    let body = r#"<Rectangle Self="a" GeometricBounds="0 0 20 20" ItemTransform="1 0 0 1 100 100" FillColor="Color/Black"/>
<Rectangle Self="b" GeometricBounds="0 0 20 20" ItemTransform="1 0 0 1 140 100" FillColor="Color/Black"/>
<Rectangle Self="c" GeometricBounds="0 0 20 20" ItemTransform="1 0 0 1 100 160" FillColor="Color/Black"/>"#;
    let mut m = CanvasModel::load("made", &idml(body), CanvasOptions::default()).expect("load");
    let group_of = |out: paged_canvas::model::MutationOutcome| match out.created_id {
        Some(ElementId::Group(id)) => id,
        other => panic!("expected a group, got {other:?}"),
    };
    let inner = group_of(
        m.apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![
                ElementId::Rectangle("a".to_string()),
                ElementId::Rectangle("b".to_string()),
            ],
        })
        .expect("group a b"),
    );
    set_group(&mut m, &inner, rot(30.0, 10.0, 0.0));
    let outer = group_of(
        m.apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![
                ElementId::Group(inner.clone()),
                ElementId::Rectangle("c".to_string()),
            ],
        })
        .expect("group inner c"),
    );
    set_group(&mut m, &outer, rot(90.0, 400.0, 0.0));

    let expect_inner = mul(rot(90.0, 400.0, 0.0), rot(30.0, 10.0, 0.0));
    near(
        geometry(&m, &inner).item_transform.unwrap(),
        expect_inner,
        "inner box in session",
    );
    let leaves: Vec<M> = ["a", "b", "c"].iter().map(|id| leaf(&m, id)).collect();

    // IDML spells a matrix to a few decimals, so the reopened values
    // agree to that precision.
    let bytes = m.export_idml().expect("export");
    let back = CanvasModel::load("reopened", &bytes, CanvasOptions::default()).expect("reopen");
    near_within(
        geometry(&back, &inner).item_transform.unwrap(),
        expect_inner,
        0.01,
        "inner box after reopening",
    );
    near_within(
        geometry(&back, &outer).item_transform.unwrap(),
        rot(90.0, 400.0, 0.0),
        0.01,
        "outer box after reopening",
    );
    for (id, before) in ["a", "b", "c"].iter().zip(leaves) {
        near_within(leaf(&back, id), before, 0.01, id);
    }
}
