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

//! RFI C-15 — batch composability.
//!
//! Two limits forced plugins to split one authoring action across
//! several batches, so the user got several undo steps for one gesture:
//!
//! 1. `setDocumentDefaults` inside a batch applied but logged NOTHING,
//!    so the batch's undo left the triple changed and a failing sibling
//!    left it half-applied;
//! 2. a batch child could not address an id an earlier child minted
//!    (the v34 `$created` sentinel reached only the LAST creation, from
//!    two mutation kinds).
//!
//! These tests pin the capability AND the failure modes: an unresolvable
//! handle, a bind with nothing to name, and a failing child mid-batch —
//! each all-or-nothing, each naming what went wrong.

use std::io::Write;

use paged_canvas::{
    channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions, LoggedMutation,
    PageId,
};
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

/// One triangle-ish contour; `insertPath` mints a Polygon from it.
fn triangle(x: f32) -> Vec<PathAnchorSpec> {
    [[x, 0.0], [x + 30.0, 0.0], [x + 15.0, 30.0]]
        .into_iter()
        .map(|p| PathAnchorSpec {
            anchor: p,
            left: p,
            right: p,
        })
        .collect()
}

fn insert_path(x: f32) -> Mutation {
    Mutation::InsertPath {
        page_id: PageId("p1".into()),
        anchors: triangle(x),
        open: false,
        smooth: false,
    }
}

fn bind(handle: &str) -> Mutation {
    Mutation::BindCreated {
        handle: handle.into(),
    }
}

/// Address a handle. The `kind` is deliberately WRONG (`rectangle`)
/// where the insert mints a polygon: resolution replaces the whole
/// address, so a caller never has to predict the minted kind.
fn handle_ref(handle: &str) -> ElementId {
    ElementId::Rectangle(format!("$h:{handle}"))
}

fn fill(handle: &str, swatch: &str) -> Mutation {
    Mutation::SetElementProperty {
        element_id: handle_ref(handle),
        path: paged_mutate::PropertyPath::FrameFillColor,
        value: paged_mutate::Value::ColorRef(Some(swatch.into())),
    }
}

fn polygon_fill(model: &CanvasModel, self_id: &str) -> Option<String> {
    model.scene().spreads[0]
        .spread
        .polygons
        .iter()
        .find(|p| p.self_id.as_deref() == Some(self_id))
        .and_then(|p| p.fill_color.clone())
}

fn polygon_ids(model: &CanvasModel) -> Vec<String> {
    model.scene().spreads[0]
        .spread
        .polygons
        .iter()
        .filter_map(|p| p.self_id.clone())
        .collect()
}

/// The fill of the polygon whose first anchor sits at `x` — identity by
/// GEOMETRY, because the kind vec is not in insertion order.
fn fill_of_polygon_at(model: &CanvasModel, x: f32) -> Option<String> {
    model.scene().spreads[0]
        .spread
        .polygons
        .iter()
        .find(|p| {
            p.anchors
                .first()
                .is_some_and(|a| (a.anchor.0 - x).abs() < 1e-3)
        })
        .and_then(|p| p.fill_color.clone())
}

// ── Handles ────────────────────────────────────────────────────────

