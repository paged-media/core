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

//! v59 — `Operation::ReorderNode` (Arrange) INV suite.
//!
//! The three properties the op has to hold:
//!
//!   * **It paints.** A reorder is only real if the display-list
//!     command stream changes — every order assertion here is paired
//!     with a read of what the renderer actually emits, so a
//!     bookkeeping-only reorder (one that permuted a vec nothing walks)
//!     fails. That is the trap `MoveNode` fell into: it permutes the
//!     KIND vec, which the renderer ignores whenever `frames_in_order`
//!     is populated, and its forward path re-registers the node with
//!     `z_slot: None` — i.e. on top — whatever `position` said.
//!   * **The inverse is exact.** Whatever verb went in, one inverse
//!     restores the previous order AND the previous paint stream
//!     bytewise.
//!   * **It cannot smuggle.** A grouped item reorders inside its
//!     group, a B-18 pasted-in child inside its container, and neither
//!     can reach the other's list or the spread's.
//!
//! The fixture is built in memory rather than taken from
//! `corpus/generated`: every page item gets a DISTINCT size, so the
//! `FillPath` transforms read back as an unambiguous paint order, and
//! nothing carries an `ItemLayer` — the renderer sorts `frames_in_order`
//! by layer first (Q-10), which would otherwise mask a z change (see
//! `a_layer_sort_outranks_arrange`, which pins exactly that limit).

use std::io::Write;

use paged_model::FrameRef;
use paged_mutate::{apply, NodeId, Operation, OperationError, ZOrderTarget};
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One page, one layer. Top level (back → front): `a` (10pt), `b`
/// (20pt), `c` (30pt), a `<Group>` of `g1` (40pt) + `g2` (50pt), and
/// the paste-into container `host` (200pt) holding `n1` (60pt) + `n2`
/// (70pt). Sizes are unique so the emitted `FillPath` transforms name
/// the item that painted.
fn arrange_idml_spread_xml() -> Vec<u8> {
    br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1" ItemTransform="1 0 0 1 0 0">
    <Page Self="p1" GeometricBounds="0 0 600 600" ItemTransform="1 0 0 1 0 0"/>
    <Rectangle Self="a" GeometricBounds="0 0 10 10" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Rectangle Self="b" GeometricBounds="0 0 20 20" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Rectangle Self="c" GeometricBounds="0 0 30 30" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Group Self="grp" ItemTransform="1 0 0 1 0 0">
      <Rectangle Self="g1" GeometricBounds="0 0 40 40" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
      <Rectangle Self="g2" GeometricBounds="0 0 50 50" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    </Group>
    <Rectangle Self="host" GeometricBounds="0 0 200 200" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0">
      <Rectangle Self="n1" GeometricBounds="0 0 60 60" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
      <Rectangle Self="n2" GeometricBounds="0 0 70 70" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#
        .to_vec()
}

/// Zip one spread XML into a minimal IDML package.
fn package(spread_xml: &[u8]) -> Vec<u8> {
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
    zip.write_all(spread_xml).unwrap();

    zip.finish().unwrap().into_inner()
}

fn arrange_idml() -> Vec<u8> {
    package(&arrange_idml_spread_xml())
}

fn doc() -> Document {
    idml_import::import_idml_doc(&arrange_idml()).expect("fixture must open")
}

/// The same spread minus the group and the paste-into container — the
/// shape a synthesised (File ▸ New / `InsertNode`-built) document has,
/// where `frames_in_order` is legitimately empty.
fn flat_doc() -> Document {
    let full = String::from_utf8(arrange_idml_spread_xml()).unwrap();
    let flat = {
        let start = full.find("    <Group").expect("group marker");
        let end = full.find("  </Spread>").expect("spread close");
        format!("{}{}", &full[..start], &full[end..])
    };
    idml_import::import_idml_doc(&package(flat.as_bytes())).expect("flat fixture must open")
}

