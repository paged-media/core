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

//! `variables.idml` — text variables and page-number markers, written in
//! the vocabulary InDesign itself writes (ADR 033, RFI C-37).
//!
//! The previous version of this sample used `RunningHeaderType`,
//! `<TextVariablePreference>` and a section numbering that InDesign never
//! writes; InDesign's reference PDF of it printed nothing for the running
//! header and the dates, so the fidelity gate measured a blank slot. Every
//! construct here was first built through InDesign's DOM, exported as IDML
//! and read back (InDesign 20.0.1, 2026-10-01), and this file writes that
//! spelling:
//!
//! * running headers are `MatchParagraphStyleType` /
//!   `MatchCharacterStyleType` with a `<Match…StylePreference
//!   SearchStrategy ChangeCase DeleteEndPunctuation>`;
//! * the last page number is `LastPageNumberType` with a
//!   `<PageNumberVariablePreference Scope Format>`;
//! * next / previous page number are `<?ACE 18?>` inside a range carrying
//!   `PageNumberType="NextPageNumber"` / `"PreviousPageNumber"`; `<?ACE 19?>`
//!   is the SECTION MARKER;
//! * a variable instance sits in a range with `PageNumberType="TextVariable"`.
//!
//! Five A4 pages on one master. A section restarts at 1 on page 3, so the
//! page labels are 1 2 1 2 3. The master's header frame carries the
//! running headers, its footer the page numbers, the section marker, the
//! chapter number and a custom variable whose instance has
//! an EMPTY `ResultText` (RFI C-39). Headings sit on pages 1 (two), 3 and
//! 5; pages 2 and 4 have none, so their headers carry the previous match
//! forward. A story threaded from page 3 to page 5 prints its next and
//! previous page numbers. Page 1 carries the three dates and a
//! cross-reference to page 2's story.

use crate::builders::designmap::{
    write_designmap_with_markers, DesignMap, HyperlinkDef, HyperlinkDestinationDef,
    MarkerResources, SectionDef, TextVariableDef, VariablePreference,
};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "variables";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const MARGIN_X: f32 = 57.638;
const FRAME_W_PT: f32 = PAGE_W_PT - 2.0 * MARGIN_X;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub const HEADING_STYLE: &str = "ParagraphStyle/Heading";
pub const KEYWORD_STYLE: &str = "CharacterStyle/Keyword";
const NO_PARA_STYLE: &str = "ParagraphStyle/$ID/[No paragraph style]";
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

/// The two styles the running headers match. Defined, not just named: an
/// `AppliedParagraphStyle` naming a style the package does not define is
/// remapped by InDesign on open, and the variable then matches nothing.
const STYLES_FRAGMENT: &str = r#"<RootCharacterStyleGroup><CharacterStyle Self="CharacterStyle/Keyword" Name="Keyword" Underline="true" /></RootCharacterStyleGroup><RootParagraphStyleGroup><ParagraphStyle Self="ParagraphStyle/Heading" Name="Heading" PointSize="18" SpaceBefore="6" SpaceAfter="6" FillColor="Color/Black" FontStyle="Regular"><Properties><BasedOn type="string">$ID/[No paragraph style]</BasedOn><AppliedFont type="string">Open Sans</AppliedFont></Properties></ParagraphStyle></RootParagraphStyleGroup>"#;

/// The page-number flavours an `<?ACE 18?>` takes from its range.
#[derive(Clone, Copy)]
enum PageNumber {
    Current,
    Next,
    Previous,
}