/// The shape every one of the four named paged.draw flows has: insert N
/// things, then paint EACH of them. Before C-15 the paint had to be a
/// second batch, because `$created` only ever named the last insert —
/// two undo steps for one command. Here it is one batch, one undo.
#[test]
fn a_handle_addresses_an_id_minted_earlier_in_the_same_batch() {
    let mut model = load();
    let before = format!("{:?}", model.scene().spreads);

    let outcome = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path(0.0),
                bind("first"),
                insert_path(100.0),
                bind("second"),
                // Paint the FIRST insert — the case `$created` cannot
                // express, because a second creation has happened since.
                fill("first", "Color/Black"),
                fill("second", "Color/Paper"),
            ],
        })
        .expect("handle batch applies");
    assert!(outcome.applied_seq > 0);

    let ids = polygon_ids(&model);
    assert_eq!(ids.len(), 2, "two polygons inserted: {ids:?}");
    assert_eq!(
        fill_of_polygon_at(&model, 0.0).as_deref(),
        Some("Color/Black"),
        "the FIRST insert took the first handle's fill"
    );
    assert_eq!(
        fill_of_polygon_at(&model, 100.0).as_deref(),
        Some("Color/Paper"),
        "the SECOND insert took the second handle's fill"
    );

    // ONE undo step for the whole thing — the point of the exercise.
    assert_eq!(model.applied_log_len(), 1);
    let after = format!("{:?}", model.scene().spreads);
    model.undo().expect("undo");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        before,
        "one undo restores everything the batch did, byte-identically"
    );
    model.redo().expect("redo");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        after,
        "redo reproduces the batch exactly (same ids, same fills)"
    );
    assert_eq!(model.applied_log_len(), 1);
}

/// A handle-using batch whose children ALL translate still collapses to
/// one `Operation::Batch` — one apply, one rebuild — rather than
/// degrading onto the per-child mixed-lane path.
#[test]
fn a_translatable_handle_batch_stays_one_operation() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![insert_path(0.0), bind("a"), fill("a", "Color/Black")],
        })
        .expect("applies");
    match model.applied_log_back().expect("log entry").kind.clone() {
        LoggedMutation::Frame(applied) => assert!(
            matches!(applied.op, paged_mutate::Operation::Batch { .. }),
            "one Operation::Batch, not a per-child composite"
        ),
        other => panic!("expected a single Frame(Batch) entry, got {other:?}"),
    }
}

/// Handles also compose the OTHER direction: several creations grouped
/// by name in the same batch (the pattern / appearance-bake shape,
/// where batch 2 existed only to wrap the inserts in a group).
#[test]
fn handles_group_several_creations_in_one_batch() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path(0.0),
                bind("a"),
                insert_path(100.0),
                bind("b"),
                Mutation::CreateGroup {
                    member_ids: vec![handle_ref("a"), handle_ref("b")],
                },
            ],
        })
        .expect("group batch applies");
    let groups = &model.scene().spreads[0].spread.groups;
    assert_eq!(groups.len(), 1, "one group wrapping both inserts");
    assert_eq!(
        groups[0].members.len(),
        2,
        "both handle-addressed members joined the group"
    );
    assert_eq!(model.applied_log_len(), 1, "still one undo step");
}

/// A text frame's handle addresses the story its insert MINTED, so
/// create-frame-then-pour-text is one batch (`insertText` has no
/// `Operation` form, so this also proves handles work on the mixed
/// lane, not only the all-translatable one).
#[test]
fn a_text_frame_handle_addresses_the_story_it_minted() {
    let mut model = load();
    let stories_before = model.scene().stories.len();

    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::InsertTextFrame {
                    page_id: PageId("p1".into()),
                    bounds: (10.0, 10.0, 200.0, 300.0),
                },
                bind("frame"),
                Mutation::InsertText {
                    story_id: "$h:frame".into(),
                    offset: 0,
                    text: "poured".into(),
                    cell: None,
                },
            ],
        })
        .expect("frame + pour applies as one batch");

    assert_eq!(model.scene().stories.len(), stories_before + 1);
    let minted = model.scene().stories.last().expect("minted story");
    let text: String = minted
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(text, "poured", "the text landed in the MINTED story");
    assert_eq!(
        model.applied_log_len(),
        1,
        "insert-frame + pour is ONE undo step"
    );

    let frames_before_undo = model.scene().spreads[0].spread.text_frames.len();
    model.undo().expect("undo");
    assert_eq!(
        model.scene().spreads[0].spread.text_frames.len(),
        frames_before_undo - 1,
        "one undo removes the frame the batch inserted"
    );
    let text_after_undo: String = model
        .scene()
        .stories
        .last()
        .expect("story")
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.as_str())
        .collect();
    assert!(
        text_after_undo.is_empty(),
        "the poured text was taken back too, got {text_after_undo:?}"
    );
    // PRE-EXISTING, not C-15: `RemoveNode`'s inverse of an
    // `InsertTextFrame` leaves the MINTED story behind (empty, unowned)
    // — the same after one batch as after the standalone mutation.
    assert_eq!(model.scene().stories.len(), stories_before + 1);

    // Redo replays the composite forward — frame back, text back, in the
    // same story — from ONE redo step.
    model.redo().expect("redo");
    assert_eq!(
        model.scene().spreads[0].spread.text_frames.len(),
        frames_before_undo
    );
    let text_after_redo: String = model
        .scene()
        .stories
        .last()
        .expect("story")
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(text_after_redo, "poured");
    assert_eq!(model.applied_log_len(), 1);
}

