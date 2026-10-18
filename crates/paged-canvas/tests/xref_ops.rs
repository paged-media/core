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

//! Update Cross-References (ADR 169) on `xref`, whose sources are stale:
//! the update writes what InDesign 20.0.1 writes, recorded by
//! `xref-update-probe` in `tools/indesign-export/xref-update-probe.json`
//! (each source's ranges after `update()`; a page-number range carries
//! the variable's result).

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::xref;

fn model() -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&xref::build()).expect("idml");
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

/// A source's text as `(text, live page number)`: its runs' text, and
/// the text of its `XRefPageNumber` run.
fn source_text(m: &CanvasModel, source: &str) -> (String, Option<String>) {
    let story = &m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == xref::story_id())
        .unwrap()
        .story;
    let runs: Vec<_> = story
        .paragraphs
        .iter()
        .flat_map(|p| &p.runs)
        .filter(|r| r.hyperlink_source.as_deref() == Some(source))
        .collect();
    let text = runs
        .iter()
        .filter(|r| r.text_variable.is_none())
        .map(|r| r.text.as_str())
        .collect();
    let page = runs
        .iter()
        .find(|r| r.text_variable.is_some())
        .map(|r| r.text.clone());
    (text, page)
}

/// InDesign's record: per source, its text up to its own end (a range
/// dump runs on into the next paragraph) and its page variable's result.
fn indesign_after() -> Vec<(String, (String, Option<String>))> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/indesign-export/xref-update-probe.json");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("probe record")).expect("json");
    let decode = |s: &str| {
        s.replace("<U+201C>", "\u{201C}")
            .replace("<U+201D>", "\u{201D}")
            .replace("<U+0018>", "")
    };
    json["after"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let mut text = String::new();
            let mut page = None;
            for r in s["ranges"].as_array().unwrap() {
                let result = r[2].as_str().unwrap();
                if !result.is_empty() {
                    page = Some(result.to_string());
                    continue;
                }
                let t = decode(r[1].as_str().unwrap());
                text.push_str(t.split("<U+000D>").next().unwrap());
            }
            (s["name"].as_str().unwrap().to_string(), (text, page))
        })
        .collect()
}

#[test]
fn a_stale_source_prints_its_stored_text() {
    let m = model();
    let (text, page) = source_text(&m, &xref::source_id(0));
    assert_eq!(text, "\u{201C}Old Heading\u{201D} on page ");
    assert_eq!(
        page.as_deref(),
        Some(xref::STALE_PAGE),
        "stored, until laid out live"
    );
}

#[test]
fn update_all_writes_what_indesign_writes_and_undoes() {
    let mut m = model();
    let before: Vec<_> = (0..5)
        .map(|i| source_text(&m, &xref::source_id(i)))
        .collect();
    m.apply_mutation(&Mutation::UpdateCrossReferences { source_id: None })
        .expect("update");
    let expected = indesign_after();
    assert_eq!(expected.len(), 5, "{expected:?}");
    for (name, want) in &expected {
        assert_eq!(&source_text(&m, name), want, "{name}");
    }
    m.undo().expect("undo");
    let undone: Vec<_> = (0..5)
        .map(|i| source_text(&m, &xref::source_id(i)))
        .collect();
    assert_eq!(undone, before);
}

#[test]
fn update_one_leaves_the_others() {
    let mut m = model();
    m.apply_mutation(&Mutation::UpdateCrossReferences {
        source_id: Some(xref::source_id(1)),
    })
    .expect("update one");
    assert_eq!(
        source_text(&m, &xref::source_id(1)).0,
        "\u{201C}Where Things Are\u{201D}"
    );
    assert_eq!(
        source_text(&m, &xref::source_id(0)).0,
        "\u{201C}Old Heading\u{201D} on page "
    );
    assert!(m
        .apply_mutation(&Mutation::UpdateCrossReferences {
            source_id: Some("uxs-none".into()),
        })
        .is_err());
}

#[test]
fn updated_references_save_as_idml() {
    let mut m = model();
    m.apply_mutation(&Mutation::UpdateCrossReferences { source_id: None })
        .expect("update");
    let saved = m.export_idml().expect("export");
    let m2 = CanvasModel::load("re", &saved, CanvasOptions::default()).expect("reload");
    for (name, (text, _)) in indesign_after() {
        assert_eq!(source_text(&m2, &name).0, text, "{name} after a save");
    }
}
