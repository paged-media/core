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

//! ADR 132 — transparency effects on a GraphicLine.
//!
//! The line carried an effects bag the importer filled and nothing could
//! set or draw: every effect is defined against a fill path, and a line
//! has none. Its drop shadow is already cast from the stroke's outline
//! band (43d7042); the other effects now are too, so the arms that write
//! them reach `GraphicLine` — each field accepted, undone, redone — and a
//! glow on a line adds an `OuterGlow` command to the page.

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_mutate::{apply, NodeId, NodeSpec, Operation, PropertyPath, Value};
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;

use PropertyPath as P;

fn doc_with_line() -> (Document, NodeId) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/geometry-groups.idml");
    let bytes = std::fs::read(path).expect("read geometry-groups fixture");
    let mut doc = idml_import::import_idml_doc(&bytes).expect("open");
    let spread_id = doc.spreads[0].spread.self_id.clone().expect("spread id");
    apply(
        &mut doc,
        &Operation::InsertNode {
            z_slot: None,
            parent: NodeId::Spread(spread_id),
            position: 0,
            node: NodeSpec::GraphicLine {
                self_id: "GraphicLine/fx_probe".into(),
                bounds: [100.0, 100.0, 100.0, 400.0],
                anchors: Vec::new(),
                subpath_starts: Vec::new(),
                subpath_open: Vec::new(),
                stroke_color: Some("Color/Black".into()),
                stroke_weight: Some(6.0),
                item_transform: None,
            },
        },
    )
    .expect("mint a line");
    (doc, NodeId::GraphicLine("GraphicLine/fx_probe".into()))
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

fn effects_of(doc: &Document) -> String {
    let line = doc
        .spreads
        .iter()
        .flat_map(|s| &s.spread.graphic_lines)
        .find(|l| l.self_id.as_deref() == Some("GraphicLine/fx_probe"))
        .expect("line present");
    // An empty bag and no bag are the same document: the first write
    // materialises the bag, as on every other kind.
    match &line.effects {
        Some(e) if format!("{e:?}") == format!("{:?}", paged_model::FrameEffects::default()) => {
            "None".to_string()
        }
        other => format!("{other:?}"),
    }
}

#[test]
fn every_effect_toggle_and_field_reaches_a_line_and_undoes() {
    let (mut doc, line) = doc_with_line();
    let cases: Vec<(PropertyPath, Value)> = vec![
        (P::FrameInnerShadowEnabled, Value::Bool(true)),
        (P::FrameInnerShadowOpacity, Value::Length(Some(40.0))),
        (P::FrameOuterGlowEnabled, Value::Bool(true)),
        (P::FrameOuterGlowSize, Value::Length(Some(8.0))),
        (P::FrameInnerGlowEnabled, Value::Bool(true)),
        (P::FrameInnerGlowSource, Value::Text("CenterSourced".into())),
        (P::FrameBevelEnabled, Value::Bool(true)),
        (P::FrameBevelDepth, Value::Length(Some(50.0))),
        (P::FrameSatinEnabled, Value::Bool(true)),
        (P::FrameSatinInvert, Value::Bool(true)),
        (P::FrameFeatherEnabled, Value::Bool(true)),
        (P::FrameFeatherWidth, Value::Length(Some(3.0))),
        (P::FrameDirectionalFeatherEnabled, Value::Bool(true)),
        (
            P::FrameDirectionalFeatherLeftWidth,
            Value::Length(Some(2.0)),
        ),
    ];
    for (path, value) in cases {
        let before = effects_of(&doc);
        let applied = apply(&mut doc, &set(&line, path, value))
            .unwrap_or_else(|e| panic!("{path:?} on a line: {e:?}"));
        apply(&mut doc, &applied.inverse).expect("undo");
        assert_eq!(effects_of(&doc), before, "{path:?} undoes exactly");
        apply(&mut doc, &applied.op).expect("redo");
    }
}

#[test]
fn a_glow_on_a_line_is_drawn() {
    fn glows(doc: &Document) -> usize {
        build_document(doc, &PipelineOptions::default())
            .expect("build")
            .pages
            .iter()
            .flat_map(|p| p.list.commands.iter())
            .filter(|c| matches!(c, DisplayCommand::OuterGlow { .. }))
            .count()
    }
    let (mut doc, line) = doc_with_line();
    let before = glows(&doc);
    apply(
        &mut doc,
        &set(&line, P::FrameOuterGlowEnabled, Value::Bool(true)),
    )
    .expect("glow");
    assert_eq!(glows(&doc), before + 1, "the line's glow is painted");
}

#[test]
fn a_group_still_refuses_the_effects_it_cannot_paint() {
    let (mut doc, _) = doc_with_line();
    let group = doc
        .spreads
        .iter()
        .flat_map(|s| &s.spread.groups)
        .find_map(|g| g.self_id.clone())
        .expect("fixture has a group");
    assert!(apply(
        &mut doc,
        &set(
            &NodeId::Group(group),
            P::FrameOuterGlowEnabled,
            Value::Bool(true)
        ),
    )
    .is_err());
}
