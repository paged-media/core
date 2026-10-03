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
//! (ADR 029), and the style setters accepted only size, tracking,
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

#[test]
fn a_style_refuses_an_unknown_justification_and_keeps_its_own() {
    let mut m = model();
    set(
        &mut m,
        StyleCollection::Paragraph,
        "ParagraphStyle/w",
        P::ParagraphJustification,
        V::Text("CenterAlign".into()),
    );
    let refused = m.apply_mutation(&Mutation::SetStyleProperty {
        collection: StyleCollection::Paragraph,
        style_id: "ParagraphStyle/w".into(),
        path: P::ParagraphJustification,
        value: V::Text("Sideways".into()),
    });
    assert!(refused.is_err(), "an unknown alignment is refused");
    assert_eq!(
        m.scene().styles.paragraph_styles["ParagraphStyle/w"].justification,
        Some(paged_model::Justification::CenterAlign),
        "and the style keeps the alignment it had"
    );
}

/// Kerning and ligatures set on a PARAGRAPH style reach its runs. Word sets
/// text without kerning or ligatures by default, and plugin-doc states
/// that on the document's base paragraph style; the model carried both
/// only on character styles, so a paragraph style could not say it.
#[test]
fn a_paragraph_style_switches_kerning_and_ligatures_off() {
    let idml = paged_canvas::blank::blank_idml(612.0, 792.0);
    let inter = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("read Inter.ttf");
    let opts = CanvasOptions {
        fonts: vec![inter],
        ..CanvasOptions::default()
    };
    let mut m = CanvasModel::load("doc", &idml, opts).expect("load");
    let page = m.scene().spreads[0].spread.pages[0]
        .self_id
        .clone()
        .expect("page");
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: paged_canvas::PageId(page),
            bounds: (36.0, 36.0, 576.0, 756.0),
        })
        .expect("frame");
    let Some(paged_canvas::ElementId::TextFrame(frame)) = out.created_id else {
        panic!("text frame");
    };
    let story = m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame.as_str()))
        .and_then(|f| f.parent_story.clone())
        .expect("story");
    // Pairs Inter kerns: AV, To, WA, Ty.
    let text = "AVAVAVAV Tomorrow WAVE Type AVATAR";
    for mutation in [
        Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: text.into(),
            cell: None,
        },
        Mutation::CreateParagraphStyle {
            self_id: Some("ParagraphStyle/k".into()),
            name: Some("K".into()),
            based_on: None,
        },
        Mutation::ApplyStyle {
            paragraph: None,
            story_id: story.clone(),
            start: 0,
            end: text.chars().count() as u32,
            style: "ParagraphStyle/k".into(),
            scope: paged_mutate::operation::StyleScope::Paragraph,
            cell: None,
        },
    ] {
        m.apply_mutation(&mutation).expect("setup");
    }
    let line_end = |m: &CanvasModel| -> f32 {
        let lines = m.built().story_layout(&story);
        let c = lines[0].clusters.last().expect("clusters");
        c.x_pt + c.advance_pt
    };
    let kerned = line_end(&m);
    set(
        &mut m,
        StyleCollection::Paragraph,
        "ParagraphStyle/k",
        P::CharacterKerningMethod,
        V::Text("None".into()),
    );
    let unkerned = line_end(&m);
    assert!(
        unkerned > kerned + 1.0,
        "without kerning the line is wider: {kerned} -> {unkerned}"
    );
    set(
        &mut m,
        StyleCollection::Paragraph,
        "ParagraphStyle/k",
        P::CharacterLigatures,
        V::Bool(false),
    );
    let def = &m.scene().styles.paragraph_styles["ParagraphStyle/k"];
    assert_eq!(def.kerning_method.as_deref(), Some("None"));
    assert_eq!(def.ligatures_on, Some(false));
    m.undo().expect("undo ligatures");
    m.undo().expect("undo kerning");
    assert!(
        (line_end(&m) - kerned).abs() < 0.001,
        "undo restores kerning"
    );
}
