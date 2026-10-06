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

//! ADR 131's corpus gate: every form the published `idGrammar` lists
//! parses, formats back byte-identically, and RESOLVES on one document
//! that holds an object of every kind — and every core address maps to an
//! engine target (`Address → VirtualTarget`, ADR 132), so `Set` and `Get`
//! can take it.
//!
//! The document is the generated annual (layers, masters, styles of all
//! five families, swatches, gradients, colour groups, conditions and sets,
//! hyperlinks, text variables), topped up through the ordinary mutations
//! with the kinds it lacks: one of each page-item shape and a group, a
//! table, a placeholder field, a numbering list, a guide and a section.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_wire::{address_forms, Address, AddressKind};
use serde_json::json;

fn apply(m: &mut CanvasModel, op: serde_json::Value) -> Option<paged_wire::ElementId> {
    let mutation: Mutation =
        serde_json::from_value(op.clone()).unwrap_or_else(|e| panic!("{op}: {e}"));
    m.apply_mutation(&mutation)
        .unwrap_or_else(|e| panic!("{op}: {e:?}"))
        .created_id
}

fn everything() -> CanvasModel {
    let bytes = paged_gen::write_idml(&paged_gen::samples::annual_base::build()).unwrap();
    let mut m = CanvasModel::load("grammar", &bytes, CanvasOptions::default()).expect("load");
    let page = m.pages()[0].self_id.clone();
    let story = m.scene().stories[0].self_id.clone();
    let spread = m.scene().spreads[0]
        .spread
        .self_id
        .clone()
        .expect("a spread id");
    apply(
        &mut m,
        json!({"op":"insertFrame","args":{"pageId":page,"bounds":[10.0,10.0,40.0,40.0]}}),
    );
    apply(
        &mut m,
        json!({"op":"insertOval","args":{"pageId":page,"bounds":[50.0,10.0,80.0,40.0]}}),
    );
    apply(
        &mut m,
        json!({"op":"insertLine","args":{"pageId":page,"start":[10.0,90.0],"end":[60.0,90.0]}}),
    );
    let a = |x: f32, y: f32| json!({"anchor":[x,y],"left":[x,y],"right":[x,y]});
    let p1 = apply(
        &mut m,
        json!({"op":"insertPath","args":{"pageId":page,"open":false,
        "anchors":[a(100.0,100.0),a(140.0,100.0),a(120.0,130.0)]}}),
    )
    .expect("a polygon");
    let p2 = apply(
        &mut m,
        json!({"op":"insertPath","args":{"pageId":page,"open":false,
        "anchors":[a(200.0,100.0),a(240.0,100.0),a(220.0,130.0)]}}),
    )
    .expect("a polygon");
    apply(
        &mut m,
        json!({"op":"createGroup","args":{"memberIds":[p1,p2]}}),
    );
    apply(
        &mut m,
        json!({"op":"insertTable","args":{"storyId":story,"rows":2,"cols":2}}),
    );
    apply(
        &mut m,
        json!({"op":"insertField","args":{"storyId":story,"offset":0,
        "field":{"placeholder":{"plugin":"media.paged.data","key":"price"}}}}),
    );
    apply(
        &mut m,
        json!({"op":"createNumberingList","args":{"spec":{"name":"Steps"}}}),
    );
    apply(
        &mut m,
        json!({"op":"insertGuide","args":{"spreadId":spread,"orientation":"horizontal","position":72.0}}),
    );
    apply(
        &mut m,
        json!({"op":"insertSection","args":{"atPage":page,"prefix":"A"}}),
    );
    m
}

/// A live address of `kind` in `m`, in canonical form.
fn live(m: &CanvasModel, kind: AddressKind) -> Option<String> {
    let first = |sel: &str| m.query(sel).ok()?.into_iter().next().map(|a| a.to_string());
    match kind {
        // An alias: any page item, by its bare id.
        AddressKind::Frame => {
            let item = first("frame")?;
            Some(format!("frame:{}", item.split_once(':')?.1))
        }
        AddressKind::StoryRange => {
            let story = m.scene().stories[0].self_id.clone();
            Some(
                Address::StoryRange {
                    story_id: story,
                    start: 0,
                    end: 1,
                }
                .to_string(),
            )
        }
        // Opaque to core / bound per batch: carried through as written.
        AddressKind::Plugin | AddressKind::Handle => Some(kind.form().example.to_string()),
        k => first(k.prefix()),
    }
}

#[test]
fn every_published_form_parses_round_trips_and_resolves_on_one_document() {
    let m = everything();
    let mut unresolved = Vec::new();
    for form in address_forms() {
        // The worked example is canonical.
        let example =
            Address::parse(form.example).unwrap_or_else(|e| panic!("{}: {e}", form.example));
        assert_eq!(example.kind(), form.kind, "{}", form.example);
        assert_eq!(
            example.to_string(),
            form.example,
            "{} is not canonical",
            form.example
        );

        let Some(text) = live(&m, form.kind) else {
            unresolved.push(form.kind.prefix());
            continue;
        };
        let parsed = Address::parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(parsed.to_string(), text, "{text} does not format back");
        let resolved = m
            .resolve_address(&text)
            .unwrap_or_else(|| panic!("{text} does not resolve"));
        if form.kind == AddressKind::Frame {
            assert!(resolved.kind().is_page_item(), "{text} → {resolved}");
        } else {
            assert_eq!(resolved.to_string(), text, "{text} resolves elsewhere");
        }
        // ADR 132: every core address is a Set / Get target.
        let target = m.target_of(&parsed);
        match form.kind {
            AddressKind::Plugin | AddressKind::Handle => {
                assert!(target.is_err(), "{text} must not be a core target")
            }
            _ => {
                let t = target.unwrap_or_else(|e| panic!("{text}: {e}"));
                // …and the target maps back to the canonical address.
                if form.kind != AddressKind::Frame {
                    assert_eq!(Address::from(t).to_string(), text, "{text} target");
                }
            }
        }
    }
    assert!(
        unresolved.is_empty(),
        "no object of these kinds in the corpus document: {unresolved:?}"
    );
}

#[test]
fn the_document_dependent_forms_resolve_to_their_canonical_targets() {
    let m = everything();
    // page:#1 → the first page's id.
    let first_page = m.pages()[0].self_id.clone();
    assert_eq!(
        m.resolve_address("page:#1").map(|a| a.to_string()),
        Some(format!("page:{first_page}"))
    );
    // A style by display name → its Self.
    let style = m.query("style").unwrap().into_iter().find_map(|a| match a {
        Address::Style { family, id } => Some((family, id)),
        _ => None,
    });
    let (family, id) = style.expect("a style");
    let name = m
        .get_property_at(
            &Address::Style {
                family,
                id: id.clone(),
            },
            "name",
        )
        .ok()
        .and_then(|v| match v {
            paged_mutate::Value::Text(s) if !s.is_empty() => Some(s),
            _ => None,
        });
    if let Some(name) = name {
        let by_name = format!("style:{}/{name}", family.as_str());
        assert_eq!(
            m.resolve_address(&by_name).map(|a| a.to_string()),
            Some(format!("style:{}/{id}", family.as_str())),
            "{by_name}"
        );
    }
    // A cell finds its story: the target carries it.
    let cell = m.query("cell").unwrap().into_iter().next().expect("a cell");
    match m.target_of(&cell).unwrap() {
        paged_wire::VirtualTarget::Cell { story_id, .. } => assert!(!story_id.is_empty()),
        other => panic!("{cell} → {other:?}"),
    }
}
