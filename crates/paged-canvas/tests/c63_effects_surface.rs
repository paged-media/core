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

//! C-63 on the WIRE — what the element descriptor reports is what the
//! kernel will take.
//!
//! `paged-mutate/tests/effects_fanout.rs` proves the apply arms. This
//! file proves the two halves a panel actually touches agree: the READ
//! descriptor (`element_properties`) and the `setElementProperty` write,
//! with undo through the model's own history.
//!
//! * A Polygon and an Oval read the same effect inventory a Rectangle
//!   reads — derived from the Rectangle's own descriptor, so a row added
//!   there cannot be forgotten here.
//! * Each of those rows is writable on a Polygon, reads back, and undoes.
//! * A GraphicLine reads none of them (and rejects the write): its
//!   effects are stored and never drawn.
//! * A Group reads and writes its opacity and blend mode, and an
//!   ungroup-then-undo restores them.

use paged_canvas::{
    channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions, PageId,
};
use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{PropertyPath, Value};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("geometry").expect("geometry sample");
    let bytes = paged_gen::write_idml(&sample).expect("write idml");
    CanvasModel::load("c63", &bytes, CanvasOptions::default()).expect("load")
}

fn page(m: &CanvasModel) -> PageId {
    PageId(m.pages()[0].self_id.clone())
}

fn minted(m: &mut CanvasModel, mutation: Mutation) -> ElementId {
    m.apply_mutation(&mutation)
        .expect("insert")
        .created_id
        .expect("the insert reports the element it created")
}

// One of each leaf kind, minted the way its TOOL mints it — no fixture
// carries all four, and what the tools produce is what a user edits.

fn rectangle(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    minted(
        m,
        Mutation::InsertFrame {
            page_id,
            bounds: (100.0, 100.0, 260.0, 220.0),
        },
    )
}
fn oval(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    minted(
        m,
        Mutation::InsertOval {
            page_id,
            bounds: (100.0, 300.0, 260.0, 420.0),
        },
    )
}
/// The pen: a closed four-point path, which is a `Polygon`.
fn polygon(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    let corner = |x: f32, y: f32| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    };
    minted(
        m,
        Mutation::InsertPath {
            page_id,
            anchors: vec![
                corner(300.0, 100.0),
                corner(460.0, 100.0),
                corner(460.0, 220.0),
                corner(300.0, 220.0),
            ],
            open: false,
            smooth: false,
        },
    )
}
fn graphic_line(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    minted(
        m,
        Mutation::InsertLine {
            page_id,
            start: (300.0, 300.0),
            end: (460.0, 420.0),
        },
    )
}

fn read(m: &CanvasModel, id: &ElementId, path: PropertyPath) -> Option<Value> {
    m.element_properties(id)
        .expect("element answers properties")
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .and_then(|e| e.value)
}

