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

//! An advertised path is one a SCRIPT can set.
//!
//! `advertised_paths_apply.rs` proves the apply layer names every
//! advertised path, and `catalog_apply_parity` that the element tables'
//! pairs apply. Neither crosses the JS bridge: `paged.set(id, path, v)`
//! first turns the JS value into a wire `Value`, and if that conversion
//! refuses, `paged.set` returns false on a path the catalog promises.
//! `framePath` was exactly that — advertised, applied, and unreachable
//! from a script, because the bridge had no arm for a path's anchors.
//!
//! This gate closes the class: for EVERY advertised path there is a JS
//! literal a script author would write, the bridge converts it, and the
//! apply layer accepts the converted value's TYPE on some real node of
//! a generated document (a semantic refusal such as an out-of-range
//! enum still counts — the type reached the arm; a type mismatch, an
//! unsupported (node, path) pair everywhere, or a refused conversion
//! does not).

use std::collections::BTreeSet;

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_mutate::{
    apply, NodeId, NodeSpec, Operation, OperationError, PropertyPath, StyleCollection, Value,
};
use paged_scene::Document;
use paged_script::execute_script;

/// Literals a script author writes for a scalar path, tried in order.
const BATTERY: &[&str] = &[
    "12",
    "true",
    "\"Color/Black\"",
    "[10, 20, 110, 220]",
    "[1, 0, 0, 1, 5, 5]",
];

/// Paths whose value is structured: the literal a script writes. A path
/// with a natural JS form (`framePath`, `frameStrokeDashArray`) is listed
/// in that form — the bare form is the point. The rest have no form but
/// the `{ type, value }` wrapper, and say so.
const STRUCTURED: &[(&str, &str)] = &[
    ("framePath", "[[0, 0], [100, 0], [50, 80]]"),
    // Enums the apply layer checks against its value list (it answers a
    // value outside the list with a type mismatch naming the list).
    ("paragraphStartParagraph", "\"NextPage\""),
    ("paragraphSpanColumnType", "\"SpanColumns\""),
    ("frameStrokeDashArray", "[6, 3]"),
    (
        "framePathPoint",
        "{ type: 'pathPoint', value: { address: { index: 0, role: 'anchor' }, position: [5, 5] } }",
    ),
    (
        "pathPointInsert",
        "{ type: 'pathPointInsert', value: { index: 1, anchor: { anchor: [1, 1], left: [1, 1], right: [1, 1] } } }",
    ),
    (
        "pathPointRemove",
        "{ type: 'pathPointRemove', value: { index: 0 } }",
    ),
    (
        "pathPointCurveType",
        "{ type: 'pathPointCurveType', value: { index: 0, smooth: true } }",
    ),
    (
        "frameGradientFeather",
        "{ type: 'gradientFeather', value: null }",
    ),
    ("paragraphRuleAbove", "{ type: 'paragraphRule', value: null }"),
    ("paragraphRuleBelow", "{ type: 'paragraphRule', value: null }"),
    ("paragraphTabStops", "{ type: 'tabStops', value: [] }"),
    (
        "pluginMetadata",
        "{ type: 'pluginMetadata', value: { key: 'x-paged:probe', value: '1' } }",
    ),
];

fn load(sample: paged_gen::Sample) -> Document {
    let bytes = paged_gen::write_idml(&sample).expect("generate");
    CanvasModel::load("probe", &bytes, CanvasOptions::default())
        .expect("load")
        .scene()
        .clone()
}

