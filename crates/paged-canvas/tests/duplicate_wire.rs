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

//! C-64 on the WIRE — `duplicateElements { elementIds, offset }`.
//!
//! `paged-mutate/tests/duplicate_nodes.rs` proves the clone itself. This
//! file proves what a host sees: the wire spelling, the reply (the new
//! ids in `minted`, the last of them as `createdId`), that the page
//! repaints, that it is ONE undo step through the model's own history
//! (and redo brings back the same ids), that a refusal says why, and
//! that the ids a duplicate mints do not collide with another insert's
//! in the same batch.

use paged_canvas::{
    channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions, PageId,
};
use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{PropertyPath, Value};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("geometry").expect("geometry sample");
    let bytes = paged_gen::write_idml(&sample).expect("write idml");
    CanvasModel::load("c64", &bytes, CanvasOptions::default()).expect("load")
}

fn page(m: &CanvasModel) -> PageId {
    PageId(m.pages()[0].self_id.clone())
}

fn digests(m: &CanvasModel) -> Vec<u64> {
    m.built().pages.iter().map(|p| p.list.digest()).collect()
}

fn corner(x: f32, y: f32) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    }
}

fn insert_path() -> impl Fn(PageId) -> Mutation {
    |page_id| Mutation::InsertPath {
        page_id,
        anchors: vec![
            corner(100.0, 100.0),
            corner(260.0, 100.0),
            corner(260.0, 220.0),
            corner(100.0, 220.0),
        ],
        open: false,
        smooth: false,
    }
}

/// A filled pen path; returns its id.
fn pen_path(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    let id = m
        .apply_mutation(&insert_path()(page_id))
        .expect("insert path")
        .created_id
        .expect("created id");
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameFillColor,
        value: Value::ColorRef(Some("Color/Black".to_string())),
    })
    .expect("fill");
    id
}

fn duplicate(ids: &[ElementId], offset: (f32, f32)) -> Mutation {
    Mutation::DuplicateElements {
        element_ids: ids.to_vec(),
        offset,
    }
}

fn read(m: &CanvasModel, id: &ElementId, path: PropertyPath) -> Option<Value> {
    m.element_properties(id)?
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .and_then(|e| e.value)
}

/// The wire spelling: camelCase tag, `elementIds`, `offset` as a pair.
#[test]
fn the_wire_shape_is_duplicate_elements_with_element_ids_and_an_offset_pair() {
    let m: Mutation = serde_json::from_str(
        r#"{"op":"duplicateElements","args":{"elementIds":[{"kind":"polygon","id":"u1"}],"offset":[10.5,-20]}}"#,
    )
    .expect("the documented shape deserialises");
    match &m {
        Mutation::DuplicateElements {
            element_ids,
            offset,
        } => {
            assert_eq!(element_ids, &vec![ElementId::Polygon("u1".into())]);
            assert_eq!(*offset, (10.5, -20.0));
        }
        other => panic!("decoded as {other:?}"),
    }
    assert_eq!(m.wire_tag(), "duplicateElements");
    assert_eq!(
        serde_json::to_value(&m).expect("encodes")["args"]["offset"],
        serde_json::json!([10.5, -20.0])
    );
}

