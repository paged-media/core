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

//! v59 — `reorderElement` through the WIRE (the surface a plugin
//! sends), end to end: the mutation lands, the BUILT display list
//! (what the canvas actually paints, not just the model) changes, and
//! one `undo` puts it back.
//!
//! This is the gap the editor had no door for at all: `bringToFront` /
//! `sendToBack` did not exist anywhere in the app, and five plugin
//! modules documented "inserted items land on top of the z-order" as an
//! accepted limit for want of this one mutation.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, ElementId};
use paged_mutate::ZOrderTarget;

/// One spread, four overlapping rectangles of DISTINCT size, plus a
/// group of two more. Sizes make the built display list read back as an
/// unambiguous paint order. No `ItemLayer` anywhere — the renderer's
/// Q-10 layer sort would otherwise outrank the z table.
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
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1">
    <Page Self="p1" GeometricBounds="0 0 600 600"/>
    <Rectangle Self="a" GeometricBounds="0 0 10 10" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Rectangle Self="b" GeometricBounds="0 0 20 20" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Rectangle Self="c" GeometricBounds="0 0 30 30" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Rectangle Self="d" GeometricBounds="0 0 15 15" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    <Group Self="grp" ItemTransform="1 0 0 1 0 0">
      <Rectangle Self="g1" GeometricBounds="0 0 40 40" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
      <Rectangle Self="g2" GeometricBounds="0 0 50 50" FillColor="Color/Black" StrokeColor="Swatch/None" StrokeWeight="0"/>
    </Group>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

fn load() -> CanvasModel {
    CanvasModel::load("doc-v59", &build_idml(), CanvasOptions::default()).expect("load")
}

/// The BUILT page's fill widths, in paint order — read off the model's
/// own display list, so this is what the canvas would show.
fn painted(model: &CanvasModel) -> Vec<i32> {
    use paged_compose::DisplayCommand;
    model
        .built()
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter())
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0[0].round() as i32),
            _ => None,
        })
        .collect()
}

fn z_ids(model: &CanvasModel) -> Vec<paged_model::FrameRef> {
    model.scene().spreads[0].spread.frames_in_order.clone()
}

/// Bring-to-front through the wire changes what the canvas paints, and
/// one undo puts it back.
#[test]
fn bring_to_front_through_the_wire_repaints_and_undoes() {
    let mut model = load();
    let before = painted(&model);
    assert_eq!(before, vec![10, 20, 30, 15, 40, 50], "fixture precondition");

    model
        .apply_mutation(&Mutation::ReorderElement {
            element_id: ElementId::Rectangle("a".into()),
            to: ZOrderTarget::Front,
        })
        .expect("reorderElement front");
    assert_eq!(painted(&model), vec![20, 30, 15, 40, 50, 10]);

    model.undo().expect("undo");
    assert_eq!(painted(&model), before);

    model.redo().expect("redo");
    assert_eq!(painted(&model), vec![20, 30, 15, 40, 50, 10]);
}

/// Every verb reaches the engine through the wire, and each undo is
/// exact — including the absolute `{ index }` form.
#[test]
fn every_verb_round_trips_through_the_wire() {
    let mut model = load();
    let before_z = z_ids(&model);
    let before = painted(&model);

    for (target, expect) in [
        (ZOrderTarget::Back, vec![30, 10, 20, 15, 40, 50]),
        (ZOrderTarget::Backward, vec![10, 30, 20, 15, 40, 50]),
        (ZOrderTarget::Forward, vec![10, 20, 15, 30, 40, 50]),
        (ZOrderTarget::Front, vec![10, 20, 15, 40, 50, 30]),
        (ZOrderTarget::Index(0), vec![30, 10, 20, 15, 40, 50]),
    ] {
        model
            .apply_mutation(&Mutation::ReorderElement {
                element_id: ElementId::Rectangle("c".into()),
                to: target,
            })
            .unwrap_or_else(|e| panic!("reorderElement {target:?}: {e:?}"));
        assert_eq!(painted(&model), expect, "verb {target:?}");
        model.undo().expect("undo");
        assert_eq!(painted(&model), before, "undo of {target:?}");
        assert_eq!(
            z_ids(&model),
            before_z,
            "undo of {target:?} restores the table"
        );
    }
}

/// A GROUP restacks through the wire too (`ElementId::Group` resolves
/// to `NodeId::Group`), carrying its members.
#[test]
fn a_group_restacks_through_the_wire() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::ReorderElement {
            element_id: ElementId::Group("grp".into()),
            to: ZOrderTarget::Back,
        })
        .expect("reorderElement on a group");
    assert_eq!(painted(&model), vec![40, 50, 10, 20, 30, 15]);
    model.undo().expect("undo");
    assert_eq!(painted(&model), vec![10, 20, 30, 15, 40, 50]);
}

/// A group MEMBER restacks inside its group and never surfaces at top
/// level — the containment guarantee, asserted through the wire.
#[test]
fn a_group_member_stays_inside_its_group_through_the_wire() {
    let mut model = load();
    let before_z = z_ids(&model);
    model
        .apply_mutation(&Mutation::ReorderElement {
            element_id: ElementId::Rectangle("g1".into()),
            to: ZOrderTarget::Front,
        })
        .expect("reorderElement on a group member");
    assert_eq!(painted(&model), vec![10, 20, 30, 15, 50, 40]);
    assert_eq!(
        z_ids(&model),
        before_z,
        "the spread z table must be untouched"
    );
    assert_eq!(
        model.scene().spreads[0].spread.groups[0].members.len(),
        2,
        "membership unchanged"
    );
    model.undo().expect("undo");
    assert_eq!(painted(&model), vec![10, 20, 30, 15, 40, 50]);
}