fn z_table(doc: &Document) -> Vec<FrameRef> {
    doc.spreads[0].spread.frames_in_order.clone()
}

fn members(doc: &Document) -> Vec<FrameRef> {
    doc.spreads[0].spread.groups[0].members.clone()
}

fn nested(doc: &Document) -> Vec<FrameRef> {
    doc.spreads[0].spread.nested_children["host"].clone()
}

/// Every display-list command, stringified in emission order — the
/// bytewise read undo has to restore.
fn paint_stream(doc: &Document) -> Vec<String> {
    let built = build_document(doc, &PipelineOptions::default()).expect("build");
    built
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter().map(|c| format!("{c:?}")))
        .collect()
}

/// The WIDTH of every `FillPath`, in paint order. Because the fixture
/// gives each item a unique size, this reads as "who painted, in which
/// order" — the assertion a reorder that never reached the renderer
/// cannot pass.
fn paint_order(doc: &Document) -> Vec<i32> {
    use paged_compose::DisplayCommand;
    let built = build_document(doc, &PipelineOptions::default()).expect("build");
    built
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter())
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0[0].round() as i32),
            _ => None,
        })
        .collect()
}

fn rect(id: &str) -> NodeId {
    NodeId::Rectangle(id.to_string())
}

/// The fixture's premise, asserted once so every later test can read
/// `paint_order` as a plain list of item sizes.
#[test]
fn fixture_paints_back_to_front_in_z_order() {
    let d = doc();
    assert_eq!(
        z_table(&d),
        vec![
            FrameRef::Rectangle(0), // a
            FrameRef::Rectangle(1), // b
            FrameRef::Rectangle(2), // c
            FrameRef::Group(0),     // grp (g1 = 3, g2 = 4)
            FrameRef::Rectangle(5), // host
        ]
    );
    assert_eq!(
        members(&d),
        vec![FrameRef::Rectangle(3), FrameRef::Rectangle(4)]
    );
    assert_eq!(
        nested(&d),
        vec![FrameRef::Rectangle(6), FrameRef::Rectangle(7)]
    );
    assert_eq!(paint_order(&d), vec![10, 20, 30, 40, 50, 200, 60, 70]);
}

// ---------------------------------------------------------------------------
// Top level — the spread z table
// ---------------------------------------------------------------------------

/// The keystone. `bringToFront` on the backmost item moves it to the
/// end of the paint stream, and ONE inverse restores the table and the
/// command stream bytewise.
#[test]
fn bring_to_front_repaints_last_and_the_inverse_is_exact() {
    let mut d = doc();
    let before_table = z_table(&d);
    let before_paint = paint_stream(&d);

    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Front,
        },
    )
    .expect("bring to front");

    assert_eq!(
        z_table(&d),
        vec![
            FrameRef::Rectangle(1),
            FrameRef::Rectangle(2),
            FrameRef::Group(0),
            FrameRef::Rectangle(5),
            FrameRef::Rectangle(0),
        ]
    );
    // THE RENDER ASSERTION — `a` (10pt) now paints last, after the
    // container and its children.
    assert_eq!(paint_order(&d), vec![20, 30, 40, 50, 200, 60, 70, 10]);

    // The inverse is the absolute slot it came from — exact for every
    // verb, the same property `RemoveNode`'s `z_slot` gives undo.
    assert_eq!(
        applied.inverse,
        Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Index(0),
        }
    );
    assert!(applied.invalidation.structural);

    let undone = apply(&mut d, &applied.inverse).expect("undo");
    assert_eq!(z_table(&d), before_table);
    assert_eq!(
        paint_stream(&d),
        before_paint,
        "undo restores the paint stream bytewise"
    );

    // Redo through the inverse's inverse.
    apply(&mut d, &undone.inverse).expect("redo");
    assert_eq!(paint_order(&d), vec![20, 30, 40, 50, 200, 60, 70, 10]);
}