/// One inline segment of a paragraph.
enum Seg {
    Text(&'static str),
    /// Text in the `Keyword` character style.
    Keyword(&'static str),
    Variable {
        instance: String,
        name: &'static str,
        result_text: &'static str,
        associated: String,
    },
    PageNumber(PageNumber),
    SectionMarker,
    Xref {
        source_self: String,
        text: &'static str,
    },
}

struct Para {
    style: &'static str,
    /// Extra `<ParagraphStyleRange>` attributes (`StartParagraph`).
    attrs: Vec<(&'static str, &'static str)>,
    segs: Vec<Seg>,
}

fn heading(text: &'static str) -> Para {
    Para {
        style: HEADING_STYLE,
        attrs: Vec::new(),
        segs: vec![Seg::Text(text)],
    }
}

fn body(segs: Vec<Seg>) -> Para {
    Para {
        style: NO_PARA_STYLE,
        attrs: Vec::new(),
        segs,
    }
}

fn csr_start(b: &mut XmlBuilder, char_style: &str, page_number_type: Option<&str>) {
    let mut attrs = vec![("AppliedCharacterStyle", char_style)];
    if let Some(kind) = page_number_type {
        attrs.push(("PageNumberType", kind));
    }
    b.start("CharacterStyleRange", &attrs);
}

/// One segment as its own `<CharacterStyleRange>`; `br` appends the
/// paragraph mark (`<Br/>`), which InDesign writes as the last character of
/// every paragraph but the story's last.
fn write_seg(b: &mut XmlBuilder, seg: &Seg, br: bool) {
    match seg {
        Seg::Text(t) | Seg::Keyword(t) => {
            let style = if matches!(seg, Seg::Keyword(_)) {
                KEYWORD_STYLE
            } else {
                NO_CHAR_STYLE
            };
            csr_start(b, style, None);
            b.start("Content", &[]);
            b.text(t);
            b.end("Content");
        }
        Seg::Variable {
            instance,
            name,
            result_text,
            associated,
        } => {
            csr_start(b, NO_CHAR_STYLE, Some("TextVariable"));
            b.empty(
                "TextVariableInstance",
                &[
                    ("Self", instance.as_str()),
                    ("Name", name),
                    ("ResultText", result_text),
                    ("AssociatedTextVariable", associated.as_str()),
                ],
            );
        }
        Seg::PageNumber(kind) => {
            let attr = match kind {
                PageNumber::Current => None,
                PageNumber::Next => Some("NextPageNumber"),
                PageNumber::Previous => Some("PreviousPageNumber"),
            };
            csr_start(b, NO_CHAR_STYLE, attr);
            b.start("Content", &[]);
            b.write_pi("ACE", "18");
            b.end("Content");
        }
        Seg::SectionMarker => {
            csr_start(b, NO_CHAR_STYLE, None);
            b.start("Content", &[]);
            b.write_pi("ACE", "19");
            b.end("Content");
        }
        Seg::Xref { source_self, text } => {
            b.start(
                "CrossReferenceSource",
                &[
                    ("Self", source_self.as_str()),
                    ("Name", source_self.as_str()),
                    ("AppliedCharacterStyle", NO_CHAR_STYLE),
                ],
            );
            csr_start(b, NO_CHAR_STYLE, None);
            b.start("Content", &[]);
            b.text(text);
            b.end("Content");
            if br {
                b.empty("Br", &[]);
            }
            b.end("CharacterStyleRange");
            b.end("CrossReferenceSource");
            return;
        }
    }
    if br {
        b.empty("Br", &[]);
    }
    b.end("CharacterStyleRange");
}

fn write_story(story_id: &str, paras: &[Para]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    for (i, para) in paras.iter().enumerate() {
        let mut attrs = vec![("AppliedParagraphStyle", para.style)];
        attrs.extend(para.attrs.iter().copied());
        b.start("ParagraphStyleRange", &attrs);
        let last_para = i + 1 == paras.len();
        for (j, seg) in para.segs.iter().enumerate() {
            let last_seg = j + 1 == para.segs.len();
            write_seg(&mut b, seg, last_seg && !last_para);
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

fn text_frame(self_id: String, w: f32, h: f32, dx: f32, dy: f32, story: String) -> Rect {
    Rect {
        self_id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(dx, dy),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story),
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
        custom_subpaths: None,
    }
}

/// `TextVariable/<id>` for the sample's `n`th variable.
fn var_id(n: u32) -> String {
    format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", n))
}

struct VarSpec {
    name: &'static str,
    variable_type: &'static str,
    preference: VariablePreference,
}

fn match_style(
    character: bool,
    search: &'static str,
    case: &'static str,
    delete_punct: bool,
) -> VariablePreference {
    VariablePreference::MatchStyle {
        character,
        style: if character {
            KEYWORD_STYLE
        } else {
            HEADING_STYLE
        }
        .to_string(),
        search_strategy: search,
        change_case: case,
        delete_end_punctuation: delete_punct,
    }
}

/// The variables, in id order. Index = the `n` of [`var_id`].
fn variable_specs() -> Vec<VarSpec> {
    let spec = |name, variable_type, preference| VarSpec {
        name,
        variable_type,
        preference,
    };
    let para = "MatchParagraphStyleType";
    let chr = "MatchCharacterStyleType";
    vec![
        spec(
            "Header First",
            para,
            match_style(false, "FirstOnPage", "None", false),
        ),
        spec(
            "Header Last",
            para,
            match_style(false, "LastOnPage", "None", false),
        ),
        spec(
            "Keyword First",
            chr,
            match_style(true, "FirstOnPage", "None", false),
        ),
        spec(
            "Keyword Last",
            chr,
            match_style(true, "LastOnPage", "None", false),
        ),
        spec(
            "Header Upper",
            para,
            match_style(false, "FirstOnPage", "Uppercase", true),
        ),
        spec(
            "Header Lower",
            para,
            match_style(false, "FirstOnPage", "Lowercase", false),
        ),
        spec(
            "Header Title",
            para,
            match_style(false, "FirstOnPage", "Titlecase", false),
        ),
        spec(
            "Header Sentence",
            para,
            match_style(false, "FirstOnPage", "Sentencecase", false),
        ),
        spec(
            "Last Page Document",
            "LastPageNumberType",
            VariablePreference::PageNumber {
                scope: "DocumentScope",
                format: "Current",
            },
        ),
        spec(
            "Last Page Section",
            "LastPageNumberType",
            VariablePreference::PageNumber {
                scope: "SectionScope",
                format: "Current",
            },
        ),
        spec(
            "Last Page Roman",
            "LastPageNumberType",
            VariablePreference::PageNumber {
                scope: "DocumentScope",
                format: "UpperRoman",
            },
        ),
        spec(
            "Chapter",
            "ChapterNumberType",
            VariablePreference::ChapterNumber { format: "Current" },
        ),
        spec(
            "Edition",
            "CustomTextType",
            VariablePreference::CustomText {
                contents: "Edition 7".to_string(),
            },
        ),
        spec(
            "Created",
            "CreationDateType",
            VariablePreference::Date {
                format: "yyyy-MM-dd".to_string(),
            },
        ),
        spec(
            "Modified",
            "ModificationDateType",
            VariablePreference::Date {
                format: "MMMM d, yyyy".to_string(),
            },
        ),
        spec(
            "Output",
            "OutputDateType",
            VariablePreference::Date {
                format: "EEEE dd.MM.yy".to_string(),
            },
        ),
    ]
}

pub fn build() -> Sample {
    build_with(false)
}

/// W1.19 variant — a BLANK spread between page 1 and page 2, so the
/// cross-reference's destination story moves from flat page index 1 to 2.
/// Proves a fresh render re-resolves the xref against the CURRENT layout.
pub fn build_moved() -> Sample {
    build_with(true)
}

fn build_with(move_destination: bool) -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let specs = variable_specs();
    let mut instance_seq = 0u32;
    let mut var = |n: u32, result_text: &'static str| {
        instance_seq += 1;
        Seg::Variable {
            instance: self_id(SAMPLE, "TextVariableInstance", instance_seq),
            name: specs[n as usize].name,
            result_text,
            associated: var_id(n),
        }
    };

    // ---- master: header + footer ------------------------------------
    let header_story = self_id(SAMPLE, "MasterStory", 0);
    let footer_story = self_id(SAMPLE, "MasterStory", 1);
    let header_paras = vec![
        body(vec![
            Seg::Text("First: "),
            var(0, "<Header First>"),
            Seg::Text(" | Last: "),
            var(1, "<Header Last>"),
        ]),
        body(vec![
            Seg::Text("Keyword first: "),
            var(2, "<Keyword First>"),
            Seg::Text(" | last: "),
            var(3, "<Keyword Last>"),
        ]),
        body(vec![Seg::Text("Upper, no end punctuation: "), var(4, "")]),
        body(vec![
            Seg::Text("Lower: "),
            var(5, "<Header Lower>"),
            Seg::Text(" | Title: "),
            var(6, "<Header Title>"),
        ]),
        body(vec![Seg::Text("Sentence: "), var(7, "<Header Sentence>")]),
    ];
    let footer_paras = vec![
        body(vec![
            Seg::Text("Page "),
            Seg::PageNumber(PageNumber::Current),
            Seg::Text(" of "),
            var(8, "3"),
            Seg::Text(" | section ends at "),
            var(9, "2"),
            Seg::Text(" | roman "),
            var(10, "III"),
        ]),
        body(vec![
            Seg::Text("Section: "),
            Seg::SectionMarker,
            Seg::Text(" | chapter "),
            var(11, "1"),
            Seg::Text(" | "),
            // RFI C-39: an instance whose stored result is empty.
            var(12, ""),
        ]),
    ];
    let header_frame = text_frame(
        self_id(SAMPLE, "MasterFrame", 0),
        FRAME_W_PT,
        110.0,
        MARGIN_X,
        28.0,
        header_story.clone(),
    );
    let footer_frame = text_frame(
        self_id(SAMPLE, "MasterFrame", 1),
        FRAME_W_PT,
        44.0,
        MARGIN_X,
        PAGE_H_PT - 72.0,
        footer_story.clone(),
    );
    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![header_frame.into(), footer_frame.into()],
    });

