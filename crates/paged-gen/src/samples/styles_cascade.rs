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

//! W4.9 mega-file: `styles-cascade.idml`.
//!
//! The first generated fixture to populate the advanced style-cascade +
//! OpenType-typography corners the existing text fixtures leave open.
//! Five A4 pages, one feature each:
//!
//!   1. **next-style chaining** — three paragraph styles
//!      (`Title → Subtitle → Body`) whose `NextStyle` links chain the
//!      following paragraph's style. A two-paragraph story applies
//!      `Title` then `Body`; the parse round-trip asserts the chain
//!      resolves, and the render proves the styled paragraphs lay out.
//!   2. **named-list definition cascade** — a `<NumberingList>` resource
//!      bound through a paragraph style + a child style `BasedOn` it, so
//!      the list reference cascades down the BasedOn chain.
//!   3. **cell + table styles cascade (BasedOn)** — a base `<CellStyle>`
//!      (supplies the fill) + a derived cell style `BasedOn` it, and a
//!      base `<TableStyle>` + a derived table style `BasedOn` it. A table
//!      references the derived table style; the cascade resolves the
//!      inherited fill + region cell-style.
//!   4. **OTF feature runs** — three runs each carrying one discrete
//!      OpenType feature (`OTFFraction`, `OTFOrdinal`,
//!      `OTFContextualAlternate`) on its `<CharacterStyleRange>`. The
//!      render test renders this page with the features on vs off and
//!      asserts the rasters diverge (Inter ships `frac`/`ordn`/`calt`).
//!   5. **hyphenation-zone justified composition** — a justified
//!      paragraph style carrying `HyphenationZone`, laid over a long
//!      single-word-rich body so the breaker's zone restriction is in
//!      play. The render proves it composes multiple justified lines.

use crate::builders::designmap::{write_designmap, DesignMap};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml_with_raw,
    ExtraColor,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::story::{write_story, Cell, Paragraph, Run, Story, Table};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "styles-cascade";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 600.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

// ── Style self-ids exported for the tests ────────────────────────

pub const STYLE_TITLE: &str = "ParagraphStyle/Title";
pub const STYLE_SUBTITLE: &str = "ParagraphStyle/Subtitle";
pub const STYLE_BODY: &str = "ParagraphStyle/Body";
pub const NUMBERING_LIST: &str = "NumberingList/Steps";
pub const STYLE_LIST_BASE: &str = "ParagraphStyle/ListBase";
pub const STYLE_LIST_DERIVED: &str = "ParagraphStyle/ListDerived";
pub const CELL_BASE: &str = "CellStyle/CellBase";
pub const CELL_DERIVED: &str = "CellStyle/CellDerived";
pub const TABLE_BASE: &str = "TableStyle/TableBase";
pub const TABLE_DERIVED: &str = "TableStyle/TableDerived";
pub const STYLE_JUSTIFIED: &str = "ParagraphStyle/JustifiedHyphen";
pub const CELL_FILL: &str = "Color/CellFill";

/// Distinct ASCII bodies so a glyph-stream / pixel assert can attribute
/// the OTF page's runs.
pub const FRACTION_TEXT: &str = "1/2 3/4";
pub const ORDINAL_TEXT: &str = "1st 2nd 3rd";
pub const CONTEXTUAL_TEXT: &str = "AVALANCHE Type";

