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

//! A paragraph is hyphenated with the dictionary its own
//! `AppliedLanguage` names.
//!
//! The parser has read, cascaded and stored `AppliedLanguage` for a
//! long time; until now nothing downstream looked at it, so every
//! paragraph of every document was hyphenated as American English. This
//! is the end-to-end guard for that class of bug — a value that is
//! carried the whole way and then dropped at the last seam.
//!
//! `column` is the discriminator: the American patterns break it
//! `col-umn` and the British ones do not break it at all. Set in a
//! measure too narrow for the whole word, the two dictionaries
//! therefore produce a different number of lines from identical text.

use std::path::PathBuf;

use paged_gen::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml,
};
use paged_gen::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use paged_gen::geometry::translate;
use paged_gen::package::Sample;
use paged_renderer::{pipeline, PipelineOptions};

const PAGE_W_PT: f32 = 400.0;
const PAGE_H_PT: f32 = 400.0;
/// Narrow enough that "column" cannot finish the line it starts on.
const FRAME_W_PT: f32 = 28.0;
const FRAME_H_PT: f32 = 300.0;
const BODY: &str = "typeface typeface typeface";

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// One page, one frame, one paragraph, tagged with `language`.
fn sample(language: &'static str) -> Sample {
    sample_w(language, FRAME_W_PT)
}

fn sample_w(language: &'static str, frame_w: f32) -> Sample {
    let story_id = "lang_story".to_string();
    let frame_id = "lang_frame".to_string();
    let master_id = "lang_master".to_string();
    let spread_id = "lang_spread".to_string();

    let story_bytes = write_story(&Story {
        self_id: story_id.clone(),
        paragraphs: vec![Paragraph {
            justification: Some("LeftJustified"),
            runs: vec![Run {
                text: BODY.to_string(),
                point_size: Some(11.0),
                fill_color: Some("Color/Black".to_string()),
                font_style: None,
                tracking: None,
                baseline_shift: None,
                underline: None,
                applied_font: None,
                anchored_frame: None,
                extra_char_attrs: vec![("AppliedLanguage", language)],
            }],
            ..Paragraph::plain("")
        }
        .with_para_attrs(vec![
            ("Hyphenation", "true"),
            ("HyphenateWordsLongerThan", "5"),
        ])],
        extra_story_attrs: Vec::new(),
    });

    let mut frame = Rect::filled(frame_id, frame_w, FRAME_H_PT, translate(20.0, 20.0));
    frame.fill_color = None;
    frame.parent_story = Some(story_id.clone());

    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: "lang_master_page".to_string(),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let spread_bytes = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: "lang_page".to_string(),
        page_name: "language".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![frame.into()],
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: vec![story_id.clone()],
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master_bytes)],
        spreads: vec![(spread_id, spread_bytes)],
        stories: vec![(story_id, story_bytes)],
    }
}

/// Lines the renderer composed for the single story.
fn line_count(language: &'static str) -> usize {
    let bytes = paged_gen::write_idml(&sample(language)).expect("write idml");
    let document = idml_import::import_idml_doc(&bytes).expect("parse idml");
    let font = read_font("OpenSans.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build");
    built.story_layout("lang_story").len()
}

#[test]
fn a_run_is_hyphenated_by_the_language_it_names() {
    let us = line_count("$ID/English: USA");
    let gb = line_count("$ID/English: UK");
    assert_eq!(
        us, 6,
        "American patterns break type-face, so each word takes two lines"
    );
    assert_eq!(
        gb, 3,
        "British patterns do not break `typeface` at all, so each word \
         stays on one line and overhangs the measure"
    );
}

/// A language we hold no patterns for must not be hyphenated with
/// English ones — it composes as if hyphenation were off.
#[test]
fn a_language_we_have_no_patterns_for_is_not_hyphenated_as_english() {
    let english = line_count("$ID/English: USA");
    let russian = line_count("$ID/Russian");
    let none = line_count("$ID/[No Language]");
    assert_eq!(
        russian, none,
        "a language with no dictionary of ours must compose like no \
         dictionary at all"
    );
    assert_ne!(
        russian, english,
        "...and specifically NOT like American English, which is what \
         every document got before the language was read"
    );
}