/// An out-of-range absolute index is rejected through the wire and
/// paints nothing new.
#[test]
fn an_out_of_range_index_is_rejected_through_the_wire() {
    let mut model = load();
    let before = painted(&model);
    let err = model
        .apply_mutation(&Mutation::ReorderElement {
            element_id: ElementId::Rectangle("a".into()),
            to: ZOrderTarget::Index(99),
        })
        .expect_err("stale index must fail loudly");
    let msg = format!("{err:?}");
    assert!(msg.contains("99"), "the error must name the index: {msg}");
    assert_eq!(painted(&model), before);
}

/// **Does a reorder SURVIVE a save?** The two save paths differ, and
/// the difference is load-bearing for a plugin:
///
///   * `.paged` (native) round-trips the new order verbatim — the z
///     table rides the N2 model part, which `export_paged` refreshes.
///   * `.idml` (interchange) round-trips it too, as of the writer's
///     z-reorder save-back lane. It did NOT when `reorderElement`
///     landed: the writer was a byte-preserving splice that re-emitted
///     source XML untouched and only placed NEW items, so an Arrange
///     showed on canvas, survived a native save, and silently reverted
///     through IDML — which would have been worse than not shipping
///     Arrange, since the app confirms an edit it then discards.
///
/// This test pins BOTH halves. It was originally written to pin the
/// LOSS; it now pins the fix, and its failure when the writer landed is
/// exactly the signal a known-limit test exists to give.
#[test]
fn a_reorder_survives_both_paged_and_idml_export() {
    let mut model = load();
    model
        .apply_mutation(&Mutation::ReorderElement {
            element_id: ElementId::Rectangle("a".into()),
            to: ZOrderTarget::Front,
        })
        .expect("reorderElement front");
    let expected = painted(&model);
    assert_eq!(expected, vec![20, 30, 15, 40, 50, 10]);

    // --- LOSSLESS: .paged carries the z table.
    model.refresh_model_part().expect("refresh model part");
    let bytes = model
        .export_paged(paged_canvas::channel::PROTOCOL_VERSION.0)
        .expect("export .paged");
    let reloaded = CanvasModel::load("doc-v59-paged", &bytes, CanvasOptions::default())
        .expect("reload .paged");
    assert_eq!(
        painted(&reloaded),
        expected,
        ".paged must round-trip the new stacking order"
    );

    // --- .idml carries it too, since the writer gained z-reorder
    // save-back. Asserted against `expected` rather than a literal so
    // this cannot drift from the .paged half above.
    let idml = model.export_idml().expect("export idml");
    let reopened = CanvasModel::load("doc-v59-idml", &idml, CanvasOptions::default())
        .expect("reopen exported idml");
    assert_eq!(
        painted(&reopened),
        expected,
        ".idml must round-trip the new stacking order — an Arrange that \
         reverts on save is worse than no Arrange at all"
    );
}

/// **The headline consumer, proven.** paged.draw's Live Paint inserts a
/// path to fill a planar face; it lands on TOP, so the fill paints over
/// the inner half of the strokes bounding it (Illustrator paints faces
/// UNDER edges). With v59 the fix is create-then-arrange as ONE atomic,
/// single-undo mutation: `bindCreated` names the just-inserted path and
/// `reorderElement` addresses it as `$h:<name>` (the C-15 within-batch
/// handle), so [insert, bind, sendToBack] collapses to one history
/// entry.
///
/// Note for consumers: the bare v34 `$created` sentinel is understood
/// by `setPluginMetadata` / `setElementProperty` only. The GENERIC
/// resolver — the one that rewrites any id position of any mutation
/// kind, `reorderElement` included — is armed by the presence of a
/// `bindCreated` child, so bind a handle rather than reaching for
/// `$created` here.
#[test]
fn insert_then_send_to_back_is_one_undoable_batch() {
    use paged_mutate::operation::PathAnchorSpec;

    let mut model = load();
    // A new object needs a fill to be visible in the paint stream at
    // all; document defaults are app state, not an undoable edit.
    model
        .apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: Some("Color/Black".into()),
            stroke_color: None,
            stroke_weight: None,
        })
        .expect("set defaults");
    let before = painted(&model);
    let page = model.built().pages[0].id.clone();
    let anchor = |x: f32, y: f32| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    };

    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                Mutation::InsertPath {
                    page_id: page,
                    anchors: vec![
                        anchor(0.0, 0.0),
                        anchor(0.0, 25.0),
                        anchor(25.0, 25.0),
                        anchor(25.0, 0.0),
                    ],
                    open: false,
                    smooth: false,
                },
                Mutation::BindCreated {
                    handle: "face".into(),
                },
                Mutation::ReorderElement {
                    // The `kind` is discarded by the resolver — the
                    // caller cannot know what the insert minted.
                    element_id: ElementId::Polygon("$h:face".into()),
                    to: ZOrderTarget::Back,
                },
            ],
        })
        .expect("insert + sendToBack as one batch");

    let after = painted(&model);
    assert_eq!(
        after.len(),
        before.len() + 1,
        "the batch created exactly one new painted item"
    );
    assert_eq!(
        &after[1..],
        &before[..],
        "the new path went to the BACK, not on top — everything else keeps its slot"
    );

    // ONE undo takes both children back.
    model.undo().expect("undo");
    assert_eq!(painted(&model), before, "one undo reverses the whole batch");
}
