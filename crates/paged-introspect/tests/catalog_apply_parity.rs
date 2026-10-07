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

//! Catalog×apply parity — what the catalog ADVERTISES, apply must SUPPORT.
//!
//! The catalog's `elements[].attributes[].settable_path` is a public
//! promise: "this attribute on this element is writable through
//! `paged.set`". Nothing enforced the promise until 2026-08-18, when the
//! corpus sweep caught `FrameBounds` on `Oval` advertised-but-rejected —
//! every sibling kind had the arm, ovals didn't, and the unrotated
//! translate gesture failed on every oval in every document. This test
//! makes that bug class structurally impossible: every advertised
//! (element, settable_path) pair on a node-addressable kind is APPLIED
//! against a real fixture node and must succeed.
//!
//! Scope: the node-addressable page-item kinds (TextFrame / Rectangle /
//! Oval / Polygon / GraphicLine / Group). Story, ParagraphStyleRange and
//! CharacterStyleRange paths write through RANGE mutations (a different
//! op shape); Page / Spread / Layer attrs are structural or route through
//! dedicated mutations (`layerSet*`) — all named in `OUT_OF_SCOPE` so a
//! new catalog element must be classified here or the test fails.
//!
//! Ratchet: `KNOWN_UNSUPPORTED` is a SHRINK-ONLY list (the editor
//! capability matrix's `KNOWN_UNCLASSIFIED` model). A newly advertised
//! pair that fails → immediate red naming the campaign rule; a listed
//! pair that starts succeeding → red demanding the entry's removal.

use paged_introspect::{api_catalog, lookup_path};
use paged_mutate::{apply, NodeId, NodeSpec, Operation, PropertyPath, Value};
use paged_scene::Document;

/// (element name, settable_path) pairs the engine currently rejects, each
/// with the reason and, implicitly, a filed gap. Shrink-only: remove the
/// entry when the arm lands. Empty today — keep it that way.
const KNOWN_UNSUPPORTED: &[(&str, &str, &str)] = &[];

/// Catalog elements whose settable attrs do NOT go through
/// `SetProperty { node, path, value }` — with the mechanism that owns
/// them instead. A catalog element that is neither here nor in `SCOPE`
/// fails the test, so new elements must be classified deliberately.
const OUT_OF_SCOPE: &[(&str, &str)] = &[
    (
        "Story",
        "range mutations (insertText / applyStyle / deleteRange)",
    ),
    (
        "ParagraphStyleRange",
        "range mutations with story-local offsets",
    ),
    (
        "CharacterStyleRange",
        "range mutations with story-local offsets",
    ),
    (
        "Layer",
        "a layer has no element address: its paths live on the `layer` kind and are probed through `Mutation::Set` by paged-canvas/tests/virtual_paths.rs",
    ),
    (
        "Page",
        "structural here; its settable paths (bounds, margins, columns, master, labels) live on the `page` kind and are probed by paged-canvas/tests/virtual_paths.rs",
    ),
    ("Spread", "structural; not a paged.set target"),
];

const SCOPE: &[&str] = &[
    "TextFrame",
    "Rectangle",
    "Oval",
    "Polygon",
    "GraphicLine",
    "Group",
];

fn fixture(name: &str) -> Document {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    idml_import::import_idml_doc(&bytes).unwrap_or_else(|e| panic!("open {name}: {e:?}"))
}

/// First self-id of `kind` in `doc`, if any.
fn find_kind(doc: &Document, kind: &str) -> Option<String> {
    let spreads = doc.spreads.iter().map(|s| &s.spread);
    match kind {
        "TextFrame" => spreads
            .flat_map(|s| s.text_frames.iter())
            .find_map(|n| n.self_id.clone()),
        "Rectangle" => spreads
            .flat_map(|s| s.rectangles.iter())
            .find_map(|n| n.self_id.clone()),
        "Oval" => spreads
            .flat_map(|s| s.ovals.iter())
            .find_map(|n| n.self_id.clone()),
        "Polygon" => spreads
            .flat_map(|s| s.polygons.iter())
            .find_map(|n| n.self_id.clone()),
        "GraphicLine" => spreads
            .flat_map(|s| s.graphic_lines.iter())
            .find_map(|n| n.self_id.clone()),
        "Group" => spreads
            .flat_map(|s| s.groups.iter())
            .find_map(|n| n.self_id.clone()),
        other => panic!("SCOPE lists {other} but find_kind has no finder for it"),
    }
}

