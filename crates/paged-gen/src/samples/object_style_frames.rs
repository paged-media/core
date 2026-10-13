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

//! `object-style-frames.idml` — text-frame options a frame takes from its
//! object style, asked of InDesign (ADR 120, ADR 137).
//!
//! InDesign leaves the options a frame takes from its object style off the
//! frame: a frame of a style whose text is centred is written with no
//! `VerticalJustification` (measured on InDesign 20.0.1 through its DOM).
//! Every body frame here states no option of its own, unless its case
//! says so, and takes them from its `AppliedObjectStyle`:
//!
//!  1. vertical justification `CenterAlign`;
//!  2. `BottomAlign`;
//!  3. insets of 18 pt (InDesign's typed `<InsetSpacing>` list);
//!  4. two columns, a 12 pt gutter;
//!  5. first baseline `CapHeight` (baseline options);
//!  6. `CenterAlign`, with the style's text-frame category switched off
//!     (`EnableTextFrameGeneralOptions="false"`);
//!  7. `CenterAlign`, the frame stating `TopAlign` itself;
//!  8. a style based on case 1's, setting nothing of its own;
//!  9. a style based on case 1's, its text-frame category switched off.
//!
//! Each frame is tinted so its bounds show, with its case label above it.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml_with_raw,
        ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "object-style-frames";
const PAGE_W_PT: f32 = 595.276; // A4
const PAGE_H_PT: f32 = 841.89;
const FONT: &str = "Open Sans";
const BODY_SIZE: f32 = 11.0;
const LABEL_SIZE: f32 = 7.0;
pub const FRAME_W: f32 = 160.0;
pub const FRAME_H: f32 = 150.0;
const COL_X: [f32; 3] = [36.0, 217.6, 399.2];
const ROW_Y: [f32; 3] = [90.0, 330.0, 570.0];
const TINT: &str = "Color/FrameTint";

/// One case: its label, the style it applies, whether the frame states a
/// vertical justification of its own, and its text.
pub struct Case {
    pub label: &'static str,
    pub style: &'static str,
    pub own_vertical_justification: Option<&'static str>,
    pub text: &'static str,
}

const SHORT: &str = "Set where the object style puts it.";
const LONG: &str = "Two columns from the object style: this paragraph runs long \
enough to fill the first column of the frame and carry on into the second, \
where it ends a few lines down. Neither the column count nor the gutter is \
written on the frame itself.";

pub fn cases() -> [Case; 9] {
    [
        Case {
            label: "1 · style: centre",
            style: "OS Center",
            own_vertical_justification: None,
            text: SHORT,
        },
        Case {
            label: "2 · style: bottom",
            style: "OS Bottom",
            own_vertical_justification: None,
            text: SHORT,
        },
        Case {
            label: "3 · style: insets 18 pt",
            style: "OS Insets",
            own_vertical_justification: None,
            text: SHORT,
        },
        Case {
            label: "4 · style: 2 columns",
            style: "OS Columns",
            own_vertical_justification: None,
            text: LONG,
        },
        Case {
            label: "5 · style: first baseline cap height",
            style: "OS CapHeight",
            own_vertical_justification: None,
            text: SHORT,
        },
        Case {
            label: "6 · style: centre, category off",
            style: "OS Center Off",
            own_vertical_justification: None,
            text: SHORT,
        },
        Case {
            label: "7 · style: centre, frame: top",
            style: "OS Center",
            own_vertical_justification: Some("TopAlign"),
            text: SHORT,
        },
        Case {
            label: "8 · based on 1",
            style: "OS Child",
            own_vertical_justification: None,
            text: SHORT,
        },
        Case {
            label: "9 · based on 1, category off",
            style: "OS Child Off",
            own_vertical_justification: None,
            text: SHORT,
        },
    ]
}

fn object_style(name: &str, enable: &str, based_on: &str, tfp: &str) -> String {
    format!(
        "<ObjectStyle Self=\"ObjectStyle/{name}\" Name=\"{name}\" {enable}>\
<Properties><BasedOn type=\"object\">{based_on}</BasedOn></Properties>{tfp}</ObjectStyle>"
    )
}

