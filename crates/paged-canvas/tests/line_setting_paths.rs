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

//! A paragraph's line setting on the wire (ADR 153). The `paragraph-attrs`
//! fixture sets each attribute on one case and leaves it off its twin,
//! with the same text and measure, and InDesign measured both. Writing
//! the attribute onto the bare twin must give the set twin's lines, and
//! undo must give the bare ones back.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_mutate::{PropertyPath as P, Value as V};

const SAMPLE: &str = "paragraph-attrs";

fn model() -> CanvasModel {
    let font = |name: &str| {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/fonts")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("read {name}: {e}"))
    };
    let idml = paged_gen::write_idml(&paged_gen::samples::paragraph_attrs::build()).expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![font("OpenSans.ttf")],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

fn story(case: u32) -> String {
    paged_gen::ids::self_id(SAMPLE, "Story", case)
}

/// A case's lines: `(byte range, baseline y)`.
fn lines(m: &CanvasModel, case: u32) -> Vec<(std::ops::Range<u32>, f32)> {
    m.built()
        .story_layout(&story(case))
        .iter()
        .map(|l| {
            (
                l.byte_range.clone(),
                (l.baseline_y_pt * 100.0).round() / 100.0,
            )
        })
        .collect()
}

fn breaks(m: &CanvasModel, case: u32) -> Vec<std::ops::Range<u32>> {
    lines(m, case).into_iter().map(|(r, _)| r).collect()
}

fn range(case: u32, start: u32, end: u32) -> ElementId {
    ElementId::StoryRange {
        story_id: story(case),
        start,
        end,
    }
}

fn set(m: &mut CanvasModel, id: &ElementId, path: P, value: V) {
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path,
        value,
    })
    .unwrap_or_else(|e| panic!("set {path:?}: {e:?}"));
}

fn read(m: &CanvasModel, id: &ElementId, path: P) -> Option<V> {
    m.element_properties(id)
        .expect("properties")
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{path:?} is read back"))
        .value
}

#[test]
fn auto_leading_on_the_wire_sets_the_lines_apart_and_undoes() {
    // A1 (auto leading absent, InDesign's 120) and A3 (AutoLeading 150)
    // stand side by side in one row: the same baselines once A1 takes 150.
    let mut m = model();
    let before = lines(&m, 0);
    let a3 = lines(&m, 2);
    assert_ne!(before, a3, "the fixture's twins differ");
    let whole = range(0, 0, 1);
    assert_eq!(
        read(&m, &whole, P::ParagraphAutoLeading),
        Some(V::Length(None))
    );
    set(
        &mut m,
        &whole,
        P::ParagraphAutoLeading,
        V::Length(Some(150.0)),
    );
    assert_eq!(lines(&m, 0), a3);
    assert_eq!(
        read(&m, &whole, P::ParagraphAutoLeading),
        Some(V::Length(Some(150.0)))
    );
    m.undo().expect("undo");
    assert_eq!(lines(&m, 0), before);
}

#[test]
fn last_line_indent_on_the_wire_rebreaks_the_paragraph() {
    // C1 (LastLineIndent 0) and C3 (36): the same breaks once C1 takes 36.
    let mut m = model();
    let before = breaks(&m, 8);
    let c3 = breaks(&m, 10);
    assert_ne!(before, c3, "the fixture's twins differ");
    let whole = range(8, 0, 1);
    set(
        &mut m,
        &whole,
        P::ParagraphLastLineIndent,
        V::Length(Some(36.0)),
    );
    assert_eq!(breaks(&m, 8), c3);
    m.undo().expect("undo");
    assert_eq!(breaks(&m, 8), before);
}

#[test]
fn balanced_ragged_lines_on_the_wire_even_the_lines() {
    // D1 (ragged) and D2 (FullyBalanced), the same text.
    let mut m = model();
    let before = breaks(&m, 12);
    let d2 = breaks(&m, 13);
    assert_ne!(before, d2, "the fixture's twins differ");
    let whole = range(12, 0, 1);
    set(
        &mut m,
        &whole,
        P::ParagraphBalanceRaggedLines,
        V::Text("FullyBalanced".into()),
    );
    assert_eq!(breaks(&m, 12), d2);
    assert_eq!(
        read(&m, &whole, P::ParagraphBalanceRaggedLines),
        Some(V::Text("FullyBalanced".into()))
    );
    // `""` clears: ragged again.
    set(
        &mut m,
        &whole,
        P::ParagraphBalanceRaggedLines,
        V::Text(String::new()),
    );
    assert_eq!(breaks(&m, 12), before);
}

#[test]
fn no_break_on_the_wire_keeps_the_phrase_on_one_line() {
    // E1 and E2: "We will all meet in New York City on Friday", E2 with
    // NoBreak on "New York City" (characters 20..33).
    let mut m = model();
    let before = breaks(&m, 16);
    let e2 = breaks(&m, 17);
    assert_ne!(before, e2, "the fixture's twins differ");
    let phrase = range(16, 20, 33);
    set(&mut m, &phrase, P::CharacterNoBreak, V::Bool(true));
    assert_eq!(breaks(&m, 16), e2);
    assert_eq!(read(&m, &phrase, P::CharacterNoBreak), Some(V::Bool(true)));
    m.undo().expect("undo");
    assert_eq!(breaks(&m, 16), before);
}

#[test]
fn an_unknown_single_word_justification_is_refused() {
    let mut m = model();
    let err = m
        .apply_mutation(&Mutation::SetElementProperty {
            element_id: range(4, 0, 1),
            path: P::ParagraphSingleWordJustification,
            value: V::Text("Justify".into()),
        })
        .expect_err("refused");
    assert!(format!("{err:?}").contains("FullyJustified"), "{err:?}");
}