    // ---- body stories ------------------------------------------------
    let xref_source = format!("CrossReferenceSource/{}", self_id(SAMPLE, "Xref", 0));
    let xref_dest = format!(
        "HyperlinkTextDestination/{}",
        self_id(SAMPLE, "XrefDest", 0)
    );
    let page_stories: Vec<Vec<Para>> = vec![
        vec![
            heading("Introduction: the First Heading."),
            body(vec![
                Seg::Text("Body text with an "),
                Seg::Keyword("alpha keyword"),
                Seg::Text(" in it."),
            ]),
            heading("a second heading, on page one?"),
            body(vec![
                Seg::Text("More text and a "),
                Seg::Keyword("beta"),
                Seg::Text(" keyword."),
            ]),
            body(vec![
                Seg::Text("Created "),
                var(13, ""),
                Seg::Text(" | modified "),
                var(14, ""),
                Seg::Text(" | output "),
                var(15, ""),
            ]),
            body(vec![
                Seg::Text("See "),
                Seg::Xref {
                    source_self: xref_source.clone(),
                    text: "the next page",
                },
                Seg::Text("."),
            ]),
        ],
        vec![body(vec![Seg::Text(
            "No heading and no keyword on this page: both carry page one's forward.",
        )])],
        vec![
            heading("PART TWO begins (a third heading)"),
            body(vec![
                Seg::Text("Text with the "),
                Seg::Keyword("gamma"),
                Seg::Text(" keyword."),
            ]),
        ],
        vec![body(vec![Seg::Text("No heading here either.")])],
        vec![
            heading("the final heading!"),
            body(vec![
                Seg::Text("And the "),
                Seg::Keyword("delta"),
                Seg::Text(" keyword, last."),
            ]),
        ],
    ];
    // A story threaded from page 3 to page 5 (labels 1 and 3): the next
    // page number of a frame is the page of the NEXT frame in its thread,
    // the previous one that of the previous frame, and a frame at the end
    // of the thread prints its own page (measured).
    let jump_story = self_id(SAMPLE, "JumpStory", 0);
    let jump_paras = vec![
        body(vec![
            Seg::Text("Jump: continued on page "),
            Seg::PageNumber(PageNumber::Next),
            Seg::Text(", previous "),
            Seg::PageNumber(PageNumber::Previous),
            Seg::Text("."),
        ]),
        Para {
            style: NO_PARA_STYLE,
            attrs: vec![("StartParagraph", "NextFrame")],
            segs: vec![
                Seg::Text("Jump: continued from page "),
                Seg::PageNumber(PageNumber::Previous),
                Seg::Text(", next "),
                Seg::PageNumber(PageNumber::Next),
                Seg::Text("."),
            ],
        },
    ];