/// Two text frames in ONE batch, each poured through its own handle.
/// This is where the story-mint sibling of FINDING #6 surfaced: both
/// children used to mint the SAME `Story/u<n>` (the frame ids were
/// offset by the batch's mint counter, the story ids were not), so the
/// second frame silently adopted the first frame's story and both
/// pours landed in one place.
#[test]
fn two_text_frames_in_one_batch_get_distinct_stories() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::InsertTextFrame {
                    page_id: PageId("p1".into()),
                    bounds: (10.0, 10.0, 100.0, 100.0),
                },
                bind("a"),
                Mutation::InsertTextFrame {
                    page_id: PageId("p1".into()),
                    bounds: (110.0, 10.0, 200.0, 100.0),
                },
                bind("b"),
                Mutation::InsertText {
                    story_id: "$h:a".into(),
                    offset: 0,
                    text: "alpha".into(),
                    cell: None,
                },
                Mutation::InsertText {
                    story_id: "$h:b".into(),
                    offset: 0,
                    text: "beta".into(),
                    cell: None,
                },
            ],
        })
        .expect("two frames + two pours in one batch");

    let stories: Vec<(String, String)> = model
        .scene()
        .stories
        .iter()
        .map(|s| {
            (
                s.self_id.clone(),
                s.story
                    .paragraphs
                    .iter()
                    .flat_map(|p| p.runs.iter())
                    .map(|r| r.text.as_str())
                    .collect::<String>(),
            )
        })
        .collect();
    let minted: Vec<&(String, String)> = stories
        .iter()
        .filter(|(id, _)| id.starts_with("Story/u"))
        .collect();
    assert_eq!(minted.len(), 2, "two DISTINCT stories minted: {stories:?}");
    let texts: Vec<&str> = minted.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        texts.contains(&"alpha") && texts.contains(&"beta"),
        "each pour landed in its own frame's story: {stories:?}"
    );
    assert_eq!(model.applied_log_len(), 1, "still one undo step");
}

/// A handle no `bindCreated` bound fails the WHOLE batch: nothing is
/// left behind, and the error names the handle plus what IS bound —
/// never a literal `$h:` id reaching the scene.
#[test]
fn an_unresolvable_handle_fails_the_whole_batch() {
    let mut model = load();
    let before = format!("{:?}", model.scene().spreads);

    let err = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path(0.0),
                bind("a"),
                insert_path(100.0),
                fill("typo", "Color/Black"),
            ],
        })
        .expect_err("an unknown handle must fail the batch");

    let msg = format!("{err:?}");
    assert!(msg.contains("typo"), "names the missing handle: {msg}");
    assert!(msg.contains('a'), "names what IS bound: {msg}");
    assert!(msg.contains("child 3"), "names the child: {msg}");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        before,
        "both inserts were rolled back — nothing half-applied"
    );
    assert_eq!(model.applied_log_len(), 0, "nothing logged");
}

/// Binding before anything has been created names nothing, so the batch
/// fails rather than binding a stale/absent id.
#[test]
fn binding_before_any_creation_fails_the_batch() {
    let mut model = load();
    let before = format!("{:?}", model.scene().spreads);

    let err = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![bind("a"), insert_path(0.0), fill("a", "Color/Black")],
        })
        .expect_err("bind with nothing to name must fail");

    let msg = format!("{err:?}");
    assert!(msg.contains("bindCreated"), "{msg}");
    assert!(msg.contains("child 0"), "names the child: {msg}");
    assert_eq!(format!("{:?}", model.scene().spreads), before);
    assert_eq!(model.applied_log_len(), 0);
}

