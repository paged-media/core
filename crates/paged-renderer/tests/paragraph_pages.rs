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

//! L-06 — a heading in a story threaded across pages takes the page it
//! is laid out on. The scene alone can only name the story's head frame's
//! page; `BuiltDocument::paragraph_pages` says where each paragraph
//! landed, and `Document::resolve_toc_on` takes it.

use std::path::PathBuf;

use paged_gen::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use paged_gen::geometry::translate;
use paged_gen::package::Sample;
use paged_model::{TOCStyleDef, TOCStyleEntryDef};
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

const STORY: &str = "u_threaded";
const HEADING: &str = "ParagraphStyle/Heading";

fn paragraph(n: usize) -> Paragraph {
    let mut p = Paragraph::plain(format!("Paragraph {n} sets a few words of body copy."));
    p.runs[0].applied_font = Some("Inter");
    p.runs[0].point_size = Some(10.0);
    p.leading = Some(12.0);
    if n.is_multiple_of(10) {
        p.extra_paragraph_attrs = vec![("AppliedParagraphStyle", HEADING)];
    }
    p
}

fn frame(id: &str, prev: Option<&str>, next: Option<&str>) -> PageItem {
    Rect {
        self_id: id.to_string(),
        width_pt: 300.0,
        height_pt: 200.0,
        item_transform: translate(72.0, 72.0),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(STORY.to_string()),
        next_text_frame: next.map(str::to_string),
        previous_text_frame: prev.map(str::to_string),
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    }
    .into()
}

/// One story of 40 paragraphs threaded from a frame on page 1 to one on
/// page 2; every tenth paragraph is a heading.
fn sample() -> Sample {
    let story = write_story(&Story {
        extra_story_attrs: Vec::new(),
        self_id: STORY.to_string(),
        paragraphs: (0..40).map(paragraph).collect(),
    });
    let spread = |id: &str, page: &str, name: &str, item: PageItem| {
        (
            id.to_string(),
            write_spread(&Spread {
                self_id: id.to_string(),
                page_self_id: page.to_string(),
                page_name: name.to_string(),
                applied_master: "MasterSpread/m".to_string(),
                page_width_pt: 612.0,
                page_height_pt: 792.0,
                page_items: vec![item],
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )
    };
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec!["m".to_string()],
            spreads: vec!["s1".to_string(), "s2".to_string()],
            stories: vec![STORY.to_string()],
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(
            "m".to_string(),
            write_master(&Master {
                self_id: "MasterSpread/m".to_string(),
                page_self_id: "mp".to_string(),
                page_width_pt: 612.0,
                page_height_pt: 792.0,
                page_items: Vec::new(),
            }),
        )],
        spreads: vec![
            spread("s1", "p1", "1", frame("fa", None, Some("fb"))),
            spread("s2", "p2", "2", frame("fb", Some("fa"), None)),
        ],
        stories: vec![(STORY.to_string(), story)],
    }
}

#[test]
fn a_heading_takes_the_page_it_is_laid_out_on() {
    let bytes = paged_gen::write_idml(&sample()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    let inter = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    resolver.add_font("Inter", None, std::fs::read(inter).expect("Inter"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build");
    let pages = built.paragraph_pages();
    let story = pages.get(STORY).expect("the story was laid out");
    assert_eq!(story.get(&0), Some(&0), "{story:?}");
    let last = story.keys().max().copied().unwrap();
    assert_eq!(
        story.get(&last),
        Some(&1),
        "the chain reaches page 2: {story:?}"
    );

    let toc = TOCStyleDef {
        self_id: "TOCStyle/Contents".to_string(),
        entries: vec![TOCStyleEntryDef {
            include_style: Some(HEADING.to_string()),
            level: Some(1),
            ..Default::default()
        }],
        ..Default::default()
    };
    // The head frame's page for every heading, before L-06.
    let head: Vec<_> = document
        .resolve_toc(&toc)
        .iter()
        .map(|e| e.page_number)
        .collect();
    assert!(head.iter().all(|p| *p == Some(0)), "{head:?}");
    // Laid out: each heading on the page its first line stands on.
    let entries = document.resolve_toc_on(&toc, &pages);
    assert_eq!(entries.len(), 4);
    for (k, e) in entries.iter().enumerate() {
        assert_eq!(e.page_number, story.get(&(k * 10)).copied(), "{e:?}");
    }
    assert!(
        entries.iter().any(|e| e.page_number == Some(1)),
        "{entries:?}"
    );
}
