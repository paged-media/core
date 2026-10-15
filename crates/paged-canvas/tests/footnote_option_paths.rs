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

//! The document's footnote options on the wire (ADR 162). On
//! `footnote-numbering` (story A's notes 1-4 run over two pages), a
//! `PageRestart` written to `doc` numbers page 2's notes from 1 again,
//! lower roman prints them as InDesign labels them, undo gives the
//! document's own options back, and a value InDesign cannot state is
//! refused.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_mutate::Value as V;
use paged_wire::Address;

fn model() -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&paged_gen::samples::footnote_numbering::build(
        &paged_gen::samples::footnote_numbering::NUMBERING,
    ))
    .expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![font],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

fn set(m: &mut CanvasModel, path: &str, value: V) -> Result<(), String> {
    m.apply_mutation(&Mutation::Set {
        address: Address::parse("doc").expect("address"),
        path: path.to_string(),
        value,
    })
    .map(|_| ())
    .map_err(|e| format!("{e:?}"))
}

/// Story A's notes on page 2, as their reference prints them.
fn page_two_labels(m: &CanvasModel) -> Vec<String> {
    let doc = m.scene();
    let options = &doc.designmap.footnote_options;
    let story = paged_gen::samples::footnote_numbering::story_id("footnote-numbering", "A");
    m.built().pages[1]
        .footnotes
        .iter()
        .filter(|f| f.host_story_id == story)
        .map(|f| options.reference_label(f.number))
        .collect()
}

#[test]
fn a_page_restart_written_to_the_document_renumbers_and_undoes() {
    let mut m = model();
    assert_eq!(page_two_labels(&m), ["[3]", "[4]"]);

    set(
        &mut m,
        "documentFootnoteRestartNumbering",
        V::Text("PageRestart".into()),
    )
    .unwrap();
    set(
        &mut m,
        "documentFootnoteNumberingStyle",
        V::Text("LowerRoman".into()),
    )
    .unwrap();
    assert_eq!(page_two_labels(&m), ["[i]", "[ii]"]);

    m.undo().expect("undo style");
    m.undo().expect("undo restart");
    assert_eq!(page_two_labels(&m), ["[3]", "[4]"]);
}

#[test]
fn a_value_indesign_cannot_state_is_refused() {
    let mut m = model();
    for (path, value) in [
        (
            "documentFootnoteRestartNumbering",
            V::Text("EveryLine".into()),
        ),
        ("documentFootnoteNumberingStyle", V::Text("Klingon".into())),
        ("documentFootnoteStartAt", V::Length(Some(0.0))),
        ("documentFootnoteSpaceBetween", V::Length(Some(-2.0))),
    ] {
        assert!(set(&mut m, path, value).is_err(), "{path}");
    }
    assert_eq!(page_two_labels(&m), ["[3]", "[4]"]);
}