/// A child that fails AFTER handles have bound rolls the whole batch
/// back — the C-14 guarantee, still exact with handles in play.
#[test]
fn a_failing_child_after_a_bind_rolls_the_batch_back() {
    let mut model = load();
    let before = format!("{:?}", model.scene().spreads);

    let err = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path(0.0),
                bind("a"),
                fill("a", "Color/Black"),
                // No such story — this child cannot apply.
                Mutation::InsertText {
                    story_id: "nope".into(),
                    offset: 0,
                    text: "x".into(),
                    cell: None,
                },
            ],
        })
        .expect_err("the batch must fail");

    assert!(format!("{err:?}").contains("child 3"), "{err:?}");
    assert_eq!(
        format!("{:?}", model.scene().spreads),
        before,
        "the insert and the fill were both reversed"
    );
    assert_eq!(model.applied_log_len(), 0);
}

/// `bindCreated` is meaningless outside a batch and says so rather than
/// silently succeeding as a no-op.
#[test]
fn bind_created_outside_a_batch_is_rejected() {
    let mut model = load();
    let err = model
        .apply_mutation(&bind("a"))
        .expect_err("standalone bind is not a mutation");
    assert!(format!("{err:?}").contains("BindCreated"), "{err:?}");
}

/// A batch that binds nothing keeps the pre-C-15 behaviour exactly,
/// including the v34 `$created` sentinel.
#[test]
fn a_batch_without_handles_is_unchanged() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path(0.0),
                Mutation::SetElementProperty {
                    element_id: ElementId::Polygon("$created".into()),
                    path: paged_mutate::PropertyPath::FrameFillColor,
                    value: paged_mutate::Value::ColorRef(Some("Color/Black".into())),
                },
            ],
        })
        .expect("the v34 sentinel still resolves");
    let ids = polygon_ids(&model);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        polygon_fill(&model, &ids[0]).as_deref(),
        Some("Color/Black")
    );
    assert_eq!(model.applied_log_len(), 1);
}

// ── Document defaults inside a batch ───────────────────────────────

/// The defaults idiom plugins actually use — swap the defaults, create
/// with them, restore — as ONE batch and ONE undo step. Before C-15 the
/// defaults child logged nothing, so undo restored the geometry but
/// left the document's fill/stroke wells changed.
#[test]
fn document_defaults_inside_a_batch_undo_as_one_step() {
    let mut model = load();
    assert_eq!(model.document_meta().default_fill_color, None);

    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::SetDocumentDefaults {
                    fill_color: Some("Color/Black".into()),
                    stroke_color: None,
                    stroke_weight: Some(3.0),
                },
                Mutation::InsertFrame {
                    page_id: PageId("p1".into()),
                    bounds: (10.0, 10.0, 60.0, 60.0),
                },
            ],
        })
        .expect("defaults compose inside a batch");

    // The insert saw the defaults its own batch set …
    let rect = model.scene().spreads[0]
        .spread
        .rectangles
        .last()
        .expect("inserted rectangle");
    assert_eq!(rect.fill_color.as_deref(), Some("Color/Black"));
    assert_eq!(rect.stroke_weight, Some(3.0));
    let meta = model.document_meta();
    assert_eq!(meta.default_fill_color.as_deref(), Some("Color/Black"));
    assert_eq!(model.applied_log_len(), 1, "one batch, one undo entry");

    // … and ONE undo takes back BOTH the frame and the defaults.
    model.undo().expect("undo");
    assert_eq!(
        model.scene().spreads[0].spread.rectangles.len(),
        1,
        "the inserted rectangle is gone (only the fixture's r1 remains)"
    );
    assert_eq!(
        model.document_meta().default_fill_color,
        None,
        "the defaults triple the batch replaced is restored"
    );

    // … and redo reproduces both.
    model.redo().expect("redo");
    assert_eq!(
        model.document_meta().default_fill_color.as_deref(),
        Some("Color/Black")
    );
    assert_eq!(model.document_meta().default_stroke_weight, Some(3.0));
    assert_eq!(model.scene().spreads[0].spread.rectangles.len(), 2);
}