#[test]
fn a_duplicate_reports_its_clones_repaints_and_is_one_undo_step() {
    let mut m = model();
    let source = pen_path(&mut m);
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: source.clone(),
        path: PropertyPath::FrameOpacity,
        value: Value::Length(Some(40.0)),
    })
    .expect("opacity");
    let before = digests(&m);
    let log_before = m.applied_log_len();

    let out = m
        .apply_mutation(&duplicate(std::slice::from_ref(&source), (30.0, 20.0)))
        .expect("duplicate");
    assert_eq!(out.minted.len(), 1, "one clone per source");
    let clone = out.minted[0].element.clone();
    assert!(matches!(clone, ElementId::Polygon(_)), "{clone:?}");
    assert_ne!(clone, source);
    assert_eq!(
        out.created_id.as_ref(),
        Some(&clone),
        "createdId is the clone"
    );
    assert_eq!(m.applied_log_len(), log_before + 1, "one undo step");
    assert_ne!(digests(&m), before, "the clone is painted");
    // The whole item came across, not a bare path.
    assert_eq!(
        read(&m, &clone, PropertyPath::FrameOpacity),
        Some(Value::Length(Some(40.0)))
    );
    assert_eq!(
        read(&m, &clone, PropertyPath::FrameFillColor),
        Some(Value::ColorRef(Some("Color/Black".to_string())))
    );
    // …moved by the offset, through its transform.
    let Some(Value::Transform(Some(t))) = read(&m, &clone, PropertyPath::FrameTransform) else {
        panic!("the clone carries a transform");
    };
    let source_t = match read(&m, &source, PropertyPath::FrameTransform) {
        Some(Value::Transform(Some(t))) => (t[4], t[5]),
        _ => (0.0, 0.0),
    };
    assert_eq!((t[4] - source_t.0, t[5] - source_t.1), (30.0, 20.0));

    let painted = digests(&m);
    assert!(m.undo().is_some(), "undo");
    assert!(m.element_properties(&clone).is_none(), "the clone is gone");
    assert!(m.element_properties(&source).is_some(), "the source stays");
    assert_eq!(digests(&m), before, "and the page is as it was");

    assert!(m.redo().is_some(), "redo");
    assert_eq!(
        read(&m, &clone, PropertyPath::FrameOpacity),
        Some(Value::Length(Some(40.0))),
        "redo brings the clone back under the SAME id, whole"
    );
    assert_eq!(digests(&m), painted);
}

#[test]
fn a_group_duplicates_as_one_minted_element_with_its_own_members() {
    let mut m = model();
    let page_id = page(&m);
    let mut members = Vec::new();
    for bounds in [(100.0, 100.0, 300.0, 300.0), (150.0, 150.0, 350.0, 350.0)] {
        members.push(
            m.apply_mutation(&Mutation::InsertFrame {
                page_id: page_id.clone(),
                bounds,
            })
            .expect("insert frame")
            .created_id
            .expect("created id"),
        );
    }
    let group = m
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: members.clone(),
        })
        .expect("group")
        .created_id
        .expect("group id");
    let rects_before: usize = m
        .scene()
        .spreads
        .iter()
        .map(|s| s.spread.rectangles.len())
        .sum();

    let out = m
        .apply_mutation(&duplicate(std::slice::from_ref(&group), (0.0, 50.0)))
        .expect("duplicate a group");
    assert_eq!(out.minted.len(), 1, "the group's clone, not its members");
    let clone = out.minted[0].element.clone();
    assert!(matches!(clone, ElementId::Group(_)), "{clone:?}");
    let rects: usize = m
        .scene()
        .spreads
        .iter()
        .map(|s| s.spread.rectangles.len())
        .sum();
    assert_eq!(rects, rects_before + 2, "both members were cloned");
    // The clone ungroups into ITS members, not the source's.
    let ElementId::Group(clone_id) = clone.clone() else {
        unreachable!()
    };
    m.apply_mutation(&Mutation::DissolveGroup { group_id: clone_id })
        .expect("ungroup the clone");
    assert!(
        m.element_properties(&group).is_some(),
        "the source group is untouched"
    );
    for member in &members {
        assert!(m.element_properties(member).is_some());
    }
}

#[test]
fn a_text_frame_clone_reports_its_own_story() {
    let mut m = model();
    let page_id = page(&m);
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id,
            bounds: (100.0, 100.0, 200.0, 300.0),
        })
        .expect("insert text frame");
    let frame = out.created_id.expect("frame id");
    let story = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(frame.raw_id()))
        .and_then(|f| f.parent_story.clone())
        .expect("the frame's story");
    m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 0,
        text: "duplicated".into(),
        cell: None,
    })
    .expect("pour text");

    let out = m
        .apply_mutation(&duplicate(std::slice::from_ref(&frame), (0.0, 120.0)))
        .expect("a plain text frame duplicates");
    let clone_story = out.minted[0]
        .story_id
        .clone()
        .expect("a text frame's clone reports its story");
    assert_ne!(clone_story, story, "its OWN story, not a second head frame");
    let text_of = |id: &str| -> String {
        m.scene()
            .stories
            .iter()
            .find(|s| s.self_id == id)
            .expect("story")
            .story
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter().map(|r| r.text.as_str()))
            .collect()
    };
    assert_eq!(text_of(&clone_story), text_of(&story));
    assert_eq!(text_of(&story), "duplicated");
}

