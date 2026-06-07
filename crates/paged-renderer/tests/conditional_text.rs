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

//! W4.3 — end-to-end conditional-text visibility filtering.
//!
//! Renders the `conditions` gen sample (the first generated fixture to
//! carry populated `<Condition>` defs — closing the W2.14 honest gap)
//! and asserts the renderer DROPS runs gated by a `Visible="false"`
//! condition before layout, while ungated and `Visible="true"`-gated
//! runs still emit glyphs. This exercises the real
//! `emit_paragraph_into_chain` drop path against a parsed document —
//! the inline `conditions_*` unit tests in `pipeline/mod.rs` only
//! mirror the rule against a hand-built run list.

use std::path::PathBuf;

use paged_gen::samples::conditions;
use paged_renderer::{pipeline, Document, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// All glyph-run unicode on a page, joined in command order with
/// whitespace dropped (whitespace glyphs carry no unicode).
fn glyph_text(page: &paged_renderer::BuiltPage) -> String {
    let table = page
        .list
        .glyph_runs
        .as_ref()
        .expect("collect_glyph_runs must be on");
    let mut entries: Vec<_> = table.entries.iter().collect();
    entries.sort_by_key(|e| e.command_index);
    entries
        .iter()
        .filter_map(|e| e.unicode)
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn build() -> paged_renderer::BuiltDocument {
    let sample = conditions::build();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = Document::open(&bytes).unwrap();
    let font = read_font("Inter.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

#[test]
fn hidden_condition_drops_run_while_visible_and_ungated_render() {
    let built = build();
    let text = glyph_text(&built.pages[0]);

    // The ungated run ("ALWAYS") always renders.
    assert!(
        text.contains(conditions::UNGATED_TEXT),
        "ungated run {:?} must render; got {text:?}",
        conditions::UNGATED_TEXT,
    );
    // The Visible="true"-gated run ("SHOWME") renders.
    assert!(
        text.contains(conditions::VISIBLE_TEXT),
        "Visible-gated run {:?} must render; got {text:?}",
        conditions::VISIBLE_TEXT,
    );
    // The Visible="false"-gated run ("DROPME") must NOT appear — the
    // renderer drops it before layout.
    assert!(
        !text.contains(conditions::HIDDEN_TEXT),
        "Hidden-gated run {:?} must be dropped pre-layout; leaked into {text:?}",
        conditions::HIDDEN_TEXT,
    );
    // W4.8 — the multi-gated run ("BOTHME") carries BOTH conditions;
    // the renderer keeps a run only when EVERY applied condition is
    // visible, so it is dropped (Hidden is invisible). This is the
    // end-to-end exercise of the multiple-conditions-per-run AND rule.
    assert!(
        !text.contains(conditions::MULTI_TEXT),
        "multi-gated run {:?} must be dropped (one condition is hidden); leaked into {text:?}",
        conditions::MULTI_TEXT,
    );
}
