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

//! v65 — `CanvasModel::style_properties`, the read behind
//! `RequestStyleProperties`. A style editor writes through
//! `SetStyleProperty`, so the read must name exactly the paths that setter
//! accepts (no fewer, or a field is unreachable from the editor; no more,
//! or the editor offers a field that is refused), and every value must be
//! in a shape the setter takes back unchanged.

use std::collections::BTreeSet;

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_mutate::{Operation, OperationError, PropertyPath as P, StyleCollection, Value as V};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::composer::build()).expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![inter_font()],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

/// One of the fixture's per-case styles (the spacing cases carry one per
/// composer).
fn para(m: &CanvasModel) -> String {
    m.scene()
        .styles
        .paragraph_styles
        .keys()
        .find(|k| k.starts_with("ParagraphStyle/Composer Case"))
        .cloned()
        .expect("a per-case paragraph style")
}

fn first_style(m: &CanvasModel, collection: StyleCollection) -> String {
    let s = &m.scene().styles;
    match collection {
        StyleCollection::Paragraph => s.paragraph_styles.keys().next().cloned(),
        StyleCollection::Character => s.character_styles.keys().next().cloned(),
        StyleCollection::Object => s.object_styles.keys().next().cloned(),
        StyleCollection::Cell => s.cell_styles.keys().next().cloned(),
        StyleCollection::Table => s.table_styles.keys().next().cloned(),
    }
    .unwrap_or_else(|| panic!("the fixture has no {collection:?} style"))
}

/// The paths the style setter accepts for `collection`: everything that
/// is not refused as unsupported. A probe value of the wrong kind is
/// refused with `TypeMismatch`, which still means "settable".
fn settable(m: &CanvasModel, collection: StyleCollection, style_id: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for &path in paged_introspect::catalog::ALL_PATHS {
        let mut doc = m.scene().clone();
        let op = Operation::SetStyleProperty {
            collection,
            style_id: style_id.to_string(),
            path,
            value: V::Bool(true),
        };
        match paged_mutate::apply(&mut doc, &op) {
            Err(OperationError::UnsupportedProperty { .. }) => {}
            _ => {
                out.insert(format!("{path:?}"));
            }
        }
    }
    out
}

fn read_paths(m: &CanvasModel, collection: StyleCollection, style_id: &str) -> BTreeSet<String> {
    m.style_properties(collection, style_id)
        .expect("the style resolves")
        .entries
        .iter()
        .map(|e| format!("{:?}", e.path))
        .collect()
}

#[test]
fn style_properties_cover_every_settable_style_path() {
    let m = model();
    let character = first_style(&m, StyleCollection::Character);
    for (collection, id) in [
        (StyleCollection::Paragraph, para(&m)),
        (StyleCollection::Character, character),
    ] {
        let set = settable(&m, collection, &id);
        let read = read_paths(&m, collection, &id);
        assert!(set.len() > 5, "{collection:?}: the probe found {set:?}");
        assert_eq!(
            read, set,
            "{collection:?}: the read and the setter must name the same paths"
        );
    }
    // Object styles (like cell and table ones): nothing settable, nothing read.
    let id = first_style(&m, StyleCollection::Object);
    assert!(settable(&m, StyleCollection::Object, &id).is_empty());
    let props = m
        .style_properties(StyleCollection::Object, &id)
        .expect("resolves");
    assert!(props.entries.is_empty());
}

#[test]
fn every_value_read_is_accepted_back_unchanged() {
    let mut m = model();
    let style = para(&m);
    // Give the style values worth reading first.
    let edits = [
        (P::ParagraphComposer, V::Text("HL Single Optyca".into())),
        (P::ParagraphKeepFirstLines, V::Length(Some(3.0))),
        (P::ParagraphStartParagraph, V::Text("NextColumn".into())),
        (P::ParagraphBulletsTextAfter, V::Text("^t".into())),
        (P::ParagraphNumberingContinue, V::Bool(false)),
        (P::ParagraphSpaceBefore, V::Length(Some(6.0))),
        (P::CharacterUnderline, V::Bool(true)),
    ];
    for (path, value) in edits.clone() {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: style.clone(),
            path,
            value,
        })
        .unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
    }
    let before = m
        .style_properties(StyleCollection::Paragraph, &style)
        .expect("resolves");
    assert_eq!(before.style_id, style);
    assert_eq!(before.based_on.as_deref(), Some("$ID/[No paragraph style]"));
    for (path, value) in &edits {
        let got = before
            .entries
            .iter()
            .find(|e| e.path == *path)
            .and_then(|e| e.value.clone());
        assert_eq!(got.as_ref(), Some(value), "{path:?} reads back as written");
    }
    // Replay every entry through the setter: accepted, and nothing moves.
    for entry in &before.entries {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: style.clone(),
            path: entry.path,
            value: entry.value.clone().expect("a style value is never mixed"),
        })
        .unwrap_or_else(|e| panic!("{:?} refused its own readback: {e:?}", entry.path));
    }
    let after = m
        .style_properties(StyleCollection::Paragraph, &style)
        .expect("resolves");
    assert_eq!(
        serde_json::to_value(&after.entries).unwrap(),
        serde_json::to_value(&before.entries).unwrap(),
        "replaying the readback changes nothing"
    );
}

#[test]
fn an_unknown_style_reads_as_none() {
    let m = model();
    assert!(m
        .style_properties(StyleCollection::Paragraph, "ParagraphStyle/nope")
        .is_none());
    assert!(m
        .style_properties(StyleCollection::Character, &para(&m))
        .is_none());
}
