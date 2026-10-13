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

//! `nested-styles` — nested character styles in InDesign's own spelling
//! (`<Properties><AllNestedStyles type="list">`), asked of InDesign.
//!
//! One frame per case, each a single Inter 11/14 paragraph whose style
//! carries the nested-style list named in the label: a tab, word,
//! character, literal-character and sentence delimiter, each through
//! (inclusive) and up to (exclusive); a chain of three; a repetition and
//! a `Repeat` loop; a drop cap whose character style is the first nested
//! style (`Dropcap`), and a style drop cap with none; a character style
//! applied by hand inside a nested range; a style that inherits its
//! parent's list and one that cleared it (`EmptyNestedStyles`); and a
//! paragraph's own list on its `<ParagraphStyleRange>`. The nested
//! styles only colour (red, blue, magenta) — one also sets 15 pt — so
//! the reference shows every range's extent.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{
        all_nested_styles_xml, container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml,
        styles_xml_with_raw, ExtraColor, NestedStyleSpec,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "nested-styles";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
pub const FRAME_W: f32 = 250.0;
const FRAME_H: f32 = 44.0;

pub const RED: &str = "CharacterStyle/NestRed";
pub const BLUE: &str = "CharacterStyle/NestBlue";
pub const MAGENTA: &str = "CharacterStyle/NestMagenta";
pub const RED_BIG: &str = "CharacterStyle/NestRedBig";
pub const HAND: &str = "CharacterStyle/HandUnderline";
pub const BODY: &str = "ParagraphStyle/NestBody";
pub const PARENT: &str = "ParagraphStyle/NestParent";

/// One nested style: (character style, delimiter, repetition, through).
type Nest = (&'static str, &'static str, i32, bool);