/// A defaults child is part of the all-or-nothing promise: a later
/// child's failure restores the triple too.
#[test]
fn document_defaults_roll_back_with_a_failing_batch() {
    let mut model = load();
    let err = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::SetDocumentDefaults {
                    fill_color: Some("Color/Black".into()),
                    stroke_color: None,
                    stroke_weight: Some(3.0),
                },
                Mutation::InsertText {
                    story_id: "nope".into(),
                    offset: 0,
                    text: "x".into(),
                    cell: None,
                },
            ],
        })
        .expect_err("the batch must fail");
    assert!(format!("{err:?}").contains("child 1"), "{err:?}");
    assert_eq!(
        model.document_meta().default_fill_color,
        None,
        "the defaults write was rolled back with everything else"
    );
    assert_eq!(model.applied_log_len(), 0);
}

/// Standalone, the defaults stay what they always were: applied, not
/// undoable, no log entry. C-15 changed composability, not the
/// single-mutation contract.
#[test]
fn standalone_document_defaults_are_still_not_undoable() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: Some("Color/Black".into()),
            stroke_color: None,
            stroke_weight: None,
        })
        .expect("set defaults");
    assert_eq!(model.applied_log_len(), 0, "no undo entry");
    assert_eq!(
        model.document_meta().default_fill_color.as_deref(),
        Some("Color/Black")
    );
}

/// Both halves at once — the full "swap defaults, create, name, style,
/// restore defaults" flow as a single undo step.
#[test]
fn defaults_and_handles_compose_in_one_batch() {
    let mut model = load();
    let before = format!("{:?}", model.scene().spreads);

    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::SetDocumentDefaults {
                    fill_color: Some("Color/Paper".into()),
                    stroke_color: None,
                    stroke_weight: Some(2.0),
                },
                insert_path(0.0),
                bind("shape"),
                Mutation::SetDocumentDefaults {
                    fill_color: None,
                    stroke_color: None,
                    stroke_weight: None,
                },
                fill("shape", "Color/Black"),
            ],
        })
        .expect("defaults + handles compose");

    let ids = polygon_ids(&model);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        polygon_fill(&model, &ids[0]).as_deref(),
        Some("Color/Black"),
        "the handle-addressed write won over the creation default"
    );
    assert_eq!(
        model.document_meta().default_fill_color,
        None,
        "the batch restored the defaults itself"
    );
    assert_eq!(model.applied_log_len(), 1);

    model.undo().expect("undo");
    assert_eq!(format!("{:?}", model.scene().spreads), before);
    assert_eq!(model.document_meta().default_fill_color, None);
}

/// RFI #72 — a `bindCreated` placed after a `createGroup` must name the
/// GROUP, and must do so whether or not an earlier handle is live.
///
/// Measured first from paged.draw's Repeats work, which routes around
/// this rather than depending on it: with an earlier bind in the batch,
/// `dissolveGroup { groupId: "$h:g" }` refused with
/// "node not found: Group(<the EARLIER insert's id>)" — the handle had
/// resolved to the previous creation, not the group.
///
/// The cause was NOT the resolver. `Mutation::CreateGroup` translates
/// with `spec.self_id: None` (the wire never names a group; the engine
/// mints it), and `created_element_id` read the id off the REQUESTED
/// spec. The applier fills `resolved.self_id` with what it minted, so
/// reading the APPLIED op is what makes the group nameable.
#[test]
fn bind_after_create_group_names_the_group_not_the_previous_insert() {
    let mut model = load();

    // Two inserts, each bound — the second bind is what used to be
    // shadowed by the group's missing id.
    let outcome = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_path(0.0),
                bind("a"),
                insert_path(50.0),
                bind("b"),
                Mutation::CreateGroup {
                    member_ids: vec![handle_ref("a"), handle_ref("b")],
                },
                bind("g"),
                // If `$h:g` resolved to insert "b" (the old behaviour)
                // this refuses with "node not found: Group(<b's id>)".
                Mutation::DissolveGroup {
                    group_id: "$h:g".into(),
                },
            ],
        })
        .expect("the group must be nameable by the bind that follows it");

    // Dissolving restored both members to the spread, so the two
    // polygons are still there and ungrouped.
    assert_eq!(
        polygon_ids(&model).len(),
        2,
        "both members survive the group/dissolve round trip"
    );
    assert!(
        model.scene().spreads[0].spread.groups.is_empty(),
        "the group this batch created was dissolved by its own handle"
    );
    assert_eq!(model.applied_log_len(), 1, "still ONE undo step");
    let _ = outcome;
}

