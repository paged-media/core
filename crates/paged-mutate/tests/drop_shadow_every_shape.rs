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

//! A drop shadow on every shape, not only on frames and rectangles.
//!
//! The six `frameDropShadow*` paths and the `frameDropShadow` toggle
//! applied to text frames and rectangles; an oval carried the field but
//! no arm reached it, and polygons and lines had no field at all. So a
//! shadow on a pen path or a rule could not be made, and a renderer that
//! knew how to stamp one under a polygon's real outline was never asked.
//!
//! What this file pins, for Oval, Polygon and GraphicLine:
//!
//! * the toggle and every per-field path are ACCEPTED, change the
//!   shadow, and are restored by their own inverses;
//! * the shadow is DRAWN: a polygon's under its own outline, a line's
//!   under its stroke (a line has no fill to cast one).

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_mutate::{apply, NodeId, NodeSpec, Operation, PropertyPath, Value};
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;

use PropertyPath as P;

fn fixture() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    let bytes = std::fs::read(path).expect("read geometry-groups fixture");
    idml_import::import_idml_doc(&bytes).expect("open")
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

/// A polygon with real anchors, plus an oval and a line minted where the
/// polygon sits (so they land on the same page).
fn shapes(doc: &mut Document) -> [NodeId; 3] {
    let spreads = || doc.spreads.iter().flat_map(|s| s.spread.polygons.iter());
    let poly = spreads()
        .find(|p| !p.anchors.is_empty() && p.self_id.is_some())
        .expect("fixture has a pathed polygon")
        .clone();
    let spread_id = doc.spreads[0].spread.self_id.clone().expect("spread id");
    apply(
        doc,
        &Operation::InsertNode {
            z_slot: None,
            parent: NodeId::Spread(spread_id),
            position: 0,
            node: NodeSpec::GraphicLine {
                self_id: "GraphicLine/shadow_probe".into(),
                bounds: [
                    poly.bounds.top,
                    poly.bounds.left,
                    poly.bounds.bottom,
                    poly.bounds.right,
                ],
                anchors: Vec::new(),
                subpath_starts: Vec::new(),
                subpath_open: Vec::new(),
                stroke_color: Some("Color/Black".into()),
                stroke_weight: Some(4.0),
                item_transform: poly.item_transform,
            },
        },
    )
    .expect("mint a line");
    apply(
        doc,
        &Operation::InsertNode {
            z_slot: None,
            parent: NodeId::Spread(doc.spreads[0].spread.self_id.clone().expect("spread id")),
            position: 0,
            node: NodeSpec::Oval {
                self_id: "Oval/shadow_probe".into(),
                bounds: [
                    poly.bounds.top,
                    poly.bounds.left,
                    poly.bounds.bottom,
                    poly.bounds.right,
                ],
                fill_color: Some("Color/Black".into()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: poly.item_transform,
            },
        },
    )
    .expect("mint an oval");
    [
        NodeId::Oval("Oval/shadow_probe".into()),
        NodeId::Polygon(poly.self_id.expect("checked")),
        NodeId::GraphicLine("GraphicLine/shadow_probe".into()),
    ]
}

fn shadow_of(doc: &Document, node: &NodeId) -> Option<String> {
    let id = node.self_id();
    let items = doc.spreads.iter().map(|s| &s.spread);
    let found = match node {
        NodeId::Oval(_) => items
            .flat_map(|s| &s.ovals)
            .find(|o| o.self_id.as_deref() == Some(id))
            .map(|o| o.drop_shadow.clone()),
        NodeId::Polygon(_) => items
            .flat_map(|s| &s.polygons)
            .find(|o| o.self_id.as_deref() == Some(id))
            .map(|o| o.drop_shadow.clone()),
        NodeId::GraphicLine(_) => items
            .flat_map(|s| &s.graphic_lines)
            .find(|o| o.self_id.as_deref() == Some(id))
            .map(|o| o.drop_shadow.clone()),
        other => panic!("not a shape: {other:?}"),
    };
    found.expect("shape present").map(|s| format!("{s:?}"))
}

#[test]
fn every_shape_takes_a_drop_shadow_and_gives_it_back() {
    let mut doc = fixture();
    for node in shapes(&mut doc) {
        assert_eq!(shadow_of(&doc, &node), None, "{node:?} starts shadowless");

        let on = apply(&mut doc, &set(&node, P::FrameDropShadow, Value::Bool(true)))
            .unwrap_or_else(|e| panic!("toggle on {node:?}: {e:?}"));
        let preset = shadow_of(&doc, &node).expect("the toggle materialises the preset");

        let fields: [(PropertyPath, Value); 6] = [
            (P::FrameDropShadowMode, Value::Text("Drop".into())),
            (P::FrameDropShadowXOffset, Value::Length(Some(9.0))),
            (P::FrameDropShadowYOffset, Value::Length(Some(-4.0))),
            (P::FrameDropShadowSize, Value::Length(Some(11.0))),
            (P::FrameDropShadowOpacity, Value::Length(Some(40.0))),
            (
                P::FrameDropShadowColor,
                Value::ColorRef(Some("Color/Black".into())),
            ),
        ];
        for (path, value) in fields {
            let before = shadow_of(&doc, &node);
            let applied = apply(&mut doc, &set(&node, path, value))
                .unwrap_or_else(|e| panic!("{path:?} on {node:?}: {e:?}"));
            apply(&mut doc, &applied.inverse).expect("undo the field");
            assert_eq!(
                shadow_of(&doc, &node),
                before,
                "{path:?} on {node:?} undoes"
            );
            apply(&mut doc, &applied.op).expect("redo the field");
        }
        assert_ne!(shadow_of(&doc, &node).as_deref(), Some(preset.as_str()));

        let off = apply(
            &mut doc,
            &set(&node, P::FrameDropShadow, Value::Bool(false)),
        )
        .expect("toggle off");
        assert_eq!(shadow_of(&doc, &node), None);
        apply(&mut doc, &off.inverse).expect("undo off");
        assert!(
            shadow_of(&doc, &node).is_some(),
            "{node:?}: undo of off restores"
        );
        apply(&mut doc, &on.inverse).expect("undo on");
        assert_eq!(shadow_of(&doc, &node), None, "{node:?}: undo of on clears");
    }
}

#[test]
fn polygon_and_line_shadows_are_drawn() {
    fn shadows(doc: &Document) -> usize {
        build_document(doc, &PipelineOptions::default())
            .expect("build")
            .pages
            .iter()
            .flat_map(|p| p.list.commands.iter())
            .filter(|c| matches!(c, DisplayCommand::DropShadow { .. }))
            .count()
    }
    let mut doc = fixture();
    let [_, polygon, line] = shapes(&mut doc);
    // The fill shadow is cast by a visible fill.
    apply(
        &mut doc,
        &set(
            &polygon,
            P::FrameFillColor,
            Value::ColorRef(Some("Color/Black".into())),
        ),
    )
    .expect("fill the polygon");
    let before = shadows(&doc);

    apply(
        &mut doc,
        &set(&polygon, P::FrameDropShadow, Value::Bool(true)),
    )
    .expect("on");
    assert_eq!(shadows(&doc), before + 1, "the polygon casts its shadow");

    apply(&mut doc, &set(&line, P::FrameDropShadow, Value::Bool(true))).expect("on");
    assert_eq!(
        shadows(&doc),
        before + 2,
        "the line casts its stroke's shadow"
    );
}