/// How a case's paragraph is styled.
pub enum Spec {
    /// Its own paragraph style, carrying this list.
    Style(&'static [Nest]),
    /// Its own paragraph style, carrying this list and a drop cap of
    /// (characters, lines).
    StyleWithCap(&'static [Nest], u32, u32),
    /// A paragraph style based on [`PARENT`], with these attributes and
    /// no list of its own.
    Child(&'static str),
    /// The plain body style, and this list on the range.
    Local(&'static [Nest]),
}

pub struct Case {
    /// The case's paragraph style (`ParagraphStyle/NestN01`, …).
    pub style: &'static str,
    pub label: &'static str,
    pub text: &'static str,
    pub spec: Spec,
    /// A drop cap of (characters, lines) on the range itself.
    pub range_cap: Option<(u32, u32)>,
    /// A character style applied by hand to these bytes of `text`.
    pub hand: Option<(usize, usize)>,
}

const fn case(style: &'static str, label: &'static str, text: &'static str, spec: Spec) -> Case {
    Case {
        style,
        label,
        text,
        spec,
        range_cap: None,
        hand: None,
    }
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    use Spec::*;
    vec![
        case(
            "ParagraphStyle/NestN01",
            "n01 through 1 tab",
            "Alpha\tbeta gamma delta epsilon",
            Style(&[(RED, "Tabs", 1, true)]),
        ),
        case(
            "ParagraphStyle/NestN02",
            "n02 up to 1 tab",
            "Alpha\tbeta gamma delta epsilon",
            Style(&[(RED, "Tabs", 1, false)]),
        ),
        case(
            "ParagraphStyle/NestN03",
            "n03 through 2 words",
            "Alpha  beta gamma delta epsilon",
            Style(&[(RED, "AnyWord", 2, true)]),
        ),
        case(
            "ParagraphStyle/NestN04",
            "n04 up to 2 words",
            "Alpha  beta gamma delta epsilon",
            Style(&[(RED, "AnyWord", 2, false)]),
        ),
        case(
            "ParagraphStyle/NestN05",
            "n05 through 3 characters",
            "Alphabet soup and more",
            Style(&[(RED, "AnyCharacter", 3, true)]),
        ),
        case(
            "ParagraphStyle/NestN06",
            "n06 up to 3 characters",
            "Alphabet soup and more",
            Style(&[(RED, "AnyCharacter", 3, false)]),
        ),
        case(
            "ParagraphStyle/NestN07",
            "n07 through 1 colon",
            "Heading: the body copy follows",
            Style(&[(RED, ":", 1, true)]),
        ),
        case(
            "ParagraphStyle/NestN08",
            "n08 up to 1 colon",
            "Heading: the body copy follows",
            Style(&[(RED, ":", 1, false)]),
        ),
        case(
            "ParagraphStyle/NestN09",
            "n09 tab, up to 2 words, colon",
            "Alpha\tBeta gamma delta: rest of it",
            Style(&[
                (RED, "Tabs", 1, true),
                (BLUE, "AnyWord", 2, false),
                (RED, ":", 1, true),
            ]),
        ),
        case(
            "ParagraphStyle/NestN10",
            "n10 through 1 sentence",
            "Pi is 3.14 here. Next one.",
            Style(&[(RED, "Sentence", 1, true)]),
        ),
        case(
            "ParagraphStyle/NestN11",
            "n11 up to 1 sentence",
            "He said \u{201C}stop.\u{201D} Next one.",
            Style(&[(RED, "Sentence", 1, false)]),
        ),
        case(
            "ParagraphStyle/NestN12",
            "n12 word, word, repeat 2",
            "one two three four five six seven",
            Style(&[
                (RED, "AnyWord", 1, true),
                (BLUE, "AnyWord", 1, true),
                ("CharacterStyle/$ID/[No character style]", "Repeat", 2, true),
            ]),
        ),
        Case {
            range_cap: Some((1, 2)),
            ..case(
                "ParagraphStyle/NestN13",
                "n13 range cap 1/2: dropcap, 2 words",
                "Drop cap paragraph with nested styles across two lines of text.",
                Style(&[(MAGENTA, "Dropcap", 1, true), (BLUE, "AnyWord", 2, true)]),
            )
        },
        case(
            "ParagraphStyle/NestN14",
            "n14 style cap 2/2, through 3 characters",
            "Drop cap paragraph whose cap is the style's own, two lines.",
            StyleWithCap(&[(RED, "AnyCharacter", 3, true)], 2, 2),
        ),
        Case {
            hand: Some((6, 15)),
            ..case(
                "ParagraphStyle/NestN15",
                "n15 15 pt red 3 words, underline by hand",
                "Manual underline only here",
                Style(&[(RED_BIG, "AnyWord", 3, true)]),
            )
        },
        case(
            "ParagraphStyle/NestN16",
            "n16 through 2 digits",
            "Room 12 and 34 more",
            Style(&[(RED, "Digits", 2, true)]),
        ),
        case(
            "ParagraphStyle/NestN17",
            "n17 through 2 of .,",
            "One, two. three four",
            Style(&[(RED, ".,", 2, true)]),
        ),
        case(
            "ParagraphStyle/NestN18",
            "n18 child inherits parent's list",
            "Inherited first word only",
            Child("EmptyNestedStyles=\"false\""),
        ),
        case(
            "ParagraphStyle/NestN19",
            "n19 child cleared parent's list",
            "Cleared first word only",
            Child("EmptyNestedStyles=\"true\""),
        ),
        case(
            "ParagraphStyle/NestN20",
            "n20 the range's own list, 3 words",
            "Local one two three four",
            Local(&[(BLUE, "AnyWord", 3, true)]),
        ),
    ]
}

fn specs(list: &[Nest]) -> Vec<NestedStyleSpec> {
    list.iter()
        .map(
            |&(style, delimiter, repetition, inclusive)| NestedStyleSpec {
                applied_character_style: style,
                delimiter,
                repetition,
                inclusive,
            },
        )
        .collect()
}

fn styles_fragment() -> String {
    let char_style = |id: &str, name: &str, attrs: &str| {
        format!("<CharacterStyle Self=\"{id}\" Name=\"{name}\" {attrs}/>")
    };
    let mut chars = String::new();
    chars.push_str(&char_style(RED, "Nest Red", "FillColor=\"Color/NestRed\""));
    chars.push_str(&char_style(
        BLUE,
        "Nest Blue",
        "FillColor=\"Color/NestBlue\"",
    ));
    chars.push_str(&char_style(
        MAGENTA,
        "Nest Magenta",
        "FillColor=\"Color/NestMagenta\"",
    ));
    chars.push_str(&char_style(
        RED_BIG,
        "Nest Red Big",
        "FillColor=\"Color/NestRed\" PointSize=\"15\"",
    ));
    chars.push_str(&char_style(HAND, "Hand Underline", "Underline=\"true\""));

    let para = |id: &str, name: &str, based_on: &str, attrs: &str, props: &str| {
        let based = if based_on.starts_with("$ID/") {
            format!("<BasedOn type=\"string\">{based_on}</BasedOn>")
        } else {
            format!("<BasedOn type=\"object\">{based_on}</BasedOn>")
        };
        format!(
            "<ParagraphStyle Self=\"{id}\" Name=\"{name}\" {attrs}><Properties>{based}{props}\
</Properties></ParagraphStyle>"
        )
    };
    let mut paras = String::new();
    paras.push_str(&para(
        BODY,
        "Nest Body",
        "$ID/[No paragraph style]",
        "PointSize=\"11\" FontStyle=\"Regular\" FillColor=\"Color/Black\" Hyphenation=\"false\"",
        "<AppliedFont type=\"string\">Inter</AppliedFont><Leading type=\"unit\">14</Leading>",
    ));
    paras.push_str(&para(
        PARENT,
        "Nest Parent",
        BODY,
        "EmptyNestedStyles=\"false\"",
        &all_nested_styles_xml(&specs(&[(RED, "AnyWord", 1, true)])),
    ));
    for c in cases() {
        let id = c.style;
        let name = c.style.trim_start_matches("ParagraphStyle/");
        match &c.spec {
            Spec::Style(list) => paras.push_str(&para(
                id,
                name,
                BODY,
                "EmptyNestedStyles=\"false\"",
                &all_nested_styles_xml(&specs(list)),
            )),
            Spec::StyleWithCap(list, chars, lines) => paras.push_str(&para(
                id,
                name,
                BODY,
                &format!(
                    "EmptyNestedStyles=\"false\" DropCapCharacters=\"{chars}\" \
DropCapLines=\"{lines}\""
                ),
                &all_nested_styles_xml(&specs(list)),
            )),
            Spec::Child(attrs) => paras.push_str(&para(id, name, PARENT, attrs, "")),
            Spec::Local(_) => {}
        }
    }
    format!(
        "<RootCharacterStyleGroup>{chars}</RootCharacterStyleGroup>\
<RootParagraphStyleGroup>{paras}</RootParagraphStyleGroup>"
    )
}

fn run(text: &str, style: Option<&'static str>) -> Run {
    Run {
        text: text.to_string(),
        point_size: None,
        fill_color: None,
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: None,
        anchored_frame: None,
        extra_char_attrs: style
            .map(|s| vec![("AppliedCharacterStyle", s)])
            .unwrap_or_default(),
    }
}

fn body_paragraph(c: &Case) -> Paragraph {
    let style = match c.spec {
        Spec::Local(_) => BODY,
        _ => c.style,
    };
    let runs = match c.hand {
        Some((a, b)) => vec![
            run(&c.text[..a], None),
            run(&c.text[a..b], Some(HAND)),
            run(&c.text[b..], None),
        ],
        None => vec![run(c.text, None)],
    };
    Paragraph {
        extra_paragraph_attrs: vec![("AppliedParagraphStyle", style)],
        drop_cap_characters: c.range_cap.map(|(n, _)| n),
        drop_cap_lines: c.range_cap.map(|(_, l)| l),
        runs,
        ..Paragraph::plain("")
    }
}

/// The story bytes, with a range's own list spliced into its
/// `<ParagraphStyleRange>` as the first child (the builder has no typed
/// field for a list-valued property).
fn body_story(id: &str, c: &Case) -> Vec<u8> {
    let xml = write_story(&Story {
        extra_story_attrs: Vec::new(),
        self_id: id.to_string(),
        paragraphs: vec![body_paragraph(c)],
    });
    let Spec::Local(list) = c.spec else {
        return xml;
    };
    let mut text = String::from_utf8(xml).expect("story is utf-8");
    let open = text.find("<ParagraphStyleRange").expect("a range");
    let end = open + text[open..].find('>').expect("its start tag ends");
    text.insert_str(
        end + 1,
        &format!(
            "<Properties>{}</Properties>",
            all_nested_styles_xml(&specs(list))
        ),
    );
    text.into_bytes()
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 2;
    let row = i / 2;
    (36.0 + col as f32 * 273.0, 60.0 + row as f32 * 77.0)
}

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        // No frame strokes: a hairline rasterises differently in the
        // reference (pdftoppm's one-pixel minimum) and would outweigh the
        // text this fixture measures.
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
        text_frame_pref: body.then(|| TextFramePref {
            inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
            first_baseline_offset: Some("LeadingOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

fn colors() -> Vec<ExtraColor> {
    let cmyk = |id: &str, name: &str, value: &str| ExtraColor {
        self_id: format!("Color/{id}"),
        name: name.to_string(),
        space: "CMYK",
        value: value.to_string(),
    };
    vec![
        cmyk("NestRed", "Nest Red", "0 100 100 0"),
        cmyk("NestBlue", "Nest Blue", "100 60 0 0"),
        cmyk("NestMagenta", "Nest Magenta", "0 100 0 0"),
    ]
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items = Vec::new();

    for (i, c) in cases().iter().enumerate() {
        let seq = i as u32;
        let (x, y) = frame_origin(seq);
        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph {
                    runs: vec![Run {
                        point_size: Some(8.0),
                        ..run(c.label, None)
                    }],
                    ..Paragraph::plain("")
                }],
            }),
        ));
        story_refs.push(label_story.clone());
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story,
            FRAME_W,
            14.0,
            (x, y - 16.0),
            false,
        ));

        let body = self_id(SAMPLE, "BodyStory", seq);
        stories.push((body.clone(), body_story(&body, c)));
        story_refs.push(body.clone());
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            FRAME_W,
            FRAME_H,
            (x, y),
            true,
        ));
    }

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: self_id(SAMPLE, "Page", 0),
        page_name: "1".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
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
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras(&colors()),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml_with_raw(&styles_fragment()),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