/// The same shape with NO earlier bind — the case that already worked
/// (the group was the batch's FIRST creation, so the stale `created`
/// slot happened to be empty). Pinned so the fix cannot regress it.
#[test]
fn bind_after_create_group_works_with_no_earlier_handle() {
    let mut model = load();

    // Two inserts, NEITHER bound; group them by handle-free means by
    // binding only after the group exists.
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![insert_path(0.0), insert_path(50.0)],
        })
        .expect("two plain inserts");
    let ids = polygon_ids(&model);
    assert_eq!(ids.len(), 2);

    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::CreateGroup {
                    member_ids: vec![
                        ElementId::Polygon(ids[0].clone()),
                        ElementId::Polygon(ids[1].clone()),
                    ],
                },
                bind("g"),
                Mutation::DissolveGroup {
                    group_id: "$h:g".into(),
                },
            ],
        })
        .expect("the group is nameable as the batch's first creation too");

    assert!(
        model.scene().spreads[0].spread.groups.is_empty(),
        "created and dissolved in one batch"
    );
}

/// The id a `createGroup` mints is now chosen at TRANSLATION time. It
/// must still be the one the applier would have chosen — otherwise the
/// fix above would move every group id in the document by one.
#[test]
fn a_group_id_minted_at_translation_matches_the_applier() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![insert_path(0.0), insert_path(50.0)],
        })
        .expect("two inserts");
    let ids = polygon_ids(&model);

    model
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![
                ElementId::Polygon(ids[0].clone()),
                ElementId::Polygon(ids[1].clone()),
            ],
        })
        .expect("group");

    let group_id = model.scene().spreads[0].spread.groups[0]
        .self_id
        .clone()
        .expect("the group carries its id");
    // Two polygons at u1/u2 (plus the fixture's own items) ⇒ the group
    // takes the NEXT id in the shared page-item space, exactly as
    // `mint_group_id` would have.
    let max_before: u64 = ids
        .iter()
        .filter_map(|i| u64::from_str_radix(i.strip_prefix('u')?, 16).ok())
        .max()
        .expect("polygon ids are u<hex>");
    assert_eq!(
        group_id,
        format!("u{:x}", max_before + 1),
        "the group takes the next id in the shared space, not a private one"
    );
}

// ── One rebuild ────────────────────────────────────────────────────

/// The children of an authoring gesture, in the order a page module
/// writes them: mint a frame, name it, pour its story, paint it. Spans
/// both lanes (the pour has no `Operation` form), so it takes the mixed
/// path.
fn authoring_gesture() -> Vec<Mutation> {
    vec![
        Mutation::InsertTextFrame {
            page_id: PageId("p1".into()),
            bounds: (10.0, 10.0, 200.0, 300.0),
        },
        bind("frame"),
        Mutation::InsertText {
            story_id: "$h:frame".into(),
            offset: 0,
            text: "poured".into(),
            cell: None,
        },
        fill("frame", "Color/Black"),
    ]
}