/// Every addressable node kind, each on the first generated document that
/// carries one (a GraphicLine is minted — no sample draws one).
fn probe_nodes() -> Vec<(Document, NodeId)> {
    let mut docs = vec![
        load(paged_gen::samples::geometry::build()),
        load(paged_gen::samples::text::build()),
        load(paged_gen::samples::tables::build()),
        load(paged_gen::samples::geometry_groups::build()),
        load(paged_gen::samples::anchored::build()),
    ];
    let spread_id = docs[0].spreads[0]
        .spread
        .self_id
        .clone()
        .expect("spread id");
    apply(
        &mut docs[0],
        &Operation::InsertNode {
            z_slot: None,
            parent: NodeId::Spread(spread_id),
            position: 0,
            node: NodeSpec::GraphicLine {
                self_id: "GraphicLine/probe".into(),
                bounds: [10.0, 10.0, 60.0, 210.0],
                anchors: Vec::new(),
                subpath_starts: Vec::new(),
                subpath_open: Vec::new(),
                stroke_color: Some("Color/Black".into()),
                stroke_weight: Some(1.0),
                item_transform: None,
            },
        },
    )
    .expect("mint a graphic line");

    let mut out = Vec::new();
    for doc in &docs {
        let spreads = || doc.spreads.iter().map(|s| &s.spread);
        let mut push = |id: Option<NodeId>| {
            if let Some(id) = id {
                out.push((doc.clone(), id));
            }
        };
        push(
            spreads()
                .flat_map(|s| &s.text_frames)
                .find_map(|n| n.self_id.clone())
                .map(NodeId::TextFrame),
        );
        push(
            spreads()
                .flat_map(|s| &s.rectangles)
                .find_map(|n| n.self_id.clone())
                .map(NodeId::Rectangle),
        );
        push(
            spreads()
                .flat_map(|s| &s.ovals)
                .find_map(|n| n.self_id.clone())
                .map(NodeId::Oval),
        );
        push(
            spreads()
                .flat_map(|s| &s.polygons)
                .find_map(|n| n.self_id.clone())
                .map(NodeId::Polygon),
        );
        push(
            spreads()
                .flat_map(|s| &s.graphic_lines)
                .find_map(|n| n.self_id.clone())
                .map(NodeId::GraphicLine),
        );
        push(
            spreads()
                .flat_map(|s| &s.groups)
                .find_map(|n| n.self_id.clone())
                .map(NodeId::Group),
        );
        for story in &doc.stories {
            if let Some(frame) = story
                .story
                .paragraphs
                .iter()
                .flat_map(|p| &p.anchored_frames)
                .find(|f| f.self_id.is_some())
            {
                let id = frame.self_id.clone().expect("checked");
                push(Some(match frame.frame_kind {
                    paged_model::AnchoredFrameKind::TextFrame => NodeId::TextFrame(id),
                    paged_model::AnchoredFrameKind::Rectangle => NodeId::Rectangle(id),
                    paged_model::AnchoredFrameKind::Group => NodeId::Group(id),
                }));
                break;
            }
        }
        push(
            doc.designmap
                .layers
                .first()
                .map(|l| NodeId::Layer(l.self_id.clone())),
        );
        push(doc.stories.first().map(|s| NodeId::StoryRange {
            story_id: s.self_id.clone(),
            start: 0,
            end: 3,
        }));
        for story in &doc.stories {
            if let Some(table_id) = story
                .story
                .paragraphs
                .iter()
                .find_map(|p| p.table.as_ref().and_then(|t| t.self_id.clone()))
            {
                push(Some(NodeId::Table {
                    story_id: story.self_id.clone(),
                    table_id: table_id.clone(),
                }));
                push(Some(NodeId::TableCell {
                    story_id: story.self_id.clone(),
                    table_id,
                    row: 0,
                    col: 0,
                }));
                break;
            }
        }
    }
    out
}

/// Did the value's TYPE reach the arm? A semantic refusal says yes.
fn type_accepted(result: &Result<paged_mutate::AppliedOperation, OperationError>) -> bool {
    !matches!(
        result,
        Err(OperationError::TypeMismatch { .. }
            | OperationError::UnsupportedProperty { .. }
            | OperationError::NodeNotFound(_))
    )
}

fn accepted_somewhere(nodes: &[(Document, NodeId)], path: PropertyPath, value: &Value) -> bool {
    let on_node = nodes.iter().any(|(doc, node)| {
        let mut doc = doc.clone();
        type_accepted(&apply(
            &mut doc,
            &Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
        ))
    });
    // Style-definition paths (`paragraphStyleNextStyle`) write a style,
    // not a node: probe the first paragraph and character style too.
    on_node
        || nodes.iter().take(1).any(|(doc, _)| {
            [
                (
                    StyleCollection::Paragraph,
                    doc.styles.paragraph_styles.keys().next(),
                ),
                (
                    StyleCollection::Character,
                    doc.styles.character_styles.keys().next(),
                ),
            ]
            .into_iter()
            .any(|(collection, id)| {
                let Some(style_id) = id.cloned() else {
                    return false;
                };
                let mut doc = doc.clone();
                type_accepted(&apply(
                    &mut doc,
                    &Operation::SetStyleProperty {
                        collection,
                        style_id,
                        path,
                        value: value.clone(),
                    },
                ))
            })
        })
}

