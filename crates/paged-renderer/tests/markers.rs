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

//! W1.4 — marker / variable / link resolution at render.
//!
//! Renders the `markers` gen sample and asserts that (a) text-variable
//! instances are re-resolved into the glyph stream (custom literal +
//! real page count, not the stale baked value), and (b) the
//! `LinkRegionTable` captures one region per hyperlink span with the
//! right target + a sane page-space rect.

use std::path::PathBuf;

use paged_compose::LinkTarget;
use paged_renderer::{pipeline, Document, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Assemble the page's glyph-run unicode into a single string in
/// command (reading) order.
fn glyph_text(page: &paged_renderer::BuiltPage) -> String {
    let table = page
        .list
        .glyph_runs
        .as_ref()
        .expect("collect_glyph_runs must be on");
    let mut entries: Vec<_> = table.entries.iter().collect();
    entries.sort_by_key(|e| e.command_index);
    entries.iter().filter_map(|e| e.unicode).collect()
}

fn build_markers() -> paged_renderer::BuiltDocument {
    let sample = paged_gen::samples::markers::build();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = Document::open(&bytes).unwrap();

    let font = read_font("Inter.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        collect_link_regions: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

#[test]
fn custom_and_page_count_variables_substitute_into_glyphs() {
    let built = build_markers();
    // All body text lands on the first page. The glyph-run side-channel
    // captures one unicode char per rendered glyph; whitespace glyphs
    // carry no unicode, so compare against a space-stripped string.
    let text: String = glyph_text(&built.pages[0])
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // Custom variable resolves to its literal Contents.
    assert!(
        text.contains("Spring2026"),
        "custom-text variable should resolve to 'Spring 2026'; got {text:?}"
    );
    // Page-count variable resolves to the REAL total (2 pages), not the
    // stale baked ResultText ("1").
    assert!(
        text.contains("Pages:2"),
        "page-count variable should resolve to the real count '2'; got {text:?}"
    );
    // The stale baked page count must NOT survive — guard against the
    // resolver silently no-opping. "Pages:1" would mean the baked value
    // leaked through.
    assert!(
        !text.contains("Pages:1"),
        "stale baked page count leaked: {text:?}"
    );
}

#[test]
fn hyperlink_spans_become_link_regions() {
    let built = build_markers();
    let table = built.pages[0]
        .list
        .link_regions
        .as_ref()
        .expect("collect_link_regions must be on");
    // Two link spans: a URL and a page destination.
    assert_eq!(
        table.regions.len(),
        2,
        "expected two link regions; got {}",
        table.regions.len()
    );

    let has_url = table
        .regions
        .iter()
        .any(|r| matches!(&r.target, LinkTarget::Url(u) if u == "https://paged.media"));
    assert!(has_url, "expected a URL link region to paged.media");

    // The page destination targets the SECOND page (flat index 1).
    let has_page = table
        .regions
        .iter()
        .any(|r| matches!(r.target, LinkTarget::PageIndex(1)));
    assert!(has_page, "expected a page link region targeting page index 1");

    // Rect sanity: every region has a positive area and sits within the
    // page bounds.
    let (pw, ph) = (built.pages[0].width_pt, built.pages[0].height_pt);
    for region in &table.regions {
        let r = region.rect;
        assert!(r.w > 0.0 && r.h > 0.0, "link rect must have area: {r:?}");
        assert!(
            r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= pw + 1.0 && r.y + r.h <= ph + 1.0,
            "link rect must sit on the page ({pw}x{ph}): {r:?}"
        );
    }

    // Page 2 (the link target) carries no link regions of its own.
    if let Some(t2) = built.pages[1].list.link_regions.as_ref() {
        assert!(t2.regions.is_empty(), "target page should have no links");
    }
}
