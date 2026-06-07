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

//! W1.8 mega-file: `footnotes.idml`.
//!
//! Exercises the footnotes-v2 paths the other samples leave out:
//!
//!   * a document-level **`<FootnoteOption>`** with the separator-rule
//!     turned on (explicit weight / width / left-indent / offset), so the
//!     renderer draws the rule above the pool from real designmap data.
//!   * a footnote whose body carries **per-run styling** — a bold run and
//!     a larger run inside one footnote — to prove footnote bodies now
//!     compose through the same run/shaping path as body text instead of
//!     flattening to a single style.
//!   * a deliberately **too-tall footnote** (a very long body anchored in
//!     a short frame) so the renderer's `FootnoteOverflow` diagnostic
//!     fires — the splitting-deferral guard.
//!
//! One A4 page: a single body paragraph anchors three footnotes. The
//! story is emitted with a purpose-built writer (rather than the generic
//! `story` builder) so it can nest `<Footnote>` with its own styled body
//! paragraphs without touching the shared `Run` shape.

use crate::builders::designmap::{
    write_designmap_with_markers, DesignMap, FootnoteOptionDef, MarkerResources,
};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml,
};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "footnotes";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 360.0;
// Short frame: the three footnotes' combined pool is taller than the
// whole frame content area, so even after the reservation pass pushes
// the body text up the pool still overruns — tripping the
// FootnoteOverflow diagnostic (the cross-frame-split deferral marker).
const FRAME_H_PT: f32 = 120.0;
const BODY_FONT: &str = "Open Sans";

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

/// One run inside a footnote body: text plus optional per-run overrides.
struct FnRun {
    text: &'static str,
    point_size: Option<f32>,
    font_style: Option<&'static str>,
    fill_color: Option<&'static str>,
}

impl FnRun {
    const fn plain(text: &'static str) -> Self {
        FnRun {
            text,
            point_size: None,
            font_style: None,
            fill_color: None,
        }
    }
    const fn bold(text: &'static str) -> Self {
        FnRun {
            text,
            point_size: None,
            font_style: Some("Bold"),
            fill_color: None,
        }
    }
    const fn sized(text: &'static str, pt: f32) -> Self {
        FnRun {
            text,
            point_size: Some(pt),
            font_style: None,
            fill_color: None,
        }
    }
}

/// One footnote: a self id plus its body runs (one paragraph).
struct Fn {
    self_id: &'static str,
    runs: Vec<FnRun>,
}

/// Emit a `<Footnote>` element with one body paragraph of styled runs.
fn write_footnote(b: &mut XmlBuilder, fn_: &Fn) {
    b.start("Footnote", &[("Self", fn_.self_id), ("Hidden", "false")]);
    b.start(
        "ParagraphStyleRange",
        &[(
            "AppliedParagraphStyle",
            "ParagraphStyle/$ID/[No paragraph style]",
        )],
    );
    for run in &fn_.runs {
        let pt = run.point_size.map(crate::xml::format_f32);
        let mut attrs: Vec<(&str, &str)> = vec![
            (
                "AppliedCharacterStyle",
                "CharacterStyle/$ID/[No character style]",
            ),
            ("AppliedFont", BODY_FONT),
        ];
        if let Some(ref p) = pt {
            attrs.push(("PointSize", p.as_str()));
        }
        if let Some(fs) = run.font_style {
            attrs.push(("FontStyle", fs));
        }
        if let Some(fc) = run.fill_color {
            attrs.push(("FillColor", fc));
        }
        b.start("CharacterStyleRange", &attrs);
        b.start("Content", &[]);
        b.text(run.text);
        b.end("Content");
        b.end("CharacterStyleRange");
    }
    b.end("ParagraphStyleRange");
    b.end("Footnote");
}

