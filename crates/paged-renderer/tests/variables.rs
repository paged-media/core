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

//! W1.18 + W1.19 — live text-variable + cross-reference resolution.
//!
//! Renders the `variables` gen sample and asserts that the deferred
//! variable / xref kinds resolve against the CURRENT model + layout:
//!
//!   * CreationDate honours its `Format` against the injected clock.
//!   * ChapterNumber resolves from the `<Section>` numbering ("II").
//!   * RunningHeader picks up the per-PAGE heading (page 1 → "Chapter
//!     One", page 2 → "Chapter Two").
//!   * a cross-reference resolves to the page its destination story
//!     landed on, and re-resolves when the destination MOVES.

use std::path::PathBuf;

use paged_compose::LinkTarget;
use paged_renderer::{pipeline, DateParts, Document, DocumentClock, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Assemble a page's glyph-run unicode into a single string in command
/// (reading) order, dropping whitespace (whitespace glyphs carry no
/// unicode) so substring asserts aren't sensitive to inter-word spacing.
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

fn build_with_clock(clock: DocumentClock) -> paged_renderer::BuiltDocument {
    let sample = paged_gen::samples::variables::build();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = Document::open(&bytes).unwrap();
    let font = read_font("Inter.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        collect_link_regions: true,
        document_clock: clock,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

/// A fixed clock: creation 2024-03-09, so "MMMM d, yyyy" → "March 9, 2024".
fn fixed_clock() -> DocumentClock {
    let creation = DateParts {
        year: 2024,
        month: 3,
        day: 9,
        hour: 10,
        minute: 0,
        second: 0,
    };
    DocumentClock {
        creation,
        modification: creation,
        output: creation,
    }
}

#[test]
fn creation_date_formats_from_clock() {
    let built = build_with_clock(fixed_clock());
    let text = glyph_text(&built.pages[0]);
    // "March 9, 2024" with whitespace stripped → "March9,2024".
    assert!(
        text.contains("March9,2024"),
        "CreationDate should format from the clock as 'March 9, 2024'; got {text:?}"
    );
    // The stale baked value must NOT survive.
    assert!(
        !text.contains("BAKED-DATE"),
        "stale baked CreationDate leaked: {text:?}"
    );
}

#[test]
fn output_date_is_deterministic_and_injectable() {
    // OutputDate reads the INJECTED `output` instant, never the wall
    // clock. Two renders with different output instants (creation held
    // fixed) yield different OutputDate text; the same instant always
    // yields the same text. The sample's OutputDate uses "yyyy-MM-dd".
    let mut clock_a = fixed_clock();
    clock_a.output = DateParts {
        year: 2030,
        month: 7,
        day: 4,
        hour: 0,
        minute: 0,
        second: 0,
    };
    let a = glyph_text(&build_with_clock(clock_a).pages[0]);
    assert!(
        a.contains("Output2030-07-04"),
        "OutputDate must follow the injected output instant; got {a:?}"
    );
    assert!(
        !a.contains("BAKED-OUT"),
        "stale baked OutputDate leaked: {a:?}"
    );

    // A different output instant → a different rendered OutputDate, while
    // the creation date (held fixed) is unchanged — proving OutputDate is
    // the injectable knob, not the wall clock.
    let mut clock_b = fixed_clock();
    clock_b.output = DateParts {
        year: 2031,
        month: 11,
        day: 25,
        ..clock_b.output
    };
    let b = glyph_text(&build_with_clock(clock_b).pages[0]);
    assert!(
        b.contains("Output2031-11-25"),
        "OutputDate must change with the injected instant; got {b:?}"
    );
    assert_ne!(a, b, "different output instants must render differently");

    // Determinism: same clock → byte-identical glyph text.
    assert_eq!(
        glyph_text(&build_with_clock(clock_a).pages[0]),
        a,
        "same clock must render the same output"
    );
}

#[test]
fn chapter_number_resolves_from_section() {
    let built = build_with_clock(fixed_clock());
    let text = glyph_text(&built.pages[0]);
    // Section numbering: UpperRoman, start 2 → chapter "II".
    assert!(
        text.contains("ChapterII"),
        "ChapterNumber should resolve to the section number 'II'; got {text:?}"
    );
    assert!(
        !text.contains("BAKED-CH"),
        "stale baked ChapterNumber leaked: {text:?}"
    );
}

#[test]
fn running_header_differs_per_page() {
    let built = build_with_clock(fixed_clock());
    let page0 = glyph_text(&built.pages[0]);
    let page1 = glyph_text(&built.pages[1]);
    // Page 1's running header (from the master frame) picks up page 1's
    // heading "Chapter One"; page 2 picks up "Chapter Two".
    assert!(
        page0.contains("ChapterOne"),
        "page 1 header should pick up 'Chapter One'; got {page0:?}"
    );
    assert!(
        page1.contains("ChapterTwo"),
        "page 2 header should pick up 'Chapter Two'; got {page1:?}"
    );
    // The header on page 2 must NOT carry page 1's heading (proves the
    // per-page boundary, not a single static pickup).
    assert!(
        !page1.contains("ChapterOne"),
        "page 2 header leaked page 1's heading: {page1:?}"
    );
    // The baked placeholder never survives once resolved.
    assert!(
        !page0.contains("BAKED-HDR") && !page1.contains("BAKED-HDR"),
        "stale baked running header leaked"
    );
}

#[test]
fn xref_resolves_to_destination_page() {
    let built = build_with_clock(fixed_clock());
    // The cross-reference source on page 1 targets the page-2 story; its
    // link region resolves to flat page index 1.
    let table = built.pages[0]
        .list
        .link_regions
        .as_ref()
        .expect("collect_link_regions must be on");
    let has_page1 = table
        .regions
        .iter()
        .any(|r| matches!(r.target, LinkTarget::PageIndex(1)));
    assert!(
        has_page1,
        "xref should resolve to the destination story's page (index 1); got {:?}",
        table.regions.iter().map(|r| &r.target).collect::<Vec<_>>()
    );
}

/// W1.19 — re-resolution when the destination MOVES. The `build_moved`
/// variant inserts a blank spread between page 1 and the destination
/// page, so the destination story shifts from flat page index 1 to 2. A
/// fresh render must re-resolve the xref to the NEW page — proving the
/// reference is materialised from current layout, not a parse-time
/// string.
#[test]
fn xref_re_resolves_when_destination_moves() {
    // Baseline: destination story lands on page index 1.
    let baseline = build_with_clock(fixed_clock());
    assert_eq!(
        destination_page_index(&baseline),
        Some(1),
        "baseline xref should target page index 1"
    );

    // Moved: a blank spacer page pushes the destination to index 2.
    let moved = build_moved_with_clock(fixed_clock());
    assert_eq!(
        destination_page_index(&moved),
        Some(2),
        "after moving the destination story, the xref must re-resolve to \
         its new page (index 2)"
    );
    assert!(
        moved.pages.len() > baseline.pages.len(),
        "moved variant should have an extra (blank) page"
    );
}

/// Pull the flat page index a `LinkTarget::PageIndex` xref resolves to
/// (the first such region across all pages).
fn destination_page_index(doc: &paged_renderer::BuiltDocument) -> Option<u32> {
    for page in &doc.pages {
        if let Some(table) = page.list.link_regions.as_ref() {
            for r in &table.regions {
                if let LinkTarget::PageIndex(idx) = r.target {
                    return Some(idx);
                }
            }
        }
    }
    None
}

fn build_moved_with_clock(clock: DocumentClock) -> paged_renderer::BuiltDocument {
    let sample = paged_gen::samples::variables::build_moved();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = Document::open(&bytes).unwrap();
    let font = read_font("Inter.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        collect_link_regions: true,
        document_clock: clock,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}
