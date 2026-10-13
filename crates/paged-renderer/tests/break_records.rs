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

//! The break records (`paged-inspect --emit-breaks`) say what a line prints
//! that its story does not hold — a list's marker, a tab's leader, a page
//! or footnote number. The text gate (`corpus/generated/text-diff.sh`)
//! reads them: it drops a marker on both sides and prints a leader on both,
//! instead of guessing either from InDesign's PDF alone.

use std::path::PathBuf;

use paged_renderer::pipeline::{BreakRecord, GeneratedKind, GeneratedSpan};
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
        .find(|b| b.source_text == text)
        .unwrap_or_else(|| {
            let all: Vec<_> = breaks.iter().map(|b| b.source_text.as_str()).collect();
            panic!("no line {text:?} in {all:?}")
        })
}

/// Each span as `(kind, the source text it covers)`.
fn spans(r: &BreakRecord) -> Vec<(GeneratedKind, &str)> {
    r.generated
        .iter()
        .map(|g| (g.kind, &r.source_text[g.start as usize..g.end as usize]))
        .collect()
}

/// The marker is the number or bullet with the text after it, whatever
/// `BulletsTextAfter` is (`list-overrides`): nothing, a tab, `>`.
#[test]
fn a_list_marker_is_marked_with_the_text_after_it() {
    use GeneratedKind::ListMarker;
    let b = breaks(paged_gen::samples::list_overrides::build());
    assert_eq!(spans(line(&b, "1.o02 one")), [(ListMarker, "1.")]);
    assert_eq!(spans(line(&b, "2.o02 two")), [(ListMarker, "2.")]);
    assert_eq!(spans(line(&b, "(1)\to03 one")), [(ListMarker, "(1)\t")]);
    assert_eq!(spans(line(&b, "•>o08 one")), [(ListMarker, "•>")]);
    assert_eq!(spans(line(&b, "II.\to12 two")), [(ListMarker, "II.\t")]);
    // A label is story text only.
    assert!(line(&b, "local after ^t (= c01)").generated.is_empty());
    // Every list line is its marker and then the story's text.
    let listed: Vec<_> = b.iter().filter(|r| !r.generated.is_empty()).collect();
    assert_eq!(listed.len(), 44, "22 cases, two items each");
    for r in listed {
        let g = &r.generated[0];
        assert_eq!((g.kind, g.start, g.text.as_deref()), (ListMarker, 0, None));
        let rest = &r.source_text[g.end as usize..];
        assert!(
            rest.starts_with('o') && (rest.ends_with(" one") || rest.ends_with(" two")),
            "{:?} past its marker is {rest:?}",
            r.source_text
        );
    }
}

/// A leader is drawn across its tab, so the record marks the tab and says
/// what prints there: as many whole dots as InDesign's PDF of
/// `text-advanced` carries on each line (17, 21, 21).
#[test]
fn a_tab_leader_is_marked_on_its_tab() {
    let b = breaks(paged_gen::samples::text_advanced::build());
    for (text, dots) in [
        ("Apples\t1.20\t10\t12.00", 17),
        ("Bread\t3.50\t2\t7.00", 21),
        ("Cheese\t8.99\t1\t8.99", 21),
    ] {
        let r = line(&b, text);
        assert_eq!(spans(r), [(GeneratedKind::TabLeader, "\t")], "{text:?}");
        let g = &r.generated[0];
        // The third tab: the one whose stop carries `Leader="."`.
        assert_eq!(g.start as usize, text.rfind('\t').unwrap(), "{text:?}");
        assert_eq!(
            g.text.as_deref(),
            Some(".".repeat(dots).as_str()),
            "{text:?}"
        );
    }
}

/// Page numbers, section markers, text variables and footnote numbers
/// print on both sides; they are marked so a reader can tell them from the
/// story's text.
#[test]
fn page_numbers_variables_and_footnote_numbers_are_marked() {
    use GeneratedKind::*;
    let b = breaks(paged_gen::samples::variables::build());
    let r = line(&b, "Page 2 of 3 | section ends at 2 | roman III");
    assert_eq!(
        spans(r),
        [
            (PageNumber, "2"),
            (TextVariable, "3"),
            (TextVariable, "2"),
            (TextVariable, "III")
        ]
    );
    let r = line(&b, "Section: Part One | chapter 1 | Edition 7");
    assert_eq!(
        spans(r),
        [
            (SectionMarker, "Part One"),
            (TextVariable, "1"),
            (TextVariable, "Edition 7")
        ]
    );
    let r = line(&b, "Jump: continued on page 3, previous 1.");
    assert_eq!(spans(r), [(PageNumber, "3"), (PageNumber, "1")]);

    let b = breaks(paged_gen::samples::footnote_references::build());
    let r = line(&b, "3 Two in one paragraph: first5 and second6 here.");
    assert_eq!(spans(r), [(FootnoteNumber, "5"), (FootnoteNumber, "6")]);
    // The note opens with its number.
    let note = line(&b, "5\tThe first of two.");
    assert!(note.story_id.contains("#footnote"));
    assert_eq!(spans(note), [(FootnoteNumber, "5")]);
}

/// The JSON a line without generated text writes is the one it wrote
/// before the fields existed; one with spans names their kind in snake
/// case.
#[test]
fn the_json_adds_fields_only_where_they_apply() {
    let mut r = BreakRecord {
        story_id: "u1".into(),
        paragraph_idx: 0,
        line_idx: 0,
        page_idx: 0,
        frame_idx: 0,
        first_byte: 0,
        last_byte: 9,
        baseline_y_pt: 12.0,
        width_pt: 40.0,
        source_text: "1.\tone two".into(),
        generated: Vec::new(),
    };
    let plain = serde_json::to_value(&r).unwrap();
    let mut keys: Vec<_> = plain.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "baseline_y_pt",
            "first_byte",
            "frame_idx",
            "last_byte",
            "line_idx",
            "page_idx",
            "paragraph_idx",
            "source_text",
            "story_id",
            "width_pt"
        ]
    );
    r.generated = vec![
        GeneratedSpan {
            kind: GeneratedKind::ListMarker,
            start: 0,
            end: 3,
            text: None,
        },
        GeneratedSpan {
            kind: GeneratedKind::TabLeader,
            start: 6,
            end: 7,
            text: Some("...".into()),
        },
    ];
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v["generated"],
        serde_json::json!([
            {"kind": "list_marker", "start": 0, "end": 3},
            {"kind": "tab_leader", "start": 6, "end": 7, "text": "..."}
        ])
    );
}