/// `sendToBack` is the mirror; asserted separately because "slot 0"
/// and "slot len-1" are different code paths.
#[test]
fn send_to_back_repaints_first_and_the_inverse_is_exact() {
    let mut d = doc();
    let before_paint = paint_stream(&d);

    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("host"),
            target: ZOrderTarget::Back,
        },
    )
    .expect("send to back");
    assert_eq!(z_table(&d)[0], FrameRef::Rectangle(5));
    assert_eq!(paint_order(&d), vec![200, 60, 70, 10, 20, 30, 40, 50]);
    assert_eq!(
        applied.inverse,
        Operation::ReorderNode {
            node: rect("host"),
            target: ZOrderTarget::Index(4),
        }
    );

    apply(&mut d, &applied.inverse).expect("undo");
    assert_eq!(paint_stream(&d), before_paint);
}

/// The two one-step verbs move exactly one slot and invert exactly.
#[test]
fn relative_verbs_step_exactly_one_slot() {
    let mut d = doc();
    let before_paint = paint_stream(&d);

    // `b` (slot 1) forward → slot 2, i.e. it now paints after `c`.
    let fwd = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("b"),
            target: ZOrderTarget::Forward,
        },
    )
    .expect("bring forward");
    assert_eq!(paint_order(&d), vec![10, 30, 20, 40, 50, 200, 60, 70]);
    assert_eq!(
        fwd.inverse,
        Operation::ReorderNode {
            node: rect("b"),
            target: ZOrderTarget::Index(1)
        }
    );
    apply(&mut d, &fwd.inverse).expect("undo forward");
    assert_eq!(paint_stream(&d), before_paint);

    // `c` (slot 2) backward → slot 1, i.e. it now paints before `b`.
    let back = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("c"),
            target: ZOrderTarget::Backward,
        },
    )
    .expect("send backward");
    assert_eq!(paint_order(&d), vec![10, 30, 20, 40, 50, 200, 60, 70]);
    apply(&mut d, &back.inverse).expect("undo backward");
    assert_eq!(paint_stream(&d), before_paint);
}

/// The absolute form restacks to an exact slot — the shape a layers
/// panel drag and every inverse use.
#[test]
fn an_absolute_index_restacks_to_that_exact_slot() {
    let mut d = doc();
    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("host"),
            target: ZOrderTarget::Index(1),
        },
    )
    .expect("restack to slot 1");
    assert_eq!(z_table(&d)[1], FrameRef::Rectangle(5));
    assert_eq!(paint_order(&d), vec![10, 200, 60, 70, 20, 30, 40, 50]);
    apply(&mut d, &applied.inverse).expect("undo");
    assert_eq!(paint_order(&d), vec![10, 20, 30, 40, 50, 200, 60, 70]);
}

/// A verb that cannot move (already frontmost + `Front`) still APPLIES
/// and still hands back an exact inverse — a successful no-op, not an
/// error. Matches Illustrator/InDesign, both of which push an undo step
/// for a no-op Arrange.
#[test]
fn a_no_op_verb_succeeds_and_its_inverse_is_still_exact() {
    let mut d = doc();
    let before_paint = paint_stream(&d);

    for verb in [ZOrderTarget::Front, ZOrderTarget::Forward] {
        let applied = apply(
            &mut d,
            &Operation::ReorderNode {
                node: rect("host"),
                target: verb,
            },
        )
        .expect("no-op arrange still applies");
        assert_eq!(paint_stream(&d), before_paint);
        assert_eq!(
            applied.inverse,
            Operation::ReorderNode {
                node: rect("host"),
                target: ZOrderTarget::Index(4)
            }
        );
        apply(&mut d, &applied.inverse).expect("undo the no-op");
        assert_eq!(paint_stream(&d), before_paint);
    }
}

