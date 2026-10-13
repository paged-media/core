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

//! Frames anchored in a story are page items: a kind selector lists them
//! and their own address matches them.
//!
//! An anchored (inline, above-line or custom) frame lives on the paragraph
//! that anchors it, not in its spread's item lists, and the selector index
//! only walked the spreads. So `paged.query("textFrame")` and
//! `paged.query("rectangle")` never named one, and even its own address
//! (`rectangle:<id>`) matched nothing. They are listed under their story
//! now.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

fn load(sample: &str) -> CanvasModel {
    let s = paged_gen::samples::build(sample).expect("sample");
    let bytes = paged_gen::write_idml(&s).expect("write idml");
    CanvasModel::load(sample, &bytes, CanvasOptions::default()).expect("load")
}

fn run(model: &mut CanvasModel, src: &str) -> Vec<String> {
    let r = execute_script(model, src);
    assert!(r.error.is_none(), "script error: {:?}", r.error);
    r.output
}

/// `(kind, id)` of every anchored frame in the document, nested ones too.
fn anchored(m: &CanvasModel) -> Vec<(&'static str, String)> {
    fn walk(fs: &[paged_model::AnchoredFrame], out: &mut Vec<(&'static str, String)>) {
        for f in fs {
            let kind = match f.frame_kind {
                paged_model::AnchoredFrameKind::TextFrame => "textFrame",
                paged_model::AnchoredFrameKind::Rectangle => "rectangle",
                paged_model::AnchoredFrameKind::Group => "group",
            };
            if let Some(id) = &f.self_id {
                out.push((kind, id.clone()));
            }
            walk(&f.children, out);
        }
    }
    let mut out = Vec::new();
    for s in &m.scene().stories {
        for p in &s.story.paragraphs {
            walk(&p.anchored_frames, &mut out);
        }
    }
    out
}

#[test]
fn kind_selectors_list_anchored_frames() {
    for (sample, kind) in [("anchored", "textFrame"), ("inline-objects", "rectangle")] {
        let mut m = load(sample);
        let want: Vec<String> = anchored(&m)
            .into_iter()
            .filter(|(k, _)| *k == kind)
            .map(|(k, id)| format!("{k}:{id}"))
            .collect();
        assert!(!want.is_empty(), "{sample}: the sample anchors {kind}s");
        let out = run(
            &mut m,
            &format!(
                r#"const all = JSON.parse(paged.query("{kind}"));
                   const want = {want:?};
                   console.log(want.filter(a => !all.includes(a)).length);
                   const one = JSON.parse(paged.query(want[0]));
                   console.log(one.length === 1 && one[0] === want[0]);
                   console.log(JSON.parse(paged.query("frame")).includes(want[0]));"#
            ),
        );
        assert_eq!(
            out,
            ["[log] 0", "[log] true", "[log] true"],
            "{sample}: every anchored {kind} is listed and its address resolves"
        );
    }
}

#[test]
fn an_anchored_frame_sits_under_its_story() {
    let mut m = load("inline-objects");
    let (_, id) = anchored(&m)
        .into_iter()
        .find(|(k, _)| *k == "rectangle")
        .expect("an anchored rectangle");
    let story = m
        .scene()
        .stories
        .iter()
        .find(|s| {
            s.story.paragraphs.iter().any(|p| {
                p.anchored_frames
                    .iter()
                    .any(|f| f.self_id.as_deref() == Some(id.as_str()))
            })
        })
        .map(|s| s.self_id.clone())
        .unwrap();
    let out = run(
        &mut m,
        &format!(
            r#"const kids = JSON.parse(paged.query("story:{story} > rectangle"));
               console.log(kids.includes("rectangle:{id}"));"#
        ),
    );
    assert_eq!(out, ["[log] true"]);
}