fn styles() -> Vec<u8> {
    const ROOT: &str = "ObjectStyle/$ID/[None]";
    const ON: &str =
        "EnableTextFrameGeneralOptions=\"true\" EnableTextFrameBaselineOptions=\"true\"";
    let insets = "<TextFramePreference><Properties><InsetSpacing type=\"list\">\
<ListItem type=\"unit\">18</ListItem><ListItem type=\"unit\">18</ListItem>\
<ListItem type=\"unit\">18</ListItem><ListItem type=\"unit\">18</ListItem>\
</InsetSpacing></Properties></TextFramePreference>";
    let objects = [
        object_style(
            "OS Center",
            ON,
            ROOT,
            "<TextFramePreference VerticalJustification=\"CenterAlign\"/>",
        ),
        object_style(
            "OS Bottom",
            ON,
            ROOT,
            "<TextFramePreference VerticalJustification=\"BottomAlign\"/>",
        ),
        object_style("OS Insets", ON, ROOT, insets),
        object_style(
            "OS Columns",
            ON,
            ROOT,
            "<TextFramePreference TextColumnCount=\"2\" TextColumnGutter=\"12\"/>",
        ),
        object_style(
            "OS CapHeight",
            ON,
            ROOT,
            "<TextFramePreference FirstBaselineOffset=\"CapHeight\"/>",
        ),
        object_style(
            "OS Center Off",
            "EnableTextFrameGeneralOptions=\"false\" EnableTextFrameBaselineOptions=\"true\"",
            ROOT,
            "<TextFramePreference VerticalJustification=\"CenterAlign\"/>",
        ),
        object_style("OS Child", ON, "ObjectStyle/OS Center", ""),
        object_style(
            "OS Child Off",
            "EnableTextFrameGeneralOptions=\"false\" EnableTextFrameBaselineOptions=\"true\"",
            "ObjectStyle/OS Center",
            "",
        ),
    ]
    .concat();
    styles_xml_with_raw(&format!(
        "<RootObjectStyleGroup>{objects}</RootObjectStyleGroup>"
    ))
}

fn paragraph(text: &str, size: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(size * 1.2),
        justification: Some("LeftAlign"),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(size),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

#[allow(clippy::too_many_arguments)]
fn frame(
    id: String,
    story: String,
    size: (f32, f32),
    at: (f32, f32),
    fill: Option<&str>,
    style: Option<&str>,
    pref: Option<TextFramePref>,
) -> PageItem {
    Rect {
        self_id: id,
        width_pt: size.0,
        height_pt: size.1,
        item_transform: translate(at.0, at.1),
        fill_color: fill.map(String::from),
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: style
            .map(|s| vec![("AppliedObjectStyle".to_string(), format!("ObjectStyle/{s}"))])
            .unwrap_or_default(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: pref,
        custom_subpaths: None,
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut items = Vec::new();
    let mut story = |id: String, p: Paragraph| {
        stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs: vec![p],
            }),
        ));
        id
    };
    let zero = || TextFramePref {
        inset_spacing: Some([0.0; 4]),
        ..Default::default()
    };
    let title = story(
        self_id(SAMPLE, "TitleStory", 0),
        paragraph(
            "object-style-frames · text-frame options from the object style",
            LABEL_SIZE,
        ),
    );
    items.push(frame(
        self_id(SAMPLE, "TitleFrame", 0),
        title,
        (520.0, 12.0),
        (36.0, 40.0),
        None,
        None,
        Some(zero()),
    ));
    for (i, c) in cases().iter().enumerate() {
        let seq = i as u32;
        let (x, y) = (COL_X[i % 3], ROW_Y[i / 3]);
        let label = story(
            self_id(SAMPLE, "LabelStory", seq),
            paragraph(c.label, LABEL_SIZE),
        );
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label,
            (FRAME_W, 12.0),
            (x, y - 16.0),
            None,
            None,
            Some(zero()),
        ));
        let body = story(self_id(SAMPLE, "Story", seq), paragraph(c.text, BODY_SIZE));
        let pref = c.own_vertical_justification.map(|vj| TextFramePref {
            vertical_justification: Some(vj),
            ..Default::default()
        });
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            (FRAME_W, FRAME_H),
            (x, y),
            Some(TINT),
            Some(c.style),
            pref,
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
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml_with_extras(&[ExtraColor {
            self_id: TINT.to_string(),
            name: "FrameTint".to_string(),
            space: "CMYK",
            value: "0 0 25 0".to_string(),
        }]),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