#[test]
fn a_refusal_names_its_reason() {
    let mut m = model();
    let page_id = page(&m);
    // Two frames, threaded.
    let mut frames = Vec::new();
    for bounds in [(100.0, 100.0, 200.0, 300.0), (220.0, 100.0, 320.0, 300.0)] {
        frames.push(
            m.apply_mutation(&Mutation::InsertTextFrame {
                page_id: page_id.clone(),
                bounds,
            })
            .expect("insert text frame")
            .created_id
            .expect("frame id"),
        );
    }
    m.apply_mutation(&Mutation::LinkFrames {
        from: frames[0].raw_id().to_string(),
        to: frames[1].raw_id().to_string(),
    })
    .expect("link");
    let before = digests(&m);
    let log_before = m.applied_log_len();

    let err = m
        .apply_mutation(&duplicate(&frames[..1], (10.0, 10.0)))
        .expect_err("a threaded frame is refused");
    assert!(format!("{err:?}").contains("threaded"), "{err:?}");
    let err = m
        .apply_mutation(&duplicate(
            &[ElementId::Polygon("no-such-path".into())],
            (10.0, 10.0),
        ))
        .expect_err("an unknown element is refused");
    assert!(format!("{err:?}").contains("node not found"), "{err:?}");
    assert_eq!(digests(&m), before, "a refusal paints nothing");
    assert_eq!(m.applied_log_len(), log_before, "and logs nothing");
}

/// A duplicate inside a batch shares the batch's id counter: the clone
/// and a frame inserted AFTER it must not be handed the same id, and
/// the reply lists both mints in order.
#[test]
fn a_duplicate_and_an_insert_in_one_batch_mint_distinct_ids() {
    let mut m = model();
    let source = pen_path(&mut m);
    let page_id = page(&m);
    let log_before = m.applied_log_len();
    let out = m
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                duplicate(std::slice::from_ref(&source), (20.0, 20.0)),
                Mutation::InsertFrame {
                    page_id,
                    bounds: (300.0, 300.0, 360.0, 380.0),
                },
            ],
        })
        .expect("batch");
    let minted: Vec<ElementId> = out.minted.iter().map(|e| e.element.clone()).collect();
    assert_eq!(minted.len(), 2, "{minted:?}");
    assert!(matches!(minted[0], ElementId::Polygon(_)));
    assert!(matches!(minted[1], ElementId::Rectangle(_)));
    assert_ne!(minted[0].raw_id(), minted[1].raw_id());
    assert_ne!(minted[0], source);
    assert_eq!(m.applied_log_len(), log_before + 1, "the batch is one step");
    assert!(m.undo().is_some());
    for id in &minted {
        assert!(
            m.element_properties(id).is_none(),
            "{id:?} is gone after undo"
        );
    }
}

/// The source may be minted by an EARLIER child of the same batch and
/// named by handle: the clone is of the thing just drawn.
#[test]
fn a_batch_can_duplicate_what_it_just_inserted() {
    let mut m = model();
    let page_id = page(&m);
    let out = m
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path()(page_id),
                Mutation::BindCreated {
                    handle: "path".into(),
                },
                Mutation::DuplicateElements {
                    // The placeholder's kind is discarded on resolution.
                    element_ids: vec![ElementId::Polygon("$h:path".into())],
                    offset: (15.0, 15.0),
                },
            ],
        })
        .expect("insert then duplicate, in one batch");
    let minted: Vec<ElementId> = out.minted.iter().map(|e| e.element.clone()).collect();
    assert_eq!(minted.len(), 2, "the path and its clone: {minted:?}");
    assert_ne!(minted[0], minted[1]);
    for id in &minted {
        assert!(matches!(id, ElementId::Polygon(_)), "{id:?}");
        assert!(m.element_properties(id).is_some());
    }
    assert!(m.undo().is_some(), "one undo step");
    for id in &minted {
        assert!(m.element_properties(id).is_none());
    }
}
