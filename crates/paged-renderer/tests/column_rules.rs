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

//! Column rules (ADR 155), pinned against InDesign 20.0.1's PDF of
//! `column-rules.idml`, read from its stroke operators
//! (`tools/indesign-export/pdf-strokes.py`; tolerance 0.05 pt).
//!
//! What InDesign answered: one rule per gutter, at the gutter's centre
//! moved by `ColumnRuleOffset`, from the text area's top moved down by
//! the top inset to the frame's deepest line — its baseline plus its
//! face's descent — moved up by the bottom inset. A frame with one
//! column draws none.

use std::path::PathBuf;

use paged_compose::{DisplayCommand, PathSegment};
use paged_gen::samples::column_rules as sample;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

const TOL: f32 = 0.05;

/// InDesign's rules, page pt from the page top: `(x, top, bottom, width)`.
const INDESIGN: [(f32, f32, f32, f32); 8] = [
    (120.0, 72.0, 252.895, 1.0),  // 1 the default rule
    (276.5, 72.0, 252.895, 0.5),  // 2 three columns
    (335.5, 72.0, 252.895, 0.5),  // 2
    (120.0, 312.0, 456.895, 1.0), // 4 insets 12 / 24
    (310.0, 300.0, 480.895, 1.0), // 5 offset 4
    (492.0, 292.0, 488.895, 1.0), // 6 insets -8
    (120.0, 538.0, 698.895, 1.0), // 7 frame inset 10
    (306.0, 528.0, 568.895, 1.0), // 8 three lines only
];

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

/// Every vertical stroke on the page: `(x, top, bottom, width)`.
fn vertical_strokes() -> Vec<(f32, f32, f32, f32)> {
    let bytes = paged_gen::write_idml(&sample::build()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build_document");
    let list = &built.pages[0].list;
    let mut out = Vec::new();
    for cmd in &list.commands {
        let DisplayCommand::StrokePath {
            path_id,
            stroke,
            transform,
            ..
        } = cmd
        else {
            continue;
        };
        let Some(path) = list.paths.get(*path_id) else {
            continue;
        };
        let points: Vec<(f32, f32)> = path
            .segments
            .iter()
            .filter_map(|s| match *s {
                PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => {
                    Some(transform.apply(x, y))
                }
                _ => None,
            })
            .collect();
        let Some(&(x, _)) = points.first() else {
            continue;
        };
        if points.len() < 2 || points.iter().any(|p| (p.0 - x).abs() > 0.01) {
            continue;
        }
        let top = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let bottom = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        out.push((x, top, bottom, stroke.width));
    }
    out
}

#[test]
fn every_rule_stands_where_indesign_draws_it() {
    let strokes = vertical_strokes();
    for (case, want) in INDESIGN.iter().enumerate() {
        let hit = strokes.iter().find(|s| {
            (s.0 - want.0).abs() < TOL
                && (s.1 - want.1).abs() < TOL
                && (s.2 - want.2).abs() < TOL
                && (s.3 - want.3).abs() < 1e-3
        });
        assert!(
            hit.is_some(),
            "rule {case} {want:?} not drawn; vertical strokes: {strokes:?}"
        );
    }
}

#[test]
fn a_dashed_rule_spans_the_same_length_and_one_column_draws_none() {
    let strokes = vertical_strokes();
    // 3 — the 2 pt Dashed rule: its dashes run from the frame's top to
    // the deepest line, as the solid rule does.
    let dashes: Vec<_> = strokes
        .iter()
        .filter(|s| (s.0 - 492.0).abs() < TOL && s.2 < 260.0)
        .collect();
    assert!(
        !dashes.is_empty() && dashes.iter().all(|s| s.3 == 2.0),
        "{dashes:?}"
    );
    let top = dashes.iter().map(|s| s.1).fold(f32::MAX, f32::min);
    let bottom = dashes.iter().map(|s| s.2).fold(f32::MIN, f32::max);
    assert!(
        (top - 72.0).abs() < TOL && (bottom - 252.895).abs() < TOL,
        "{top} .. {bottom}"
    );
    // 9 — one column: nothing in its frame (408..576 × 528..708).
    assert!(
        !strokes
            .iter()
            .any(|s| s.0 > 408.0 && s.0 < 576.0 && s.1 >= 520.0),
        "{strokes:?}"
    );
}