fn node_id(kind: &str, id: String) -> NodeId {
    match kind {
        "TextFrame" => NodeId::TextFrame(id),
        "Rectangle" => NodeId::Rectangle(id),
        "Oval" => NodeId::Oval(id),
        "Polygon" => NodeId::Polygon(id),
        "GraphicLine" => NodeId::GraphicLine(id),
        "Group" => NodeId::Group(id),
        other => panic!("SCOPE lists {other} but node_id has no constructor for it"),
    }
}

/// The generated corpus has no GraphicLine fixture; create one through
/// the engine's own insert door (Line-tool shape: spread-space anchors
/// omitted, no captured transform).
fn mint_graphic_line(doc: &mut Document) -> String {
    let spread_id = doc
        .spreads
        .iter()
        .find_map(|s| s.spread.self_id.clone())
        .expect("fixture has a spread with a self id");
    let id = "GraphicLine/parity_probe".to_string();
    apply(
        doc,
        &Operation::InsertNode {
            z_slot: None,
            parent: NodeId::Spread(spread_id),
            position: 0,
            node: NodeSpec::GraphicLine {
                self_id: id.clone(),
                bounds: [10.0, 10.0, 60.0, 210.0],
                anchors: Vec::new(),
                subpath_starts: Vec::new(),
                subpath_open: Vec::new(),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(1.0),
                item_transform: None,
            },
        },
    )
    .expect("the engine's own insert door mints a GraphicLine");
    id
}

/// A type-correct, definitely-a-change sample value per catalog
/// `type_hint`. An unrecognised hint fails LOUDLY — a new hint means a
/// new value family, and guessing here would probe the wrong thing.
fn sample_value(type_hint: &str) -> Value {
    match type_hint {
        "[t,l,b,r] points" => Value::Bounds([11.0, 13.0, 111.0, 113.0]),
        h if h.starts_with("affine") => Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 7.0, 5.0])),
        "swatch ref" => Value::ColorRef(Some("Color/Black".to_string())),
        "points" => Value::Length(Some(2.5)),
        "0–100" => Value::Length(Some(50.0)),
        other => panic!(
            "no sample value for catalog type_hint {other:?} — add one so the \
             advertised pair is actually probed"
        ),
    }
}

#[test]
fn every_advertised_settable_pair_applies() {
    // Each fixture opens once; a kind is probed in the first fixture that
    // carries one. Between them these three cover all six SCOPE kinds.
    let mut fixtures = [
        fixture("geometry.idml"),
        fixture("strokes-fills.idml"),
        fixture("text.idml"),
        fixture("geometry-groups.idml"),
    ];

    let catalog = api_catalog();
    let known: Vec<(&str, &str)> = KNOWN_UNSUPPORTED
        .iter()
        .map(|(el, path, _)| (*el, *path))
        .collect();
    let mut visited_known: Vec<(&str, &str)> = Vec::new();
    let mut probed = 0usize;

    for element in &catalog.elements {
        if OUT_OF_SCOPE.iter().any(|(name, _)| name == &element.name) {
            continue;
        }
        assert!(
            SCOPE.contains(&element.name),
            "catalog element {:?} is neither in SCOPE nor OUT_OF_SCOPE — \
             classify it deliberately (does paged.set address it?)",
            element.name
        );

        // The node this element is probed on, in whichever fixture has one.
        // No generated fixture carries a GraphicLine, so that kind is
        // MINTED through the engine's own insert door — which doubles as
        // coverage that the door produces a node the property arms accept.
        let (doc_idx, id) = fixtures
            .iter()
            .enumerate()
            .find_map(|(i, d)| find_kind(d, element.name).map(|id| (i, id)))
            .unwrap_or_else(|| match element.name {
                "GraphicLine" => (0, mint_graphic_line(&mut fixtures[0])),
                other => panic!(
                    "no {other} with a self id in any parity fixture — extend \
                     the fixture list, don't skip the kind"
                ),
            });

        for attr in &element.attributes {
            let Some(path_name) = attr.settable_path else {
                continue;
            };
            let path: PropertyPath = lookup_path(path_name).unwrap_or_else(|| {
                panic!(
                    "catalog advertises {path_name:?} on {} but lookup_path \
                     doesn't resolve it — PROPERTY_PATHS and the element \
                     tables have drifted apart",
                    element.name
                )
            });

            let op = Operation::SetProperty {
                node: node_id(element.name, id.clone()),
                path,
                value: sample_value(attr.type_hint),
            };
            probed += 1;
            match apply(&mut fixtures[doc_idx], &op) {
                Ok(_) => {
                    if known.contains(&(element.name, path_name)) {
                        panic!(
                            "({}, {path_name}) is in KNOWN_UNSUPPORTED but the \
                             engine now applies it — shrink the list",
                            element.name
                        );
                    }
                }
                Err(e) => {
                    if known.contains(&(element.name, path_name)) {
                        visited_known.push((element.name, path_name));
                    } else {
                        panic!(
                            "catalog advertises {path_name} on {} but apply \
                             rejects it: {e:?}\nThe catalog is a public promise \
                             — add the missing arm (the oval-FrameBounds \
                             precedent, 2026-08-18) or, if genuinely \
                             unsupportable, move the pair to KNOWN_UNSUPPORTED \
                             with its reason AND file the gap.",
                            element.name
                        );
                    }
                }
            }
        }
    }

    // Typo guard, both directions: every ratchet entry must correspond to a
    // pair the walk actually probed-and-saw-fail.
    for (el, path, reason) in KNOWN_UNSUPPORTED {
        assert!(
            visited_known.contains(&(el, path)),
            "KNOWN_UNSUPPORTED lists ({el}, {path}) [{reason}] but the catalog \
             walk never saw it fail — stale or misspelled entry"
        );
    }
    assert!(
        probed >= 20,
        "only {probed} advertised pairs probed — the catalog walk lost its \
         subject; check SCOPE/OUT_OF_SCOPE filtering"
    );
}