/// An absolute `Index` past the end is REJECTED, not clamped — a stale
/// caller model must be heard, not smoothed over — and the document is
/// untouched. (The relative verbs exist so a caller need not hold a
/// fresh index at all.)
#[test]
fn an_out_of_range_index_is_rejected_and_mutates_nothing() {
    let mut d = doc();
    let before_table = z_table(&d);
    let before_paint = paint_stream(&d);

    let err = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Index(9),
        },
    )
    .expect_err("out-of-range index must fail");
    match err {
        OperationError::InvalidPosition {
            parent,
            position,
            len,
        } => {
            assert_eq!(position, 9);
            assert_eq!(len, 5);
            assert_eq!(parent, NodeId::Spread("sp1".into()), "names the list owner");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(z_table(&d), before_table, "no partial mutation");
    assert_eq!(paint_stream(&d), before_paint);
}

// ---------------------------------------------------------------------------
// Containment — the "cannot smuggle" guarantee
// ---------------------------------------------------------------------------

/// Reordering a GROUP MEMBER permutes only that group's `members`. The
/// spread's z table is untouched and the item never appears in it, so a
/// reorder cannot be used as a back-door `DissolveGroup`.
#[test]
fn a_group_member_reorders_inside_its_group_and_never_leaves_it() {
    let mut d = doc();
    let before_table = z_table(&d);
    let before_members = members(&d);
    let before_paint = paint_stream(&d);

    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("g1"),
            target: ZOrderTarget::Front,
        },
    )
    .expect("reorder inside the group");

    assert_eq!(
        members(&d),
        vec![FrameRef::Rectangle(4), FrameRef::Rectangle(3)],
        "the member moved to the front OF ITS GROUP"
    );
    assert_eq!(
        z_table(&d),
        before_table,
        "a group-internal reorder must not touch the spread z table"
    );
    for m in &before_members {
        assert!(
            !z_table(&d).contains(m),
            "a grouped item must never surface in the spread z table"
        );
    }
    // The renderer walks members in order — 40 and 50 swap, and nothing
    // outside the group moves.
    assert_eq!(paint_order(&d), vec![10, 20, 30, 50, 40, 200, 60, 70]);

    apply(&mut d, &applied.inverse).expect("undo");
    assert_eq!(members(&d), before_members);
    assert_eq!(paint_stream(&d), before_paint);
}

/// Reordering a B-18 pasted-in child permutes only that container's
/// `nested_children` entry. The child stays nested (out of the z table,
/// still clipped by the container), so a reorder cannot be used as a
/// back-door `ReleaseFrom` either.
#[test]
fn a_nested_child_reorders_inside_its_container_and_stays_nested() {
    let mut d = doc();
    let before_table = z_table(&d);
    let before_nested = nested(&d);
    let before_paint = paint_stream(&d);

    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("n2"),
            target: ZOrderTarget::Back,
        },
    )
    .expect("reorder inside the container");

    assert_eq!(
        nested(&d),
        vec![FrameRef::Rectangle(7), FrameRef::Rectangle(6)],
        "the child went to the back OF ITS CONTAINER"
    );
    assert_eq!(
        z_table(&d),
        before_table,
        "a nested reorder must not touch the spread z table"
    );
    for c in &before_nested {
        assert!(
            !z_table(&d).contains(c),
            "a nested child must never surface in the spread z table"
        );
    }
    assert_eq!(
        d.spreads[0].spread.nested_children.len(),
        1,
        "no new container entry appeared"
    );
    // 70 now paints before 60, both still INSIDE the container's clip
    // (the 200 that precedes them).
    assert_eq!(paint_order(&d), vec![10, 20, 30, 40, 50, 200, 70, 60]);

    apply(&mut d, &applied.inverse).expect("undo");
    assert_eq!(nested(&d), before_nested);
    assert_eq!(paint_stream(&d), before_paint);
}