    let body_story_ids: Vec<String> = (0..5).map(|i| self_id(SAMPLE, "Story", i)).collect();
    let page_ids: Vec<String> = (0..5).map(|i| self_id(SAMPLE, "Page", i)).collect();
    let spread_ids: Vec<String> = (0..5).map(|i| self_id(SAMPLE, "Spread", i)).collect();
    let jump_frames = [
        self_id(SAMPLE, "JumpFrame", 0),
        self_id(SAMPLE, "JumpFrame", 1),
    ];
    let page_labels = ["1", "2", "1", "2", "3"];

    let mut spreads: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..5usize {
        let mut items = vec![text_frame(
            self_id(SAMPLE, "TextFrame", i as u32),
            FRAME_W_PT,
            300.0,
            MARGIN_X,
            160.0,
            body_story_ids[i].clone(),
        )
        .into()];
        if i == 2 || i == 4 {
            let mut jump = text_frame(
                jump_frames[usize::from(i == 4)].clone(),
                FRAME_W_PT,
                60.0,
                MARGIN_X,
                520.0,
                jump_story.clone(),
            );
            if i == 2 {
                jump.next_text_frame = Some(jump_frames[1].clone());
            } else {
                jump.previous_text_frame = Some(jump_frames[0].clone());
            }
            items.push(jump.into());
        }
        let bytes = write_spread(&Spread {
            self_id: spread_ids[i].clone(),
            page_self_id: page_ids[i].clone(),
            page_name: page_labels[i].to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        });
        spreads.push((spread_ids[i].clone(), bytes));
    }
    if move_destination {
        let blank_id = self_id(SAMPLE, "Spread", 5);
        let blank = write_spread(&Spread {
            self_id: blank_id.clone(),
            page_self_id: self_id(SAMPLE, "Page", 5),
            page_name: "spacer".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        });
        spreads.insert(1, (blank_id, blank));
    }