/// Emit the footnotes story: one body paragraph whose single content run
/// anchors three footnotes (the anchors sit at the end of the run; the
/// renderer captures them onto the host page's footnote pool in order).
fn write_footnotes_story(story_id: &str, footnotes: &[Fn]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start(
        "ParagraphStyleRange",
        &[(
            "AppliedParagraphStyle",
            "ParagraphStyle/$ID/[No paragraph style]",
        )],
    );
    b.start(
        "CharacterStyleRange",
        &[
            (
                "AppliedCharacterStyle",
                "CharacterStyle/$ID/[No character style]",
            ),
            ("AppliedFont", BODY_FONT),
            ("PointSize", "12"),
        ],
    );
    b.start("Content", &[]);
    b.text(
        "Body copy that references three footnotes. The footnote pool sits \
         below this text at the frame bottom, separated by the rule.",
    );
    b.end("Content");
    for fn_ in footnotes {
        write_footnote(&mut b, fn_);
    }
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let story_id = self_id(SAMPLE, "Story", 0);
    let frame_id = self_id(SAMPLE, "TextFrame", 0);
    let spread0_id = self_id(SAMPLE, "Spread", 0);
    let page0_id = self_id(SAMPLE, "Page", 0);

    // Three footnotes:
    //   fn1 — plain, short.
    //   fn2 — STYLED: a bold run and a larger (10pt vs the 8pt default)
    //         run inside one footnote, proving per-run composition.
    //   fn3 — TOO TALL: a long body that overruns the short frame, so the
    //         FootnoteOverflow diagnostic fires.
    let footnotes: Vec<Fn> = vec![
        Fn {
            self_id: "Footnote/fn1",
            runs: vec![FnRun::plain("First footnote, plain text.")],
        },
        Fn {
            self_id: "Footnote/fn2",
            runs: vec![
                FnRun::plain("Second footnote with a "),
                FnRun::bold("bold word"),
                FnRun::plain(" and a "),
                FnRun::sized("larger phrase", 10.0),
                FnRun::plain(" inline."),
            ],
        },
        Fn {
            self_id: "Footnote/fn3",
            runs: vec![FnRun::plain(
                "Third footnote whose body is deliberately long: it runs on \
                 and on across many wrapped lines so that, stacked beneath \
                 the first two footnotes in the narrow pool column at the \
                 bottom of this short frame, the pool grows taller than the \
                 frame can hold and the footnote-overflow diagnostic must \
                 fire — the marker for the deferred cross-frame split. It \
                 keeps going with still more sentences to be certain the \
                 accumulated pool height exceeds the entire frame content \
                 area, not merely the space left under the body text, so \
                 the overflow is unavoidable however the body reflows.",
            )],
        },
    ];

    // Document-level FootnoteOption: separator rule ON with explicit
    // geometry so the renderer draws it from real designmap data.
    let footnote_option = FootnoteOptionDef {
        rule_on: Some(true),
        rule_color: Some("Color/Black".to_string()),
        rule_line_weight: Some(1.0),
        rule_width: Some(140.0),
        rule_left_indent: Some(0.0),
        rule_offset: Some(2.0),
        separator_text: Some(". ".to_string()),
        space_between: Some(2.0),
        spacer: Some(4.0),
        ..FootnoteOptionDef::default()
    };
    let markers = MarkerResources {
        footnote_option: Some(footnote_option),
        ..MarkerResources::default()
    };

    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let story_bytes = write_footnotes_story(&story_id, &footnotes);

    let frame_transform = translate(
        (PAGE_W_PT - FRAME_W_PT) * 0.5,
        (PAGE_H_PT - FRAME_H_PT) * 0.25,
    );
    let body_frame = Rect {
        self_id: frame_id,
        width_pt: FRAME_W_PT,
        height_pt: FRAME_H_PT,
        item_transform: frame_transform,
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id.clone()),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
    };

    let spread0 = crate::builders::spread::write_spread(&crate::builders::spread::Spread {
        self_id: spread0_id.clone(),
        page_self_id: page0_id,
        page_name: "footnotes · separator + styled runs + overflow".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body_frame.into()],
        override_list: Vec::new(),
        margins: None,
    });

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread0_id.clone()],
            stories: vec![story_id.clone()],
        },
        &markers,
    );

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master_bytes)],
        spreads: vec![(spread0_id, spread0)],
        stories: vec![(story_id, story_bytes)],
    }
}