/// ADR 132 — the schema's `wire` is the form the apply layer TAKES.
///
/// The Boa bridge, the CLI and `Mutation::Set` build a `Value` from the
/// schema alone (`schema::coerce` / `from_json`), so a schema naming the
/// wrong wire form would make every surface send a value the engine
/// refuses as a type mismatch. For every advertised path with a scalar
/// wire form, a value OF THAT FORM must reach an arm on some real node:
/// applied, or refused for its content (an out-of-vocabulary token) — not
/// for its type, and not for want of any node kind that takes the path.
#[test]
fn every_advertised_schema_names_the_wire_apply_takes() {
    use paged_introspect::catalog::{schema, PROPERTY_PATHS};
    use paged_introspect::schema::{DefaultValue, Wire};
    use paged_mutate::OperationError;

    let mut docs = [
        fixture("geometry.idml"),
        fixture("text.idml"),
        fixture("tables.idml"),
        fixture("geometry-groups.idml"),
        fixture("anchored.idml"),
    ];
    let line = mint_graphic_line(&mut docs[0]);
    let mut nodes: Vec<(usize, NodeId)> = vec![(0, NodeId::GraphicLine(line))];
    for (i, d) in docs.iter().enumerate() {
        for kind in SCOPE {
            if let Some(id) = find_kind(d, kind) {
                nodes.push((i, node_id(kind, id)));
            }
            // anchored frames live inside stories; every rectangle of the
            // anchored fixture is a candidate
        }
        for s in d.spreads.iter().map(|s| &s.spread) {
            for r in &s.rectangles {
                if let Some(id) = &r.self_id {
                    nodes.push((i, NodeId::Rectangle(id.clone())));
                }
            }
        }
        // Anchored frames live in the stories, not the spreads; the
        // anchored-object arms address them by Self id, kind-agnostic.
        for st in &d.stories {
            for para in &st.story.paragraphs {
                for af in &para.anchored_frames {
                    if let Some(id) = &af.self_id {
                        nodes.push((i, NodeId::Rectangle(id.clone())));
                    }
                }
            }
        }
        if let Some(s) = d.stories.iter().find(|s| !s.story.paragraphs.is_empty()) {
            nodes.push((
                i,
                NodeId::StoryRange {
                    story_id: s.self_id.clone(),
                    start: 0,
                    end: 2,
                },
            ));
        }
        for s in &d.stories {
            if let Some(t) = s.story.paragraphs.iter().find_map(|p| p.table.as_ref()) {
                let table_id = t.self_id.clone().expect("table id");
                nodes.push((
                    i,
                    NodeId::Table {
                        story_id: s.self_id.clone(),
                        table_id: table_id.clone(),
                    },
                ));
                nodes.push((
                    i,
                    NodeId::TableCell {
                        story_id: s.self_id.clone(),
                        table_id,
                        row: 0,
                        col: 0,
                    },
                ));
                break;
            }
        }
    }

    let mut wrong = Vec::new();
    for (name, path) in PROPERTY_PATHS {
        let s = schema(*path);
        let value = match s.wire {
            Wire::Bool => Value::Bool(true),
            Wire::Length => Value::Length(Some(match s.default {
                Some(DefaultValue::Number(n)) => n as f32,
                _ => s.min.unwrap_or(1.0).max(1.0) as f32,
            })),
            Wire::Text => Value::Text(
                s.members
                    .first()
                    .map(|m| (*m).to_string())
                    .unwrap_or_else(|| "probe".to_string()),
            ),
            Wire::ColorRef => Value::ColorRef(Some("Color/Black".into())),
            Wire::Bounds => Value::Bounds([1.0, 2.0, 50.0, 60.0]),
            Wire::Transform => Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 2.0, 3.0])),
            Wire::Lengths => Value::Lengths(vec![2.0, 1.0]),
            // Structured and operation wires have their own probes.
            _ => continue,
        };
        let reached = nodes.iter().any(|(i, node)| {
            let mut doc = docs[*i].clone();
            match apply(
                &mut doc,
                &Operation::SetProperty {
                    node: node.clone(),
                    path: *path,
                    value: value.clone(),
                },
            ) {
                Ok(_) => true,
                Err(OperationError::TypeMismatch { .. })
                | Err(OperationError::UnsupportedProperty { .. })
                | Err(OperationError::NodeNotFound(_)) => false,
                Err(_) => true,
            }
        });
        // A path some style definition takes (`paragraphStyleNextStyle`
        // lives only there) reaches its arm through `SetStyleProperty`.
        let reached = reached
            || docs.iter().any(|d| {
                d.styles.paragraph_styles.keys().any(|style_id| {
                    let mut doc = d.clone();
                    !matches!(
                        apply(
                            &mut doc,
                            &Operation::SetStyleProperty {
                                collection: paged_mutate::StyleCollection::Paragraph,
                                style_id: style_id.clone(),
                                path: *path,
                                value: value.clone(),
                            },
                        ),
                        Err(OperationError::TypeMismatch { .. })
                            | Err(OperationError::UnsupportedProperty { .. })
                    )
                })
            });
        if !reached {
            wrong.push(format!("{name} ({:?})", s.wire));
        }
    }
    assert!(
        wrong.is_empty(),
        "these schemas name a wire form no apply arm takes: {wrong:?}"
    );
}