/// What a mutation actually costs is the REBUILD — `build_document`
/// walks the whole document, so the price scales with the content
/// already on the page rather than with the edit. A mixed batch used to
/// pay it per child: measured on the annual's 134-page book, ~16 ms per
/// operation natively and ~14 s in wasm, which is why authoring it
/// through one mutation per op took fifteen hours.
///
/// One batch is one build. This is the assertion that keeps it that way.
#[test]
fn a_mixed_batch_rebuilds_once() {
    let mut model = load();
    let before = model.last_rebuild_stats().rebuilds;

    model
        .apply_mutation(&Mutation::Batch {
            ops: authoring_gesture(),
        })
        .expect("the authoring gesture applies");

    assert_eq!(
        model.last_rebuild_stats().rebuilds - before,
        1,
        "a four-child batch must build the document ONCE, not once per child",
    );
}

/// The contrast, and the equivalence in one test: the same gesture sent
/// one mutation at a time pays a build per op — and lands the document
/// in exactly the same place, scene and built pages alike. Deferring the
/// rebuild may not change what a batch produces.
#[test]
fn the_deferred_build_lands_where_the_per_op_builds_do() {
    let mut batched = load();
    let batched_before = batched.last_rebuild_stats().rebuilds;
    batched
        .apply_mutation(&Mutation::Batch {
            ops: authoring_gesture(),
        })
        .expect("batched gesture");
    let batched_builds = batched.last_rebuild_stats().rebuilds - batched_before;

    // The same children, one mutation each. No handles: the sequential
    // lane addresses the ids the engine minted, which is exactly what
    // the handle resolver substitutes inside the batch.
    let mut sequential = load();
    let sequential_before = sequential.last_rebuild_stats().rebuilds;
    sequential
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId("p1".into()),
            bounds: (10.0, 10.0, 200.0, 300.0),
        })
        .expect("frame");
    let minted = sequential.scene().spreads[0]
        .spread
        .text_frames
        .last()
        .expect("the minted frame");
    let frame_id = minted.self_id.clone().expect("frame self id");
    let story_id = minted.parent_story.clone().expect("frame's minted story");
    sequential
        .apply_mutation(&Mutation::InsertText {
            story_id,
            offset: 0,
            text: "poured".into(),
            cell: None,
        })
        .expect("pour");
    sequential
        .apply_mutation(&Mutation::SetElementProperty {
            element_id: ElementId::TextFrame(frame_id),
            path: paged_mutate::PropertyPath::FrameFillColor,
            value: paged_mutate::Value::ColorRef(Some("Color/Black".into())),
        })
        .expect("paint");
    let sequential_builds = sequential.last_rebuild_stats().rebuilds - sequential_before;

    assert_eq!(
        (batched_builds, sequential_builds),
        (1, 3),
        "one build for the batch, one per op for the sequence",
    );
    assert_eq!(
        batched.current_state_hash(),
        sequential.current_state_hash(),
        "the batched gesture must leave the SCENE where the sequence does",
    );
    assert_eq!(
        format!("{:?}", batched.built().pages),
        format!("{:?}", sequential.built().pages),
        "and the BUILT pages too — a deferred build that never ran would \
         leave the batched model displaying the document it started from",
    );
}

/// A batch that fails still owes its build: the rollback restores the
/// scene, and `built` must describe THAT scene rather than the
/// half-applied one the failing child left behind.
#[test]
fn a_rolled_back_batch_rebuilds_from_the_restored_scene() {
    let mut model = load();
    let before = format!("{:?}", model.built().pages);
    let builds_before = model.last_rebuild_stats().rebuilds;

    let err = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::InsertTextFrame {
                    page_id: PageId("p1".into()),
                    bounds: (10.0, 10.0, 200.0, 300.0),
                },
                bind("frame"),
                Mutation::InsertText {
                    story_id: "$h:frame".into(),
                    offset: 0,
                    text: "poured".into(),
                    cell: None,
                },
                // Nothing bound this one — the batch fails here.
                fill("never-bound", "Color/Black"),
            ],
        })
        .expect_err("the unresolvable handle fails the batch");
    assert!(
        format!("{err:?}").contains("never-bound"),
        "the error names the handle: {err:?}",
    );
    assert_eq!(
        format!("{:?}", model.built().pages),
        before,
        "the rolled-back batch leaves the built document where it was",
    );
    assert!(
        model.last_rebuild_stats().rebuilds > builds_before,
        "and it did rebuild — the restored scene has to reach `built`",
    );
}

