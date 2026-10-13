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

//! R-10 / R-11 — the per-target transparency paths, a group's isolate and
//! knockout, a line's object opacity, the drop shadow's remaining
//! attributes and the document's global light: each is accepted where
//! the model has the slot, changes it, and is undone by its own inverse;
//! a target the kind does not have is refused.

use std::path::PathBuf;

use paged_mutate::{apply, NodeId, Operation, OperationError, PropertyPath, Value};
use paged_scene::Document;

use PropertyPath as P;

fn fixture() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/transparency-targets.idml");
    let bytes = std::fs::read(path).expect("read transparency-targets fixture (paged-gen emit-all)");
    idml_import::import_idml_doc(&bytes).expect("open")
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

/// The whole item a node names, as text — what a write must change and
/// its inverse must restore.
fn state(doc: &Document, node: &NodeId) -> String {
    let id = node.self_id();
    let spreads = || doc.spreads.iter().map(|p| &p.spread);
    let hit = |s: &Option<String>| s.as_deref() == Some(id);
    match node {
        NodeId::TextFrame(_) => format!(
            "{:?}",
            spreads().flat_map(|s| &s.text_frames).find(|f| hit(&f.self_id))
        ),
        NodeId::Rectangle(_) => format!(
            "{:?}",
            spreads().flat_map(|s| &s.rectangles).find(|f| hit(&f.self_id))
        ),
        NodeId::GraphicLine(_) => format!(
            "{:?}",
            spreads().flat_map(|s| &s.graphic_lines).find(|f| hit(&f.self_id))
        ),
        NodeId::Group(_) => format!(
            "{:?}",
            spreads().flat_map(|s| &s.groups).find(|f| hit(&f.self_id))
        ),
        NodeId::Document(_) => format!("{:?}", doc.designmap.transparency_preference),
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_new_path_writes_and_undoes() {
    let mut doc = fixture();
    let id = |s: &Option<String>| s.clone().expect("self id");
    let text = doc.spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.fill_color.as_deref() == Some("Color/Magenta"))
        .map(|f| NodeId::TextFrame(id(&f.self_id)))
        .expect("a case text frame");
    let picture = doc.spreads[1]
        .spread
        .rectangles
        .iter()
        .find(|r| r.has_image_element)
        .map(|r| NodeId::Rectangle(id(&r.self_id)))
        .expect("a graphic frame");
    let line = NodeId::GraphicLine(id(&doc.spreads[2].spread.graphic_lines[0].self_id));
    let group = NodeId::Group(id(&doc.spreads[3].spread.groups[0].self_id));
    let document = NodeId::Document(String::new());
    // A per-field shadow edit on a shadowless frame materialises the
    // preset, as every `frameDropShadow*` path does: start from one.
    apply(&mut doc, &set(&text, P::FrameDropShadow, Value::Bool(true))).expect("shadow on");

    let cases: Vec<(&NodeId, PropertyPath, Value)> = vec![
        (&text, P::FrameFillOpacity, Value::Length(Some(25.0))),
        (&text, P::FrameFillBlendMode, Value::Text("Multiply".into())),
        (&text, P::FrameStrokeOpacity, Value::Length(Some(35.0))),
        (&text, P::FrameStrokeBlendMode, Value::Text("Screen".into())),
        (&text, P::FrameContentOpacity, Value::Length(Some(10.0))),
        (&text, P::FrameContentBlendMode, Value::Text("Darken".into())),
        (&picture, P::FrameContentOpacity, Value::Length(Some(80.0))),
        (&line, P::FrameOpacity, Value::Length(Some(30.0))),
        (&line, P::FrameBlendMode, Value::Text("Screen".into())),
        (&line, P::FrameStrokeOpacity, Value::Length(Some(20.0))),
        (&group, P::GroupIsolateBlending, Value::Bool(true)),
        (&group, P::GroupKnockout, Value::Bool(true)),
        (&text, P::FrameDropShadowBlendMode, Value::Text("Screen".into())),
        (&text, P::FrameDropShadowSpread, Value::Length(Some(30.0))),
        (&text, P::FrameDropShadowNoise, Value::Length(Some(20.0))),
        (&text, P::FrameDropShadowKnockedOut, Value::Bool(false)),
        (&text, P::FrameDropShadowUseGlobalLight, Value::Bool(true)),
        (&document, P::DocumentGlobalLightAngle, Value::Length(Some(45.0))),
        (&document, P::DocumentGlobalLightAltitude, Value::Length(Some(20.0))),
    ];
    for (node, path, value) in cases {
        let before = state(&doc, node);
        let applied = apply(&mut doc, &set(node, path, value))
            .unwrap_or_else(|e| panic!("{path:?} on {node:?}: {e:?}"));
        assert_ne!(state(&doc, node), before, "{path:?} on {node:?} changes it");
        apply(&mut doc, &applied.inverse).expect("undo");
        assert_eq!(state(&doc, node), before, "{path:?} on {node:?} undoes");
    }

    // A line has no fill or content target.
    for path in [P::FrameFillOpacity, P::FrameContentBlendMode] {
        let value = if path == P::FrameFillOpacity {
            Value::Length(Some(50.0))
        } else {
            Value::Text("Multiply".into())
        };
        assert!(
            matches!(
                apply(&mut doc, &set(&line, path, value)),
                Err(OperationError::UnsupportedProperty { .. })
            ),
            "{path:?} on a line"
        );
    }
}

/// A target cleared back to nothing is removed, so it writes no block.
#[test]
fn a_cleared_target_is_dropped() {
    let mut doc = fixture();
    let rect = doc.spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.fill_color.as_deref() == Some("Color/Magenta") && f.fill_transparency.is_none())
        .and_then(|f| f.self_id.clone())
        .map(NodeId::TextFrame)
        .expect("a frame with no fill target");
    apply(&mut doc, &set(&rect, P::FrameFillOpacity, Value::Length(Some(40.0)))).unwrap();
    apply(&mut doc, &set(&rect, P::FrameFillOpacity, Value::Length(None))).unwrap();
    let f = doc.spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(rect.self_id()))
        .unwrap();
    assert!(f.fill_transparency.is_none());
}
