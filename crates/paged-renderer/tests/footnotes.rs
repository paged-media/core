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

//! W1.8 — footnotes v2 at render: separator rule, per-run styling, and
//! the too-tall-footnote overflow diagnostic.
//!
//! Renders the `footnotes` gen sample (one A4 page: a body paragraph
//! anchoring three footnotes — plain / styled / too-tall — with a
//! document `<FootnoteOption>` separator rule turned on) and asserts:
//!
//!   1. the renderer draws a horizontal **separator rule** (a StrokePath)
//!      at the FootnoteOption geometry (1pt weight, 140pt wide, left
//!      indent 0) above the footnote pool;
//!   2. the styled footnote's body produces glyph runs at **more than one
//!      point size** (8pt body + a 10pt inline phrase) — proof footnotes
//!      now compose through the per-run shaping path;
//!   3. the too-tall third footnote trips `DiagnosticCode::FootnoteOverflow`.

use std::path::PathBuf;

use paged_compose::{DisplayCommand, PathSegment};
use paged_renderer::{pipeline, DiagnosticCode, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

fn build_footnotes() -> paged_renderer::BuiltDocument {
    let sample = paged_gen::samples::footnotes::build();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = paged_parse::import_idml_doc(&bytes).unwrap();

    let font = read_font("OpenSans.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

/// A horizontal StrokePath line: returns `(x0, x1, y, width)` for any
/// command whose path is a single MoveTo→LineTo at constant y.
fn horizontal_strokes(page: &paged_renderer::BuiltPage) -> Vec<(f32, f32, f32, f32)> {
    let mut out = Vec::new();
    for cmd in &page.list.commands {
        let DisplayCommand::StrokePath {
            path_id, stroke, ..
        } = cmd
        else {
            continue;
        };
        let Some(path) = page.list.paths.get(*path_id) else {
            continue;
        };
        if path.segments.len() != 2 {
            continue;
        }
        if let (PathSegment::MoveTo { x: x0, y: y0 }, PathSegment::LineTo { x: x1, y: y1 }) =
            (&path.segments[0], &path.segments[1])
        {
            if (y0 - y1).abs() < 0.01 {
                out.push((*x0, *x1, *y0, stroke.width));
            }
        }
    }
    out
}

#[test]
fn footnote_separator_rule_is_drawn_at_option_geometry() {
    let built = build_footnotes();
    let page = &built.pages[0];

    // Frame: 360pt wide, centred on a 595.276pt page → left edge at
    // (595.276 - 360)/2 = 117.638pt. FootnoteOption RuleLeftIndent=0,
    // RuleWidth=140 → the rule runs from x≈117.6 to x≈257.6.
    let frame_left = (595.276 - 360.0) / 2.0;
    let lines = horizontal_strokes(page);
    let rule = lines
        .iter()
        .find(|(x0, x1, _y, w)| {
            (x0 - frame_left).abs() < 1.0
                && ((x1 - x0) - 140.0).abs() < 1.0
                && (*w - 1.0).abs() < 0.1
        })
        .unwrap_or_else(|| {
            panic!("no footnote separator rule at expected geometry; horizontal strokes: {lines:?}")
        });

    // The rule sits in the lower half of the frame (above the pool), not
    // at the very top where the body text starts.
    let (_, _, rule_y, _) = *rule;
    assert!(
        rule_y > 100.0,
        "separator rule y={rule_y:.1} should sit below the body, above the pool"
    );
}

#[test]
fn footnote_body_runs_render_at_multiple_point_sizes() {
    let built = build_footnotes();
    let page = &built.pages[0];
    let table = page
        .list
        .glyph_runs
        .as_ref()
        .expect("collect_glyph_runs must be on");

    // The body paragraph + the footnotes inherit 12pt from the default
    // `[No paragraph style]`; the styled footnote carries one explicit
    // 10pt run ("larger phrase"). A 10pt glyph run can ONLY come from
    // that styled footnote run (nothing else in the document is 10pt),
    // so its presence proves footnote bodies compose through the per-run
    // shaping path — a flat single-size footnote path could never emit
    // it. We also confirm ≥2 distinct sizes overall.
    let mut sizes: Vec<f32> = table.entries.iter().map(|e| e.font_size).collect();
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sizes.dedup_by(|a, b| (*a - *b).abs() < 0.05);

    assert!(
        sizes.len() >= 2,
        "expected ≥2 distinct point sizes across the page (12pt inherited + \
         10pt styled footnote run); got {sizes:?}"
    );
    assert!(
        sizes.iter().any(|s| (*s - 10.0).abs() < 0.2),
        "expected a 10pt styled footnote run (per-run footnote styling); \
         sizes={sizes:?}"
    );
    assert!(
        sizes.iter().any(|s| (*s - 12.0).abs() < 0.2),
        "expected the 12pt inherited size; sizes={sizes:?}"
    );
}

#[test]
fn too_tall_footnote_fires_overflow_diagnostic() {
    let built = build_footnotes();
    let fired = built
        .diagnostics
        .items
        .iter()
        .any(|d| d.code == DiagnosticCode::FootnoteOverflow);
    assert!(
        fired,
        "the deliberately too-tall third footnote should fire FootnoteOverflow; \
         diagnostics: {:?}",
        built
            .diagnostics
            .items
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>()
    );
}
