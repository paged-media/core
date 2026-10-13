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

//! Tables and footnote references are characters of their paragraph, as
//! InDesign 20.0.1 sets them (`table-inline-text`, `footnote-references`):
//! the text after a table stays a line of the table's paragraph, and a
//! reference prints its note's number where it stands.

use std::path::PathBuf;

use paged_renderer::pipeline::BreakRecord;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// The break records of a gen sample, set in the faces its gate uses.
fn breaks(sample: paged_gen::Sample) -> Vec<BreakRecord> {
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let font = read_font("SourceSerif4.ttf");
    let mut fonts = BytesResolver::new();
    fonts.add_font("Inter", None, read_font("Inter.ttf"));
    fonts.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = PipelineOptions {
        font: Some(&font),
        assets: Some(&fonts),
        collect_breaks: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap().breaks
}

fn line<'a>(breaks: &'a [BreakRecord], text: &str) -> &'a BreakRecord {
    breaks
        .iter()
        .find(|b| b.source_text.trim() == text)
        .unwrap_or_else(|| {
            let all: Vec<_> = breaks.iter().map(|b| b.source_text.as_str()).collect();
            panic!("no line {text:?} in {all:?}")
        })
}

/// Case 1: `Before table <Table/>After table` in one range is ONE
/// paragraph whose table stands on a line of its own; case 2, with a
/// paragraph mark either side, is three.
#[test]
fn text_after_a_table_in_the_same_range_stays_in_its_paragraph() {
    let b = breaks(paged_gen::samples::table_inline_text::build());
    let before = line(&b, "Before table");
    let after = b
        .iter()
        .find(|r| r.story_id == before.story_id && r.source_text.trim() == "After table")
        .expect("the text after the table");
    assert_eq!(
        before.paragraph_idx, after.paragraph_idx,
        "{before:?} / {after:?}"
    );
    // Below the table, in the same frame. (A break record's line index
    // counts within the piece of a split paragraph, as for a forced line
    // break; the caret mapping adds the piece's base.)
    assert!(
        after.frame_idx == before.frame_idx && after.baseline_y_pt > before.baseline_y_pt + 20.0,
        "{before:?} / {after:?}"
    );
}

/// Case 4: the numbered paragraph's text after the table takes no number,
/// so the next item is number 2 (InDesign's PDF: "2. Second item").
#[test]
fn a_table_does_not_restart_or_advance_the_list() {
    let b = breaks(paged_gen::samples::table_inline_text::build());
    let second = b
        .iter()
        .find(|r| r.source_text.contains("Second item"))
        .expect("the second item");
    let first = b
        .iter()
        .find(|r| r.story_id == second.story_id && r.source_text.contains("Before table"))
        .expect("the first item");
    assert_eq!(second.paragraph_idx, first.paragraph_idx + 1);
}

/// A reference is a character of its line and prints its note's label:
/// `footnote-references` starts its notes at 3, and the reference lines
/// carry 3, 4, ... where InDesign prints them.
#[test]
fn a_reference_prints_its_notes_number_where_it_stands() {
    let b = breaks(paged_gen::samples::footnote_references::build());
    let notes: Vec<&BreakRecord> = b
        .iter()
        .filter(|r| r.story_id.contains("#footnote"))
        .collect();
    assert!(!notes.is_empty(), "the notes are set");
    assert!(
        notes[0].source_text.trim_start().starts_with('3'),
        "the first note opens with its number, StartAt 3: {:?}",
        notes[0].source_text
    );
}
