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

//! A style definition's TYPEFACE, and the paragraph hyphenation zone —
//! three fields the model carried, the cascade resolved and the
//! composer honoured, which no surface could set.
//!
//! Both were found the same way: authoring a real 31-page InDesign
//! brochure through the CLI. Its styles are "Avenir Black 20 pt" and
//! "Minion Pro 8 pt", and `setStyleProperty` took the size and refused
//! the face — so every style came out in the document's default
//! typeface. Its body copy came back with seven hyphens where Adobe's
//! had three, because the composer's hyphenation zone could only ever
//! be zero.
//!
//! The shape of the finding is worth keeping: a field that parses,
//! cascades and renders, with no setter, looks complete from every
//! direction except the one that writes it.

use paged_mutate::{apply, NodeId, Operation, PropertyPath, StyleCollection, Value};
use paged_scene::Document;

fn fixture() -> Document {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/text.idml");
    let bytes = std::fs::read(path).expect("read the text fixture");
    idml_import::import_idml_doc(&bytes).expect("open document")
}

fn a_paragraph_style(doc: &Document) -> String {
    doc.styles
        .paragraph_styles
        .keys()
        .next()
        .expect("the fixture declares a paragraph style")
        .clone()
}

fn a_character_style(doc: &Document) -> String {
    doc.styles
        .character_styles
        .keys()
        .next()
        .expect("the fixture declares a character style")
        .clone()
}

fn set_style(
    doc: &mut Document,
    collection: StyleCollection,
    style_id: &str,
    path: PropertyPath,
    value: Value,
) -> Operation {
    apply(
        doc,
        &Operation::SetStyleProperty {
            collection,
            style_id: style_id.to_string(),
            path,
            value,
        },
    )
    .unwrap_or_else(|e| panic!("{path:?} on a {collection:?} style: {e:?}"))
    .inverse
}

#[test]
fn a_paragraph_style_takes_a_typeface_and_gives_it_back() {
    let mut doc = fixture();
    let id = a_paragraph_style(&doc);
    let before = doc.styles.paragraph_styles[&id].font.clone();

    let undo = set_style(
        &mut doc,
        StyleCollection::Paragraph,
        &id,
        PropertyPath::CharacterFontFamily,
        Value::Text("Avenir Black".into()),
    );
    assert_eq!(
        doc.styles.paragraph_styles[&id].font.as_deref(),
        Some("Avenir Black")
    );
    apply(&mut doc, &undo).expect("undo");
    assert_eq!(doc.styles.paragraph_styles[&id].font, before);
}

#[test]
fn a_paragraph_style_takes_a_face_style_and_a_leading() {
    let mut doc = fixture();
    let id = a_paragraph_style(&doc);

    set_style(
        &mut doc,
        StyleCollection::Paragraph,
        &id,
        PropertyPath::CharacterFontStyle,
        Value::Text("Oblique".into()),
    );
    assert_eq!(
        doc.styles.paragraph_styles[&id].font_style.as_deref(),
        Some("Oblique")
    );

    let undo = set_style(
        &mut doc,
        StyleCollection::Paragraph,
        &id,
        PropertyPath::CharacterLeading,
        Value::Length(Some(13.98)),
    );
    assert_eq!(doc.styles.paragraph_styles[&id].leading, Some(13.98));
    apply(&mut doc, &undo).expect("undo");
    assert_eq!(
        doc.styles.paragraph_styles[&id].leading, None,
        "`None` is IDML's Auto, and the inverse must restore it rather than a number"
    );
}

#[test]
fn the_empty_string_clears_a_style_typeface() {
    let mut doc = fixture();
    let id = a_paragraph_style(&doc);
    set_style(
        &mut doc,
        StyleCollection::Paragraph,
        &id,
        PropertyPath::CharacterFontFamily,
        Value::Text("Avenir Black".into()),
    );
    set_style(
        &mut doc,
        StyleCollection::Paragraph,
        &id,
        PropertyPath::CharacterFontFamily,
        Value::Text(String::new()),
    );
    assert_eq!(
        doc.styles.paragraph_styles[&id].font, None,
        "the empty string clears, as it does for a run and for NextStyle"
    );
}

#[test]
fn a_character_style_takes_the_same_three() {
    let mut doc = fixture();
    let id = a_character_style(&doc);

    set_style(
        &mut doc,
        StyleCollection::Character,
        &id,
        PropertyPath::CharacterFontFamily,
        Value::Text("Minion Pro".into()),
    );
    set_style(
        &mut doc,
        StyleCollection::Character,
        &id,
        PropertyPath::CharacterFontStyle,
        Value::Text("Italic".into()),
    );
    set_style(
        &mut doc,
        StyleCollection::Character,
        &id,
        PropertyPath::CharacterLeading,
        Value::Length(Some(9.6)),
    );
    let def = &doc.styles.character_styles[&id];
    assert_eq!(def.font.as_deref(), Some("Minion Pro"));
    assert_eq!(def.font_style.as_deref(), Some("Italic"));
    assert_eq!(def.leading, Some(9.6));
}

#[test]
fn a_wrong_value_kind_is_refused_rather_than_coerced() {
    let mut doc = fixture();
    let id = a_paragraph_style(&doc);
    let before = doc.styles.paragraph_styles[&id].font.clone();
    let err = apply(
        &mut doc,
        &Operation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: id.clone(),
            path: PropertyPath::CharacterFontFamily,
            // A face name is Text; handing it a Length must not write
            // "12" as the family.
            value: Value::Length(Some(12.0)),
        },
    )
    .expect_err("a Length is not a family");
    assert!(format!("{err:?}").contains("TypeMismatch"), "{err:?}");
    assert_eq!(
        doc.styles.paragraph_styles[&id].font, before,
        "a refused write leaves the definition alone"
    );
}

#[test]
fn a_paragraph_carries_its_own_hyphenation_zone() {
    let mut doc = fixture();
    let story_id = doc.stories[0].self_id.clone();

    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: story_id.clone(),
                start: 0,
                end: 10,
            },
            path: PropertyPath::ParagraphHyphenationZone,
            value: Value::Length(Some(12.0)),
        },
    )
    .expect("the zone applies over a story range");

    let para = |d: &Document| {
        d.stories
            .iter()
            .find(|s| s.self_id == story_id)
            .expect("story")
            .story
            .paragraphs[0]
            .hyphenation_zone
    };
    assert_eq!(para(&doc), Some(12.0));
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(para(&doc), None, "the inverse restores the absent zone");
}