    let text_variables = specs
        .into_iter()
        .enumerate()
        .map(|(n, s)| TextVariableDef {
            self_id: var_id(n as u32),
            name: s.name.to_string(),
            variable_type: s.variable_type.to_string(),
            preference: Some(s.preference),
            ..Default::default()
        })
        .collect();
    let markers = MarkerResources {
        layers: Vec::new(),
        text_variables,
        hyperlink_destinations: vec![HyperlinkDestinationDef::TextAnchor {
            self_id: xref_dest.clone(),
            story: body_story_ids[1].clone(),
        }],
        hyperlinks: vec![HyperlinkDef {
            self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 0)),
            name: "xref".to_string(),
            source: xref_source,
            destination: xref_dest,
        }],
        sections: vec![
            SectionDef {
                self_id: format!("Section/{}", self_id(SAMPLE, "Section", 0)),
                page_start: page_ids[0].clone(),
                marker: Some("Part One".to_string()),
                continue_numbering: Some(true),
                length: Some(2),
                ..Default::default()
            },
            SectionDef {
                self_id: format!("Section/{}", self_id(SAMPLE, "Section", 1)),
                page_start: page_ids[2].clone(),
                start_at: Some(1),
                continue_numbering: Some(false),
                marker: Some("Part Two".to_string()),
                length: Some(3),
                ..Default::default()
            },
        ],
        footnote_option: None,
        bookmarks: Vec::new(),
        index_topics: Vec::new(),
        conditions: Vec::new(),
        condition_sets: Vec::new(),
    };

    let mut stories: Vec<(String, Vec<u8>)> = body_story_ids
        .iter()
        .zip(page_stories.iter())
        .map(|(id, paras)| (id.clone(), write_story(id, paras)))
        .collect();
    stories.push((jump_story.clone(), write_story(&jump_story, &jump_paras)));
    stories.push((
        header_story.clone(),
        write_story(&header_story, &header_paras),
    ));
    stories.push((
        footer_story.clone(),
        write_story(&footer_story, &footer_paras),
    ));

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        },
        &markers,
    );

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml_with_raw(STYLES_FRAGMENT),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master_bytes)],
        spreads,
        stories,
    }
}