/// Build the rich `Resources/Styles.xml` cascade. Every advanced-style
/// construct lives here; the parser keys each by `Self` regardless of
/// the `Root…Group` wrapper, so we inline one fragment.
fn styles() -> Vec<u8> {
    let fragment = format!(
        "<RootParagraphStyleGroup>\
<ParagraphStyle Self=\"{STYLE_TITLE}\" Name=\"Title\" AppliedFont=\"Open Sans\" \
PointSize=\"24\" FillColor=\"Color/Black\" NextStyle=\"{STYLE_SUBTITLE}\"/>\
<ParagraphStyle Self=\"{STYLE_SUBTITLE}\" Name=\"Subtitle\" AppliedFont=\"Open Sans\" \
PointSize=\"16\" FillColor=\"Color/Black\" NextStyle=\"{STYLE_BODY}\"/>\
<ParagraphStyle Self=\"{STYLE_BODY}\" Name=\"Body\" AppliedFont=\"Open Sans\" \
PointSize=\"12\" FillColor=\"Color/Black\" NextStyle=\"{STYLE_BODY}\"/>\
<ParagraphStyle Self=\"{STYLE_LIST_BASE}\" Name=\"ListBase\" AppliedFont=\"Open Sans\" \
PointSize=\"12\" FillColor=\"Color/Black\" BulletsAndNumberingListType=\"NumberedList\" \
AppliedNumberingList=\"{NUMBERING_LIST}\"/>\
<ParagraphStyle Self=\"{STYLE_LIST_DERIVED}\" Name=\"ListDerived\" \
BasedOn=\"{STYLE_LIST_BASE}\" PointSize=\"12\"/>\
<ParagraphStyle Self=\"{STYLE_JUSTIFIED}\" Name=\"JustifiedHyphen\" \
AppliedFont=\"Open Sans\" PointSize=\"12\" FillColor=\"Color/Black\" \
Justification=\"LeftJustified\" Hyphenation=\"true\" HyphenationZone=\"36\"/>\
</RootParagraphStyleGroup>\
<RootCellStyleGroup>\
<CellStyle Self=\"{CELL_BASE}\" Name=\"CellBase\" FillColor=\"{CELL_FILL}\" \
VerticalJustification=\"CenterAlign\"/>\
<CellStyle Self=\"{CELL_DERIVED}\" Name=\"CellDerived\" BasedOn=\"{CELL_BASE}\"/>\
</RootCellStyleGroup>\
<RootTableStyleGroup>\
<TableStyle Self=\"TableStyle/$ID/[No table style]\" Name=\"$ID/[No table style]\"/>\
<TableStyle Self=\"{TABLE_BASE}\" Name=\"TableBase\" \
BodyRegionCellStyle=\"{CELL_DERIVED}\"/>\
<TableStyle Self=\"{TABLE_DERIVED}\" Name=\"TableDerived\" BasedOn=\"{TABLE_BASE}\"/>\
</RootTableStyleGroup>\
<RootNumberingListGroup>\
<NumberingList Self=\"{NUMBERING_LIST}\" Name=\"Steps\" \
ContinueNumbersAcrossStories=\"false\" ContinueNumbersAcrossDocuments=\"false\"/>\
</RootNumberingListGroup>"
    );
    styles_xml_with_raw(&fragment)
}

/// Page 1 story — next-style chain: a Title paragraph followed by a Body
/// paragraph. (The chain itself is a typing-time editor behaviour; the
/// renderer lays out whatever style each paragraph carries.)
fn next_style_story(story_id: &str) -> Vec<u8> {
    let story = Story {
        self_id: story_id.to_string(),
        paragraphs: vec![
            styled_paragraph("Annual Report"),
            styled_paragraph("The body text follows the title."),
        ],
    };
    write_story(&story)
}

/// Page 2 story — two numbered-list paragraphs: one applying the base
/// list style, one the derived (BasedOn) style. Both bind to the same
/// `<NumberingList>` through the cascade.
fn list_cascade_story(story_id: &str) -> Vec<u8> {
    let story = Story {
        self_id: story_id.to_string(),
        paragraphs: vec![
            styled_paragraph("Base-styled step"),
            styled_paragraph("Derived-styled step"),
        ],
    };
    write_story(&story)
}

/// A single-run paragraph carrying `text`. The applied paragraph style
/// is patched in after emission (see [`patch_paragraph_styles`]) since
/// the typed `Story` builder pins `[No paragraph style]`.
fn styled_paragraph(text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: None,
            fill_color: None,
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: None,
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

/// Page 3 story — a table whose `AppliedTableStyle` is the derived table
/// style; its body cells inherit the derived cell style (BasedOn the
/// base cell style that supplies the fill).
fn table_cascade_story(story_id: &str) -> Vec<u8> {
    let cells = vec![
        Cell::plain("A1"),
        Cell::plain("A2"),
        Cell::plain("B1"),
        Cell::plain("B2"),
    ];
    let table = Table {
        self_id: format!("{story_id}_table"),
        header_row_count: 0,
        footer_row_count: 0,
        body_row_count: 2,
        column_count: 2,
        applied_table_style: Some(TABLE_DERIVED.to_string()),
        row_heights_pt: vec![40.0, 40.0],
        column_widths_pt: vec![120.0, 120.0],
        cells,
    };
    let story = Story {
        self_id: story_id.to_string(),
        paragraphs: vec![Paragraph {
            extra_paragraph_attrs: Vec::new(),
            table: Some(table),
            ..Paragraph::plain("")
        }],
    };
    write_story(&story)
}

/// Page 4 story (raw) — three OTF-bearing runs. Each
/// `<CharacterStyleRange>` carries one discrete OpenType feature
/// attribute; the parser reads them off the run via
/// `OtfFeatures::from_attrs`. `features_on` toggles the attributes so
/// the render test can diff on/off.
fn otf_story(story_id: &str, features_on: bool) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", STYLE_BODY)],
    );
    otf_run(&mut b, FRACTION_TEXT, ("OTFFraction", features_on));
    otf_run(&mut b, ORDINAL_TEXT, ("OTFOrdinal", features_on));
    // Contextual alternates default ON; the "off" variant explicitly
    // disables `calt`, so the diff still flips for this run.
    otf_run(
        &mut b,
        CONTEXTUAL_TEXT,
        ("OTFContextualAlternate", features_on),
    );
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// One OTF-feature run. `(attr, on)` becomes `attr="on"`; the contextual
/// case writes `OTFContextualAlternate="false"` for the off variant.
fn otf_run(b: &mut XmlBuilder, text: &str, feature: (&str, bool)) {
    let (attr_name, on) = feature;
    let value = if on { "true" } else { "false" };
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("AppliedFont", "Inter"),
            ("PointSize", "32"),
            (attr_name, value),
        ],
    );
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
    b.end("CharacterStyleRange");
}

