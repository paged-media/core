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
        "dedicated layerSet* mutations — a layer has no element address",
    ),
    (
        "Page",
        "structural; insert/remove mutations, not property writes",
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
