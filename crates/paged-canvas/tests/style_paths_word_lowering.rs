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

//! Every style path plugin-doc's Word lowering sets must be settable on a
//! STYLE. plugin-doc lowers Word's direct formatting to synthesized styles
//! (thoughts ADR 029), and the style setters accepted only size, tracking,
//! fill, family, face, leading, spacing, first-line indent and
//! justification: indents, keeps, tabs, lists, case, position, underline
//! and strike-through were refused ("not supported"), and a refused child
//! rolls back the whole style batch. Each path here must apply, land in the
//! style definition, and undo.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_mutate::operation::TabStopSpec;
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

fn model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::keeps::build()).expect("idml");
    let mut m = CanvasModel::load("doc", &idml, CanvasOptions::default()).expect("load");
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some("ParagraphStyle/w".into()),
        name: Some("W".into()),
        based_on: None,
    })
    .expect("paragraph style");
    m.apply_mutation(&Mutation::CreateCharacterStyle {
        self_id: Some("CharacterStyle/w".into()),
        name: Some("W".into()),
        based_on: None,
    })
    .expect("character style");
    m
}

fn set(m: &mut CanvasModel, collection: StyleCollection, id: &str, path: P, value: V) {
    m.apply_mutation(&Mutation::SetStyleProperty {
        collection,
        style_id: id.into(),
        path,
        value,
    })
    .unwrap_or_else(|e| panic!("{path:?} on {id}: {e:?}"));
}

fn character_paths() -> Vec<(P, V)> {
    vec![
        (P::CharacterCase, V::Text("AllCaps".into())),
        (P::CharacterPosition, V::Text("Superscript".into())),
        (P::CharacterBaselineShift, V::Length(Some(3.0))),
        (P::CharacterUnderline, V::Bool(true)),
        (P::CharacterStrikethru, V::Bool(true)),
    ]
}

#[test]
fn paragraph_styles_take_every_path_the_word_lowering_sets() {
    let mut m = model();
    let mut paths = vec![
        (P::ParagraphLeftIndent, V::Length(Some(18.0))),
        (P::ParagraphRightIndent, V::Length(Some(9.0))),
        (P::ParagraphKeepWithNext, V::Length(Some(1.0))),
        (P::ParagraphKeepLinesTogether, V::Bool(true)),
        (P::ParagraphListType, V::Text("BulletList".into())),
        (P::ParagraphNumberingFormat, V::Text("^#.^t".into())),
        (P::ParagraphBulletCharacter, V::Text("•".into())),
        (
            P::ParagraphTabStops,
            V::TabStops(vec![TabStopSpec {
                position: 72.0,
                alignment: Some("LeftAlign".into()),
                alignment_character: None,
                leader: None,
            }]),
        ),
    ];
    paths.extend(character_paths());
    let n = paths.len();
    for (path, value) in paths {
        set(
            &mut m,
            StyleCollection::Paragraph,
            "ParagraphStyle/w",
            path,
            value,
        );
    }
    let def = &m.scene().styles.paragraph_styles["ParagraphStyle/w"];
    assert_eq!(def.left_indent, Some(18.0));
    assert_eq!(def.right_indent, Some(9.0));
    assert_eq!(def.keep_with_next, Some(1));
    assert_eq!(def.keep_lines_together, Some(true));
    assert_eq!(def.bullets_list_type.as_deref(), Some("BulletList"));
    assert_eq!(def.numbering_format.as_deref(), Some("^#.^t"));
    assert_eq!(def.bullet_character, Some('•' as u32));
    assert_eq!(def.tab_list.len(), 1);
    assert_eq!(def.capitalization.as_deref(), Some("AllCaps"));
    assert_eq!(def.position.as_deref(), Some("Superscript"));
    assert_eq!(def.baseline_shift, Some(3.0));
    assert_eq!(def.underline, Some(true));
    assert_eq!(def.strikethru, Some(true));

    for _ in 0..n {
        m.undo().expect("undo a style edit");
    }
    let def = &m.scene().styles.paragraph_styles["ParagraphStyle/w"];
    assert_eq!(def.left_indent, None);
    assert_eq!(def.keep_with_next, None);
    assert!(def.tab_list.is_empty());
    assert_eq!(def.underline, Some(false), "undo restores the IDML default");
}

#[test]
fn character_styles_take_every_path_the_word_lowering_sets() {
    let mut m = model();
    for (path, value) in character_paths() {
        set(
            &mut m,
            StyleCollection::Character,
            "CharacterStyle/w",
            path,
            value,
        );
    }
    let def = &m.scene().styles.character_styles["CharacterStyle/w"];
    assert_eq!(def.capitalization.as_deref(), Some("AllCaps"));
    assert_eq!(def.position.as_deref(), Some("Superscript"));
    assert_eq!(def.baseline_shift, Some(3.0));
    assert_eq!(def.underline, Some(true));
    assert_eq!(def.strikethru, Some(true));
}