/// Is this path part of the effect lane? Decided by NAME so the roster
/// is whatever the Rectangle descriptor says it is, not a second list.
fn is_effect_path(path: PropertyPath) -> bool {
    let name = format!("{path:?}");
    [
        "FrameInnerShadow",
        "FrameOuterGlow",
        "FrameInnerGlow",
        "FrameBevel",
        "FrameSatin",
        "FrameFeather",
        "FrameDirectionalFeather",
        "FrameGradientFeather",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// The effect rows the Rectangle descriptor reports.
fn effect_inventory(m: &mut CanvasModel) -> Vec<PropertyPath> {
    let rect = rectangle(m);
    m.element_properties(&rect)
        .expect("rectangle props")
        .entries
        .iter()
        .map(|e| e.path)
        .filter(|p| is_effect_path(*p))
        .collect()
}

#[test]
fn a_polygon_and_an_oval_read_the_effect_inventory_a_rectangle_reads() {
    let mut m = model();
    let inventory = effect_inventory(&mut m);
    // 58 per-effect rows + the gradient feather.
    assert_eq!(inventory.len(), 59, "the Rectangle's effect inventory");

    let kinds = [("Polygon", polygon(&mut m)), ("Oval", oval(&mut m))];
    for (kind, id) in kinds {
        let rows: Vec<PropertyPath> = m
            .element_properties(&id)
            .expect("props")
            .entries
            .iter()
            .map(|e| e.path)
            .collect();
        for path in &inventory {
            assert_eq!(
                rows.iter().filter(|p| *p == path).count(),
                1,
                "{kind} must read {path:?} exactly once"
            );
        }
        assert_eq!(
            rows.iter()
                .filter(|p| **p == PropertyPath::FrameBlendMode)
                .count(),
            1,
            "{kind} reads its blend mode once (it moved into the shared builder)"
        );
    }

    // A line stores the bag and nothing draws it: no rows, no arms.
    let line = graphic_line(&mut m);
    let line_rows: Vec<PropertyPath> = m
        .element_properties(&line)
        .expect("line props")
        .entries
        .iter()
        .map(|e| e.path)
        .collect();
    assert!(
        !line_rows.iter().any(|p| is_effect_path(*p)),
        "a GraphicLine must not advertise an effect it cannot draw"
    );
}

/// A value of the same type as the row's current one, and different
/// from it — so the write is type-correct for every row in the roster
/// without a second hand-written table.
fn changed(current: &Value) -> Value {
    match current {
        Value::Bool(b) => Value::Bool(!b),
        Value::Length(_) => Value::Length(Some(13.5)),
        Value::Text(_) => Value::Text("C63".to_string()),
        Value::ColorRef(_) => Value::ColorRef(Some("Color/C63Probe".to_string())),
        Value::GradientFeather(_) => {
            Value::GradientFeather(Some(paged_mutate::operation::GradientFeatherSpec {
                gradient_type: Some("Linear".into()),
                start_point: Some([0.0, 0.0]),
                end_point: Some([100.0, 0.0]),
                angle_deg: Some(0.0),
                stops: vec![
                    paged_mutate::operation::GradientFeatherStopSpec {
                        stop_color: None,
                        location_pct: 0.0,
                        alpha_pct: 100.0,
                        midpoint_pct: 50.0,
                    },
                    paged_mutate::operation::GradientFeatherStopSpec {
                        stop_color: None,
                        location_pct: 100.0,
                        alpha_pct: 0.0,
                        midpoint_pct: 50.0,
                    },
                ],
            }))
        }
        other => panic!("an effect row carries an unexpected value shape: {other:?}"),
    }
}

#[test]
fn every_effect_row_writes_reads_back_and_undoes_on_a_polygon() {
    let inventory = effect_inventory(&mut model());
    for path in inventory {
        let mut m = model();
        let id = polygon(&mut m);
        // The effect's block is switched on first when the row is one of
        // its FIELDS: writing a field into an absent effect materialises
        // the preset block, and undo restores the field, not the absence.
        let name = format!("{path:?}");
        if !name.ends_with("Enabled") && path != PropertyPath::FrameGradientFeather {
            let family = [
                ("FrameInnerShadow", PropertyPath::FrameInnerShadowEnabled),
                ("FrameOuterGlow", PropertyPath::FrameOuterGlowEnabled),
                ("FrameInnerGlow", PropertyPath::FrameInnerGlowEnabled),
                ("FrameBevel", PropertyPath::FrameBevelEnabled),
                ("FrameSatin", PropertyPath::FrameSatinEnabled),
                (
                    "FrameDirectionalFeather",
                    PropertyPath::FrameDirectionalFeatherEnabled,
                ),
                ("FrameFeather", PropertyPath::FrameFeatherEnabled),
            ]
            .iter()
            .find(|(prefix, _)| name.starts_with(prefix))
            .map(|(_, toggle)| *toggle)
            .expect("every effect row belongs to a family");
            m.apply_mutation(&Mutation::SetElementProperty {
                element_id: id.clone(),
                path: family,
                value: Value::Bool(true),
            })
            .unwrap_or_else(|e| panic!("Polygon rejected {family:?}: {e:?}"));
        }

        let before = read(&m, &id, path).unwrap_or_else(|| panic!("Polygon reads {path:?}"));
        let value = changed(&before);
        m.apply_mutation(&Mutation::SetElementProperty {
            element_id: id.clone(),
            path,
            value: value.clone(),
        })
        .unwrap_or_else(|e| panic!("Polygon rejected {path:?}: {e:?}"));
        assert_eq!(
            read(&m, &id, path),
            Some(value),
            "{path:?} did not read back what was written"
        );
        assert!(m.undo().is_some(), "{path:?}: undo produced no outcome");
        assert_eq!(
            read(&m, &id, path),
            Some(before),
            "{path:?}: undo did not restore the prior value"
        );
    }
}

#[test]
fn a_graphic_line_rejects_the_effect_write() {
    let mut m = model();
    let id = graphic_line(&mut m);
    let err = m
        .apply_mutation(&Mutation::SetElementProperty {
            element_id: id,
            path: PropertyPath::FrameOuterGlowEnabled,
            value: Value::Bool(true),
        })
        .expect_err("a line's effects are stored and never drawn");
    assert!(
        format!("{err:?}").contains("is not supported on"),
        "the rejection names the unsupported pair: {err:?}"
    );
}

// ── Group ───────────────────────────────────────────────────────────

fn group_of_two(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    let mut members = Vec::new();
    for bounds in [(100.0, 100.0, 300.0, 300.0), (150.0, 150.0, 350.0, 350.0)] {
        members.push(minted(
            m,
            Mutation::InsertFrame {
                page_id: page_id.clone(),
                bounds,
            },
        ));
    }
    minted(
        m,
        Mutation::CreateGroup {
            member_ids: members,
        },
    )
}

#[test]
fn a_group_reads_writes_and_undoes_its_opacity_and_blend_mode() {
    let mut m = model();
    let id = group_of_two(&mut m);
    assert_eq!(
        read(&m, &id, PropertyPath::FrameOpacity),
        Some(Value::Length(None)),
        "a fresh group reads its (absent) opacity"
    );
    assert_eq!(
        read(&m, &id, PropertyPath::FrameBlendMode),
        Some(Value::Text(String::new()))
    );

    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameOpacity,
        value: Value::Length(Some(40.0)),
    })
    .expect("a Group takes FrameOpacity");
    assert_eq!(
        read(&m, &id, PropertyPath::FrameOpacity),
        Some(Value::Length(Some(40.0)))
    );
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameBlendMode,
        value: Value::Text("Multiply".into()),
    })
    .expect("a Group takes FrameBlendMode");
    assert_eq!(
        read(&m, &id, PropertyPath::FrameBlendMode),
        Some(Value::Text("Multiply".into()))
    );

    assert!(m.undo().is_some());
    assert_eq!(
        read(&m, &id, PropertyPath::FrameBlendMode),
        Some(Value::Text(String::new())),
        "undo clears the blend mode"
    );
    assert!(m.undo().is_some());
    assert_eq!(
        read(&m, &id, PropertyPath::FrameOpacity),
        Some(Value::Length(None)),
        "undo clears the opacity"
    );
}

#[test]
fn undoing_an_ungroup_brings_the_groups_opacity_back() {
    let mut m = model();
    let id = group_of_two(&mut m);
    let ElementId::Group(group_id) = id.clone() else {
        panic!("expected a Group, got {id:?}");
    };
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameOpacity,
        value: Value::Length(Some(40.0)),
    })
    .expect("opacity");
    m.apply_mutation(&Mutation::DissolveGroup { group_id })
        .expect("ungroup");
    assert!(
        m.element_properties(&id).is_none(),
        "the group is gone after the ungroup"
    );
    assert!(m.undo().is_some(), "undo the ungroup");
    assert_eq!(
        read(&m, &id, PropertyPath::FrameOpacity),
        Some(Value::Length(Some(40.0))),
        "the re-created group carries the opacity it had"
    );
}