/// A whole GROUP restacks like any other top-level item (a group is a
/// first-class `frames_in_order` entry), carrying its members with it.
#[test]
fn a_group_itself_restacks_at_top_level() {
    let mut d = doc();
    let before_paint = paint_stream(&d);

    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: NodeId::Group("grp".into()),
            target: ZOrderTarget::Back,
        },
    )
    .expect("send the group to the back");
    assert_eq!(z_table(&d)[0], FrameRef::Group(0));
    assert_eq!(
        paint_order(&d),
        vec![40, 50, 10, 20, 30, 200, 60, 70],
        "the group's members travel with it"
    );

    apply(&mut d, &applied.inverse).expect("undo");
    assert_eq!(paint_stream(&d), before_paint);
}

/// **The honest limit, pinned.** The renderer sorts `frames_in_order`
/// by `ItemLayer` first (Q-10, `build_engine`), so a bring-to-front
/// cannot lift an item above one on a higher layer — Arrange is
/// within-layer, exactly as in InDesign, where moving between layers is
/// a different gesture (`SetProperty(ItemLayer)`), not an Arrange.
///
/// The op still does its job: the z table changes, and the order the
/// renderer keeps as the WITHIN-LAYER tiebreaker follows. This test
/// exists so that limit is a decision on record rather than a surprise.
#[test]
fn a_layer_sort_outranks_arrange_within_the_spread() {
    let mut d = doc();
    // Put `a` on the top layer and everything else on the bottom one.
    let layer = |id: &str| paged_model::Layer {
        self_id: id.to_string(),
        name: None,
        visible: true,
        locked: false,
        printable: true,
        parent_id: None,
    };
    d.designmap.layers = vec![layer("Layer/bottom"), layer("Layer/top")];
    for (i, r) in d.spreads[0].spread.rectangles.iter_mut().enumerate() {
        r.item_layer = Some(if i == 0 { "Layer/top" } else { "Layer/bottom" }.into());
    }
    let slot_of = |d: &Document, w: i32| paint_order(d).iter().position(|&x| x == w).unwrap();
    // `a` is at z slot 0 yet already paints AFTER `c` — the layer sort
    // won before Arrange was ever involved.
    assert!(slot_of(&d, 10) > slot_of(&d, 30));
    let before_paint = paint_stream(&d);

    // Sending `a` to the back of the z table changes the table…
    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Back,
        },
    )
    .expect("send to back");
    assert_eq!(z_table(&d)[0], FrameRef::Rectangle(0));
    // …and not one pixel of the paint order, because its layer still
    // sits on top. Arrange is a WITHIN-LAYER gesture.
    assert_eq!(
        paint_stream(&d),
        before_paint,
        "layer membership outranks the within-spread z order"
    );
    assert!(slot_of(&d, 10) > slot_of(&d, 30));
    apply(&mut d, &applied.inverse).expect("undo");
}

// ---------------------------------------------------------------------------
// Rejections
// ---------------------------------------------------------------------------

/// Nodes with no stacking position are rejected with a message that
/// says why — a story, a table, a spread, a layer.
#[test]
fn nodes_without_a_stacking_position_are_rejected() {
    let mut d = doc();
    for node in [
        NodeId::Story("Story/s1".into()),
        NodeId::Spread("sp1".into()),
        NodeId::Layer("Layer/l1".into()),
        NodeId::Table {
            story_id: "Story/s1".into(),
            table_id: "Table/t1".into(),
        },
    ] {
        let err = apply(
            &mut d,
            &Operation::ReorderNode {
                node: node.clone(),
                target: ZOrderTarget::Front,
            },
        )
        .expect_err("must reject");
        match err {
            OperationError::InvalidValue { reason, .. } => assert!(
                reason.contains("no stacking position"),
                "unhelpful message for {node:?}: {reason}"
            ),
            other => panic!("unexpected error for {node:?}: {other:?}"),
        }
    }
}