/// Every op-backed virtual row (and its `clearOp`) names an op the wire
/// has, so a binding that resolves a row through the catalog lands on a
/// real door. `rename<Family>Style` stands for the five rename ops.
#[test]
fn every_virtual_row_names_a_real_op() {
    use paged_introspect::virtual_paths::VIRTUAL_PATHS;
    let ops = paged_introspect::catalog::api_catalog().operations;
    let real = |op: &str| {
        if op.contains("<Family>") {
            ["Paragraph", "Character", "Object", "Cell", "Table"]
                .iter()
                .all(|f| ops.iter().any(|o| *o == op.replace("<Family>", f)))
        } else {
            ops.iter().any(|o| o == op)
        }
    };
    let mut wrong = Vec::new();
    for row in VIRTUAL_PATHS {
        for op in std::iter::once(row.op).chain(row.clear_op) {
            if !real(op) {
                wrong.push(format!("{}.{} -> {op}", row.address_kind, row.path));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "virtual rows name ops the wire lacks: {wrong:?}"
    );
    // The four bindable gaps closed in v71 are rows.
    for path in [
        "imageBytes",
        "nextTextFrame",
        "previousTextFrame",
        "opacityMask",
        "points[i].anchor",
        "points[i].left",
        "points[i].right",
        "points[i].curveType",
    ] {
        assert!(
            VIRTUAL_PATHS
                .iter()
                .any(|r| r.address_kind == "frame" && r.path == path),
            "frame.{path} is not a virtual row"
        );
    }
}
