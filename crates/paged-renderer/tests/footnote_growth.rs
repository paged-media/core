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

//! A growing story (ADR 026) grows a page for a note that has no column
//! or frame left to continue in (ADR 178), as Word paginates its notes.
//!
//! Over `footnote-split.idml`'s first paragraph, whose note needs eleven
//! lines where its frame holds nine (ADR 161): with the chain cut to that
//! one frame, the body text fits and only the note overflows.

use std::path::PathBuf;

use paged_gen::samples::footnote_split::{self as sample};
use paged_renderer::diagnostics::DiagnosticCode;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

fn one_frame_document(grow: bool) -> paged_scene::Document {
    let bytes = paged_gen::write_idml(&sample::build(&sample::SPLIT)).expect("write_idml");
    let mut doc = idml_import::import_idml_doc(&bytes).expect("open");
    let story_id = sample::story_id(sample::SPLIT.sample);
    let story = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == story_id)
        .expect("body story");
    story.story.paragraphs.truncate(1);
    if grow {
        story.story.grow = Some(paged_model::FlowGrowRule::default());
    }
    // The chain ends in frame 1.
    for s in &mut doc.spreads {
        let frames = &mut s.spread.text_frames;
        let next = frames
            .iter()
            .find_map(|f| f.next_text_frame.clone())
            .expect("frame 1 threads to frame 2");
        frames.retain(|f| f.self_id.as_deref() != Some(next.as_str()));
        for f in frames.iter_mut() {
            f.next_text_frame = None;
        }
    }
    doc
}

fn build(doc: &paged_scene::Document) -> pipeline::BuiltDocument {
    let mut resolver = BytesResolver::new();
    let inter = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    resolver.add_font("Inter", None, std::fs::read(inter).expect("Inter"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        collect_breaks: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(doc, &opts).expect("build")
}

/// The words the note's lines set, in order, its printed number left out.
fn note_words(built: &pipeline::BuiltDocument) -> Vec<String> {
    let mut words: Vec<String> = built
        .breaks
        .iter()
        .filter(|r| r.story_id.ends_with("#footnote1"))
        .flat_map(|r| r.source_text.split_whitespace().map(str::to_string))
        .collect();
    if words
        .first()
        .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))
    {
        words.remove(0);
    }
    words
}

fn expected_note_words() -> Vec<String> {
    sample::paragraphs()[0]
        .1
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn note_lines(built: &pipeline::BuiltDocument) -> usize {
    built
        .breaks
        .iter()
        .filter(|r| r.story_id.ends_with("#footnote1"))
        .count()
}

#[test]
fn a_note_with_nowhere_to_continue_is_overset_without_a_grow_rule() {
    let built = build(&one_frame_document(false));
    assert_eq!(built.pages.len(), 1);
    assert!(built
        .diagnostics
        .items
        .iter()
        .any(|d| d.code == DiagnosticCode::FootnoteOverflow));
    assert_eq!(note_lines(&built), 9, "the lines that fit frame 1");
}

#[test]
fn a_growing_story_grows_a_page_for_its_note() {
    let built = build(&one_frame_document(true));
    assert_eq!(built.pages.len(), 2, "one page grown for the continuation");
    assert!(
        !built
            .diagnostics
            .items
            .iter()
            .any(|d| d.code == DiagnosticCode::FootnoteOverflow),
        "the note continues on the grown page"
    );
    // The two lines the note keeps no room for in frame 1 continue on
    // the grown page, recomposed at its width (the page's margin box,
    // wider than frame 1): every word of the note is set, once.
    // (Compared without spaces: a hyphenated word spans two lines.)
    assert_eq!(note_words(&built).concat(), expected_note_words().concat());
    assert!(note_lines(&built) > 9, "the continuation is set");
    let story_id = sample::story_id(sample::SPLIT.sample);
    let grown = paged_scene::grow::generated_page_id(&story_id, 1);
    assert!(built.pages.iter().any(|p| p.id.as_str() == grown));
}