#[test]
fn every_advertised_path_has_a_script_literal_the_engine_accepts() {
    let catalog = paged_script::api_catalog();
    let nodes = probe_nodes();
    let structured: BTreeSet<&str> = STRUCTURED.iter().map(|(p, _)| *p).collect();

    let mut unconstructible = Vec::new();
    for name in &catalog.settable_paths {
        let path = paged_introspect::lookup_path(name).expect("advertised resolves");
        let candidates: Vec<&str> = match STRUCTURED.iter().find(|(p, _)| p == name) {
            Some((_, literal)) => vec![*literal],
            None => BATTERY.to_vec(),
        };
        let ok = candidates.iter().any(|literal| {
            paged_script::js_literal_to_wire(literal, name)
                .is_some_and(|value| accepted_somewhere(&nodes, path, &value))
        });
        if !ok {
            unconstructible.push(*name);
        }
    }
    assert!(
        unconstructible.is_empty(),
        "advertised as settable, but no script literal reaches the engine as a value \
         it accepts — `paged.set` returns false on these: {unconstructible:?}"
    );

    // Typo guard: every structured entry is an advertised path.
    let advertised: BTreeSet<&str> = catalog.settable_paths.iter().copied().collect();
    let stale: Vec<&&str> = structured.difference(&advertised).collect();
    assert!(
        stale.is_empty(),
        "STRUCTURED names unadvertised paths: {stale:?}"
    );
}

/// The path spellings a script uses, end to end through `paged.set`.
#[test]
fn paged_set_writes_a_whole_path_from_its_anchors() {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::geometry_groups::build()).expect("generate");
    let mut model = CanvasModel::load("doc", &bytes, CanvasOptions::default()).expect("load");
    let polygon = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| &s.spread.polygons)
        .find_map(|p| p.self_id.clone())
        .expect("a polygon");
    let anchors_of = |m: &CanvasModel| {
        let p = m
            .scene()
            .spreads
            .iter()
            .flat_map(|s| &s.spread.polygons)
            .find(|p| p.self_id.as_deref() == Some(polygon.as_str()))
            .expect("polygon");
        (
            p.anchors
                .iter()
                .map(|a| (a.anchor, a.left, a.right))
                .collect::<Vec<_>>(),
            p.subpath_starts.clone(),
        )
    };

    // Shorthand corners.
    let r = execute_script(
        &mut model,
        &format!(
            r#"if (!paged.set("polygon:{polygon}", "framePath", [[0, 0], [100, 0], [50, 80]])) throw new Error("rejected");"#
        ),
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    let (anchors, starts) = anchors_of(&model);
    assert_eq!(anchors.len(), 3);
    assert_eq!(anchors[2], ((50.0, 80.0), (50.0, 80.0), (50.0, 80.0)));
    assert_eq!(starts, vec![0]);

    // `{ anchors, subpathStarts }` with explicit handles: a compound path.
    let r = execute_script(
        &mut model,
        &format!(
            r#"if (!paged.set("polygon:{polygon}", "framePath", {{
                anchors: [
                  {{ anchor: [0, 0], right: [10, -5] }}, [40, 0], [20, 30],
                  [60, 60], [90, 60], [75, 90]
                ],
                subpathStarts: [0, 3]
              }})) throw new Error("rejected");"#
        ),
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    let (anchors, starts) = anchors_of(&model);
    assert_eq!(anchors.len(), 6);
    assert_eq!(anchors[0], ((0.0, 0.0), (0.0, 0.0), (10.0, -5.0)));
    assert_eq!(starts, vec![0, 3]);

    // A dash array is its numbers.
    let r = execute_script(
        &mut model,
        &format!(
            r#"if (!paged.set("polygon:{polygon}", "frameStrokeDashArray", [6, 3])) throw new Error("rejected");"#
        ),
    );
    assert!(r.error.is_none(), "{:?}", r.error);
}