// ── What a batch minted ────────────────────────────────────────────

/// A batch mints one element per creating child and `createdId` can
/// name only the last, so a caller authoring through batches could not
/// address what it had just made — it had to send one mutation per
/// element (a full rebuild each) or re-discover the ids with a scene
/// walk. `minted` is that list, in mint order, carrying the handle each
/// element was bound to.
#[test]
fn a_mixed_batch_reports_every_id_it_minted() {
    let mut model = load();
    let outcome = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::InsertTextFrame {
                    page_id: PageId("p1".into()),
                    bounds: (10.0, 10.0, 200.0, 300.0),
                },
                bind("frame"),
                Mutation::InsertText {
                    story_id: "$h:frame".into(),
                    offset: 0,
                    text: "poured".into(),
                    cell: None,
                },
                insert_path(400.0),
                bind("mark"),
            ],
        })
        .expect("gesture applies");

    let names: Vec<Option<&str>> = outcome
        .minted
        .iter()
        .map(|m| m.handle.as_deref())
        .collect();
    assert_eq!(
        names,
        vec![Some("frame"), Some("mark")],
        "both mints reported, each under the name that bound it",
    );
    let frame = &outcome.minted[0];
    assert!(
        matches!(&frame.element, ElementId::TextFrame(_)),
        "the first mint is the text frame, got {:?}",
        frame.element,
    );
    let scene_story = model.scene().spreads[0]
        .spread
        .text_frames
        .last()
        .and_then(|f| f.parent_story.clone());
    assert_eq!(
        frame.story_id, scene_story,
        "the frame's mint carries the story its insert minted — what a \
         later pour addresses",
    );
    assert_eq!(
        outcome.created_id.as_ref(),
        Some(&outcome.minted[1].element),
        "createdId still names the LAST mint, as it always did",
    );
}

/// The same on the lane that translates whole (no text child): a pure
/// geometry batch is applied as ONE `Operation::Batch`, and it reports
/// its mints in the order the children ran. Handles are resolved before
/// apply on this lane, so the names do not come back — the ORDER is the
/// contract, and it is the same order the caller wrote.
#[test]
fn a_translatable_batch_reports_its_mints_in_order() {
    let mut model = load();
    let outcome = model
        .apply_mutation(&Mutation::Batch {
            ops: vec![insert_path(10.0), bind("a"), insert_path(80.0), bind("b")],
        })
        .expect("two paths apply");

    assert_eq!(outcome.minted.len(), 2, "one entry per creating child");
    let ids: Vec<String> = outcome
        .minted
        .iter()
        .map(|m| match &m.element {
            ElementId::Polygon(id) => id.clone(),
            other => panic!("expected a polygon, got {other:?}"),
        })
        .collect();
    let by_geometry: Vec<String> = [10.0_f32, 80.0]
        .into_iter()
        .map(|x| {
            model.scene().spreads[0]
                .spread
                .polygons
                .iter()
                .find(|p| {
                    p.anchors
                        .first()
                        .is_some_and(|a| (a.anchor.0 - x).abs() < 1e-3)
                })
                .and_then(|p| p.self_id.clone())
                .expect("polygon at x")
        })
        .collect();
    assert_eq!(
        ids, by_geometry,
        "mint order matches the order the children were written",
    );
}

/// A single mutation reports nothing new: its one mint is `createdId`,
/// and a caller that reads `minted` on every reply must not see a
/// phantom second source of truth.
#[test]
fn a_single_mutation_mints_into_created_id_only() {
    let mut model = load();
    let outcome = model.apply_mutation(&insert_path(10.0)).expect("one path");
    assert!(outcome.created_id.is_some());
    assert!(
        outcome.minted.is_empty(),
        "minted is the batch's list; a single mutation leaves it empty",
    );
}