/// Page 5 story — a long justified paragraph applying the
/// hyphenation-zone style.
fn hyphenation_story(story_id: &str) -> Vec<u8> {
    let body = "Internationalization and counterrevolutionary \
        characterizations notwithstanding, the typographer's \
        responsibilities encompass extraordinarily comprehensive \
        considerations regarding hyphenation, justification, and the \
        overarching presentation of multisyllabic terminology across \
        narrow measures.";
    let story = Story {
        self_id: story_id.to_string(),
        paragraphs: vec![styled_paragraph(body)],
    };
    write_story(&story)
}

/// Patch a typed-story emission to swap the pinned `[No paragraph
/// style]` for the real per-paragraph styles. The typed `Story` builder
/// doesn't carry a per-paragraph style id; rather than widen it for one
/// sample, we substitute the `AppliedParagraphStyle` values positionally
/// after emission. `styles` lists the target style for each paragraph in
/// document order.
fn patch_paragraph_styles(xml: Vec<u8>, styles: &[&str]) -> Vec<u8> {
    let text = String::from_utf8(xml).expect("story xml is utf-8");
    let needle = "AppliedParagraphStyle=\"ParagraphStyle/$ID/[No paragraph style]\"";
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    let mut i = 0;
    while let Some(pos) = rest.find(needle) {
        out.push_str(&rest[..pos]);
        let replacement = styles
            .get(i)
            .map(|s| format!("AppliedParagraphStyle=\"{s}\""))
            .unwrap_or_else(|| needle.to_string());
        out.push_str(&replacement);
        rest = &rest[pos + needle.len()..];
        i += 1;
    }
    out.push_str(rest);
    out.into_bytes()
}

/// A text-frame Rect hosting `story_id`, filling most of the page.
fn frame(seq: u32, story_id: &str) -> Rect {
    Rect {
        self_id: self_id(SAMPLE, "TextFrame", seq),
        width_pt: FRAME_W_PT,
        height_pt: FRAME_H_PT,
        item_transform: translate(
            (PAGE_W_PT - FRAME_W_PT) * 0.5,
            (PAGE_H_PT - FRAME_H_PT) * 0.5,
        ),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id.to_string()),
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

pub fn build() -> Sample {
    build_with_otf(true)
}

/// W4.9 — the OTF-features-off control. Identical to [`build`] except
/// page 4's runs carry the features disabled, so a render diff isolates
/// the shaping effect of `frac`/`ordn`/`calt`.
pub fn build_otf_off() -> Sample {
    build_with_otf(false)
}

fn build_with_otf(otf_on: bool) -> Sample {
    // Page → (story builder, paragraph-style overrides applied after
    // emission). Index aligns with the loop below.
    let names = [
        "styles-cascade · next-style",
        "styles-cascade · list-cascade",
        "styles-cascade · table-cascade",
        "styles-cascade · otf-features",
        "styles-cascade · hyphenation-zone",
    ];

    let mut master_spreads = Vec::with_capacity(names.len());
    let mut spreads = Vec::with_capacity(names.len());
    let mut stories = Vec::with_capacity(names.len());
    let mut master_refs = Vec::with_capacity(names.len());
    let mut spread_refs = Vec::with_capacity(names.len());
    let mut story_refs = Vec::with_capacity(names.len());

    for (i, name) in names.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);

        let story_bytes = match i {
            0 => patch_paragraph_styles(next_style_story(&story_id), &[STYLE_TITLE, STYLE_BODY]),
            1 => patch_paragraph_styles(
                list_cascade_story(&story_id),
                &[STYLE_LIST_BASE, STYLE_LIST_DERIVED],
            ),
            2 => table_cascade_story(&story_id),
            3 => otf_story(&story_id, otf_on),
            _ => patch_paragraph_styles(hyphenation_story(&story_id), &[STYLE_JUSTIFIED]),
        };

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id,
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        let body_frame = frame(seq, &story_id);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                item_transform: None,
                page_self_id: page_id,
                page_name: name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![body_frame.into()],
                override_list: Vec::new(),
                margins: None,
            }),
        ));
        spread_refs.push(spread_id);

        stories.push((story_id.clone(), story_bytes));
        story_refs.push(story_id);
    }

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: master_refs,
        spreads: spread_refs,
        stories: story_refs,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml_with_extras(&[ExtraColor {
            self_id: CELL_FILL.to_string(),
            name: "Cell Fill".to_string(),
            space: "CMYK",
            value: "0 0 30 0".to_string(),
        }]),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}