/// A C-28 opacity-mask artwork is painted from NO list — it is consumed
/// by the mask. Reordering it is rejected with an error that names the
/// fix, rather than silently succeeding against a list it isn't in.
#[test]
fn opacity_mask_artwork_has_no_stacking_position() {
    let mut d = doc();
    apply(
        &mut d,
        &Operation::ApplyOpacityMask {
            target: rect("b"),
            mask: rect("c"),
            mask_type: Default::default(),
            invert: false,
        },
    )
    .expect("make the mask");

    let err = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("c"),
            target: ZOrderTarget::Front,
        },
    )
    .expect_err("mask artwork cannot restack");
    match err {
        OperationError::InvalidValue { reason, .. } => {
            assert!(reason.contains("opacity mask"), "{reason}");
            assert!(reason.contains("release"), "must name the fix: {reason}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

/// An id that isn't in the document at all is `NodeNotFound`.
#[test]
fn an_unknown_id_is_node_not_found() {
    let mut d = doc();
    let err = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("nope"),
            target: ZOrderTarget::Front,
        },
    )
    .expect_err("unknown id");
    assert!(matches!(err, OperationError::NodeNotFound(_)));
}

// ---------------------------------------------------------------------------
// Wire shape + the synthesised-document path
// ---------------------------------------------------------------------------

/// Every `ZOrderTarget` survives a JSON round-trip inside the op, and
/// the verbs serialise as the bare camelCase strings the wire doc
/// promises (the `FieldKind` convention).
#[test]
fn serde_round_trips_every_target() {
    for (target, expect) in [
        (ZOrderTarget::Front, r#""front""#),
        (ZOrderTarget::Back, r#""back""#),
        (ZOrderTarget::Forward, r#""forward""#),
        (ZOrderTarget::Backward, r#""backward""#),
        (ZOrderTarget::Index(4), r#"{"index":4}"#),
    ] {
        assert_eq!(serde_json::to_string(&target).unwrap(), expect);
        let op = Operation::ReorderNode {
            node: NodeId::Polygon("Polygon/p1".into()),
            target,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains(r#""kind":"ReorderNode""#), "{json}");
        assert_eq!(serde_json::from_str::<Operation>(&json).unwrap(), op);
    }
}

/// A synthesised spread (File ▸ New, or anything built purely through
/// `InsertNode`) keeps an EMPTY `frames_in_order` — the renderer's
/// legacy kind-vec fallback covers it, and `register_frame_ref`
/// deliberately does not populate a partial table. Arrange materialises
/// it first, and that materialisation has to be RENDER-NEUTRAL.
///
/// Scoped to a FLAT spread, which is the only shape the empty-table
/// case actually takes: `ensure_frames_in_order`'s neutrality claim
/// holds for leaves, not for a spread that already has groups or
/// pasted-in children (the legacy fallback never walks `FrameRef::Group`
/// at all and paints nested children a second time at top level). That
/// corner is unreachable through the mutation API — `CreateGroup` and
/// `PasteInto` both materialise the table before they create the
/// structure that would expose it — so it is documented here rather
/// than defended against.
#[test]
fn an_empty_z_table_materialises_render_neutrally_before_the_reorder() {
    let mut d = flat_doc();
    d.spreads[0].spread.frames_in_order.clear();
    let before_paint = paint_stream(&d);

    // `Index(0)` on the item already at slot 0 of the SYNTHESISED
    // order: the reorder itself is a no-op, so any paint change would
    // be the materialisation's fault.
    let applied = apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Index(0),
        },
    )
    .expect("reorder on a synthesised spread");
    assert_eq!(
        paint_stream(&d),
        before_paint,
        "materialising an empty z table must be render-neutral"
    );
    assert_eq!(
        applied.inverse,
        Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Index(0)
        }
    );

    // …and the now-explicit table is immediately usable.
    apply(
        &mut d,
        &Operation::ReorderNode {
            node: rect("a"),
            target: ZOrderTarget::Front,
        },
    )
    .expect("arrange on the materialised table");
    assert_eq!(*paint_order(&d).last().unwrap(), 10);
}
