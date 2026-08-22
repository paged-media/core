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

//! `showcase-base.idml` — the BASE DOCUMENT for the 16-page showcase
//! report demo.
//!
//! **This is a Tier-3 realistic document, NOT a fidelity fixture.** It
//! is deliberately absent from `corpus/generated/fidelity-thresholds.json`
//! and must stay that way: the pixel gate's fixtures are one-feature-per-
//! page mega-files sized against an InDesign-exported reference PDF, and
//! the archived brief (`thoughts/docs/old/idml-sample-generator.md`)
//! tells them to "resist the kitchen sink". This sample is the opposite
//! on purpose — one plausible publication, no reference PDF, no gate.
//!
//! ## What it is for
//!
//! A Playwright generator opens this IDML in the editor and authors the
//! whole report LIVE over the mutation wire. So the sample supplies
//! exactly what the wire cannot author, and nothing else:
//!
//!   * **Two master spreads.** `A-Editorial` (running-header frame +
//!     an auto-page-number footer, generous margins) and `B-Plate` (a
//!     bare master for full-bleed pages). `A` is applied to pages 2–4
//!     and 11–16; `B` to the plate pages 1, 5, 6, 7, 8, 9, 10. Page 11
//!     carries a per-item **override**: its `OverrideList` names the
//!     master header frame and the body page supplies its own
//!     replacement header, so the override-suppression path is live
//!     (see `masters.rs` for the established shape).
//!   * **Conditions.** `Draft` and `Print-only`, with distinct
//!     indicators, plus a `<ConditionSet>` grouping them. There is no
//!     create-condition op on the wire.
//!   * **A document `<FootnoteOption>`** with the separator rule on.
//!   * **A TOC style** with two `<TOCStyleEntry>` levels.
//!   * **Three `<Layer>` definitions** — `Background`, `Content`,
//!     `Notes`, declared bottom-first (IDML lists layers bottom-first;
//!     `designmap[0]` is the backmost band — see `paged_scene::layer`).
//!     NO page item is bound to a layer here on purpose: the editor now
//!     has `PropertyPath::ItemLayer` (protocol 62) and assigning layers
//!     live is the point of the demo. Declaring them just spares the
//!     generator from minting them first.
//!   * **Page geometry** — a `<MarginPreference>` per page: a generous
//!     72pt box with a 3-column / 18pt-gutter grid on the `A-Editorial`
//!     pages, and an explicit zero-margin single-column box on the
//!     full-bleed `B-Plate` pages.
//!   * **Named styles + swatches** the generator addresses BY NAME
//!     (`paged.paragraphStyles()` / `paged.swatches()` return
//!     `{selfId, name}`, so it never has to guess an id). Every style
//!     carries real, visually distinct properties — size, colour,
//!     alignment, spacing — so applying one is pixel-provable.
//!
//! Every page is otherwise EMPTY. The only page items in the whole
//! document are the two master frames and page 11's override header.
//!
//! ## Two behaviours worth knowing before you render it
//!
//! * `Page.Name` doubles as the page LABEL. The fidelity harness's
//!   per-page attribution reads `Page.Name`, so each page carries the
//!   usual `showcase-base · pNN · variant` descriptor — but
//!   `SectionWalk::next_label` treats a present `Name` as authoritative,
//!   so the footer's `<?ACE 18?>` marker resolves to that descriptor
//!   rather than to "3". A demo that wants numeric folios starts a
//!   section on page 1 (`insertSection`): a section edit re-bakes every
//!   `Page@Name` from its start page on, which is how InDesign keeps
//!   the derived labels true. The descriptor is kept here because the
//!   attribution convention asked for it.
//! * `Showcase Accent 20%` is built in CMYK, not RGB, even though its
//!   parent `Showcase Accent` is the RGB brand oxblood. Swatch-level
//!   `TintValue` is only honoured through `ColorEntry::effective_cmyk`,
//!   which is CMYK-only — an RGB swatch carrying a tint would paint at
//!   full strength. CMYK `0 80 78 44` is the exact naive-model build of
//!   `#8f1d1f` ((1−0.44)·255 = 143, 0.2·0.56·255 = 29, 0.22·0.56·255 =
//!   31), so the tint reads as a genuine 20% of the accent AND renders.

use crate::builders::designmap::{
    write_designmap_with_markers, DesignMap, FootnoteOptionDef, LayerDef, MarkerResources,
};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::{PageItem, Rect};
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml_rich, preferences_xml, styles_xml_with_raw, RichColor,
};
use crate::builders::spread::{write_spread, MarginPreference, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "showcase-base";

/// US Letter, portrait.
const PAGE_W_PT: f32 = 612.0;
const PAGE_H_PT: f32 = 792.0;
const PAGE_COUNT: usize = 16;

/// The editorial margin box — generous on every edge, subdivided by a
/// three-column grid. Content width = 612 − 72 − 72 = 468pt, so three
/// 144pt columns with two 18pt gutters land exactly on the box.
const MARGIN_PT: f32 = 72.0;
const COLUMN_COUNT: u32 = 3;
const COLUMN_GUTTER_PT: f32 = 18.0;
const CONTENT_W_PT: f32 = PAGE_W_PT - 2.0 * MARGIN_PT;

/// Master furniture geometry: a hairline-height header band above the
/// margin box and a footer band below it.
const FURNITURE_H_PT: f32 = 14.0;
const HEADER_Y_PT: f32 = 36.0;
const FOOTER_Y_PT: f32 = PAGE_H_PT - 48.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

// ── Names the live generator addresses. Exported so a consumer (and
//    the guard tests) refer to them without re-typing the literals. ──

pub const MASTER_EDITORIAL_NAME: &str = "A-Editorial";
pub const MASTER_PLATE_NAME: &str = "B-Plate";

pub const STYLE_TITLE: &str = "ParagraphStyle/Showcase Title";
pub const STYLE_HEADING: &str = "ParagraphStyle/Showcase Heading";
pub const STYLE_BODY: &str = "ParagraphStyle/Showcase Body";
pub const STYLE_CAPTION: &str = "ParagraphStyle/Showcase Caption";
pub const STYLE_PULLQUOTE: &str = "ParagraphStyle/Showcase Pullquote";
pub const CHAR_EMPHASIS: &str = "CharacterStyle/Showcase Emphasis";
pub const CHAR_CODE: &str = "CharacterStyle/Showcase Code";

/// Swatch self-ids carry no spaces on purpose — `ColorGroupSwatches`
/// and friends are space-separated attributes, so a spaced colour id is
/// unaddressable in a list. The user-visible `Name` carries the spaces.
pub const SWATCH_INK: &str = "Color/ShowcaseInk";
pub const SWATCH_ACCENT: &str = "Color/ShowcaseAccent";
pub const SWATCH_ACCENT_TINT: &str = "Color/ShowcaseAccentTint";
pub const SWATCH_INK_NAME: &str = "Showcase Ink";
pub const SWATCH_ACCENT_NAME: &str = "Showcase Accent";
pub const SWATCH_ACCENT_TINT_NAME: &str = "Showcase Accent 20%";

/// `AppliedConditions` is a space-separated attribute, so condition
/// self-ids must not carry spaces either.
pub const CONDITION_DRAFT: &str = "Condition/Draft";
pub const CONDITION_PRINT_ONLY: &str = "Condition/PrintOnly";
pub const CONDITION_SET_REVIEW: &str = "ConditionSet/ShowcaseReview";

pub const TOC_STYLE: &str = "TOCStyle/Showcase Contents";

pub const LAYER_BACKGROUND_NAME: &str = "Background";
pub const LAYER_CONTENT_NAME: &str = "Content";
pub const LAYER_NOTES_NAME: &str = "Notes";

/// The RGB brand oxblood, `#8f1d1f`, on IDML's 0..255 RGB scale.
const ACCENT_RGB: &str = "143 29 31";
/// The same oxblood as a CMYK build — the tint swatch's base (see the
/// module doc: swatch tints only scale through the CMYK path).
const ACCENT_CMYK: &str = "0 80 78 44";

/// Which master a page applies, and therefore which margin box it gets.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// `A-Editorial` — header + folio furniture, 3-column margin box.
    Editorial,
    /// `B-Plate` — no furniture, full-bleed zero-margin box.
    Plate,
}

/// The 16-page plan: per page, its master role and the `Page.Name`
/// detail token. Plate pages are 1, 5, 6, 7, 8, 9, 10 (1-based).
fn page_plan() -> Vec<(Role, &'static str)> {
    vec![
        (Role::Plate, "plate-cover"),            // p01
        (Role::Editorial, "editorial-contents"), // p02
        (Role::Editorial, "editorial"),          // p03
        (Role::Editorial, "editorial"),          // p04
        (Role::Plate, "plate"),                  // p05
        (Role::Plate, "plate"),                  // p06
        (Role::Plate, "plate"),                  // p07
        (Role::Plate, "plate"),                  // p08
        (Role::Plate, "plate"),                  // p09
        (Role::Plate, "plate"),                  // p10
        (Role::Editorial, "editorial-override"), // p11
        (Role::Editorial, "editorial"),          // p12
        (Role::Editorial, "editorial"),          // p13
        (Role::Editorial, "editorial"),          // p14
        (Role::Editorial, "editorial"),          // p15
        (Role::Editorial, "editorial"),          // p16
    ]
}

/// The 0-based body page that overrides the master's running-header
/// frame — page 11, where the report's second half begins.
const OVERRIDE_PAGE_IDX: usize = 10;

// ── Resources ────────────────────────────────────────────────────────

/// `Resources/Graphic.xml` — the three named brand swatches on top of
/// the built-in Black + Paper.
fn graphic() -> Vec<u8> {
    let colors = [
        RichColor {
            self_id: SWATCH_INK.to_string(),
            name: SWATCH_INK_NAME.to_string(),
            model: "Process",
            // A rich editorial black rather than flat K=100 — the text
            // colour the whole report is set in.
            space: "CMYK",
            value: "72 62 58 90".to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
        },
        RichColor {
            self_id: SWATCH_ACCENT.to_string(),
            name: SWATCH_ACCENT_NAME.to_string(),
            model: "Process",
            space: "RGB",
            value: ACCENT_RGB.to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
        },
        RichColor {
            self_id: SWATCH_ACCENT_TINT.to_string(),
            name: SWATCH_ACCENT_TINT_NAME.to_string(),
            model: "Process",
            // CMYK, not RGB — see the module doc. Swatch-level tints
            // only scale through `ColorEntry::effective_cmyk`.
            space: "CMYK",
            value: ACCENT_CMYK.to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: Some(20.0),
        },
    ];
    graphic_xml_rich(&colors, &[], &[])
}

/// `Resources/Styles.xml` — the named paragraph + character styles, the
/// conditional-text group, and the TOC style, spliced into the default
/// styles manifest as one raw fragment. The parser keys every style
/// element by `Self` regardless of which `Root…Group` wrapper holds it,
/// so a single fragment is enough (same idiom as `styles_cascade.rs` /
/// `navigation.rs`).
///
/// `<Leading>` is emitted as a typed `<Properties>` child because that
/// is where InDesign puts it. The engine's paragraph cascade does not
/// model leading (`ParagraphStyleDef` has no `leading` field — runs
/// carry it), so it is carried for InDesign + round-trip fidelity, not
/// for the renderer.
fn styles() -> Vec<u8> {
    let fragment = format!(
        "<RootCharacterStyleGroup>\
<CharacterStyle Self=\"{CHAR_EMPHASIS}\" Name=\"Showcase Emphasis\" \
AppliedFont=\"Open Sans\" FontStyle=\"Italic\" FillColor=\"{SWATCH_ACCENT}\"/>\
<CharacterStyle Self=\"{CHAR_CODE}\" Name=\"Showcase Code\" \
AppliedFont=\"Open Sans\" FontStyle=\"Bold\" FillColor=\"{SWATCH_INK}\" \
Tracking=\"20\"/>\
</RootCharacterStyleGroup>\
<RootParagraphStyleGroup>\
<ParagraphStyle Self=\"{STYLE_TITLE}\" Name=\"Showcase Title\" \
AppliedFont=\"Open Sans\" PointSize=\"42\" FillColor=\"{SWATCH_INK}\" \
Justification=\"LeftAlign\" Tracking=\"-15\" SpaceAfter=\"18\" \
NextStyle=\"{STYLE_BODY}\">\
<Properties><Leading type=\"unit\">46</Leading></Properties>\
</ParagraphStyle>\
<ParagraphStyle Self=\"{STYLE_HEADING}\" Name=\"Showcase Heading\" \
AppliedFont=\"Open Sans\" PointSize=\"18\" FillColor=\"{SWATCH_ACCENT}\" \
Justification=\"LeftAlign\" SpaceBefore=\"18\" SpaceAfter=\"6\" \
NextStyle=\"{STYLE_BODY}\">\
<Properties><Leading type=\"unit\">22</Leading></Properties>\
</ParagraphStyle>\
<ParagraphStyle Self=\"{STYLE_BODY}\" Name=\"Showcase Body\" \
AppliedFont=\"Open Sans\" PointSize=\"10\" FillColor=\"{SWATCH_INK}\" \
Justification=\"LeftJustified\" Hyphenation=\"true\" HyphenationZone=\"36\" \
SpaceAfter=\"4\" NextStyle=\"{STYLE_BODY}\">\
<Properties><Leading type=\"unit\">14</Leading></Properties>\
</ParagraphStyle>\
<ParagraphStyle Self=\"{STYLE_CAPTION}\" Name=\"Showcase Caption\" \
AppliedFont=\"Open Sans\" PointSize=\"7.5\" FillColor=\"{SWATCH_ACCENT}\" \
Justification=\"LeftAlign\" Tracking=\"40\" SpaceBefore=\"4\" \
NextStyle=\"{STYLE_BODY}\">\
<Properties><Leading type=\"unit\">10</Leading></Properties>\
</ParagraphStyle>\
<ParagraphStyle Self=\"{STYLE_PULLQUOTE}\" Name=\"Showcase Pullquote\" \
AppliedFont=\"Open Sans\" FontStyle=\"Italic\" PointSize=\"20\" \
FillColor=\"{SWATCH_ACCENT}\" Justification=\"CenterAlign\" \
LeftIndent=\"18\" RightIndent=\"18\" SpaceBefore=\"12\" SpaceAfter=\"12\" \
NextStyle=\"{STYLE_BODY}\">\
<Properties><Leading type=\"unit\">26</Leading></Properties>\
</ParagraphStyle>\
</RootParagraphStyleGroup>\
<RootConditionalTextGroup>\
<Condition Self=\"{CONDITION_DRAFT}\" Name=\"Draft\" Visible=\"true\" \
IndicatorMethod=\"UseHighlight\" IndicatorColor=\"Yellow\"/>\
<Condition Self=\"{CONDITION_PRINT_ONLY}\" Name=\"Print-only\" Visible=\"true\" \
IndicatorMethod=\"UseUnderline\" IndicatorColor=\"Green\"/>\
<ConditionSet Self=\"{CONDITION_SET_REVIEW}\" Name=\"Review\" \
Conditions=\"{CONDITION_DRAFT} {CONDITION_PRINT_ONLY}\"/>\
</RootConditionalTextGroup>\
<RootTOCStyleGroup>\
<TOCStyle Self=\"{TOC_STYLE}\" Name=\"Showcase Contents\" Title=\"Contents\" \
TitleStyle=\"{STYLE_TITLE}\">\
<TOCStyleEntry Name=\"Showcase Title\" IncludeStyle=\"{STYLE_TITLE}\" \
FormatStyle=\"{STYLE_BODY}\" Level=\"1\" PageNumber=\"On\" Separator=\"^t\"/>\
<TOCStyleEntry Name=\"Showcase Heading\" IncludeStyle=\"{STYLE_HEADING}\" \
FormatStyle=\"{STYLE_BODY}\" Level=\"2\" PageNumber=\"On\" Separator=\"^t\"/>\
</TOCStyle>\
</RootTOCStyleGroup>"
    );
    styles_xml_with_raw(&fragment)
}

// ── Stories (master furniture only — every body page is empty) ───────

/// One segment of a furniture line: literal text, or the IDML
/// auto-current-page-number marker.
enum Seg {
    Text(&'static str),
    /// `<?ACE 18?>` — the parser maps it to
    /// `paged_model::AUTO_PAGE_NUMBER_MARKER` and the renderer
    /// substitutes the host page's label at emit time.
    PageNumber,
}

/// A one-paragraph story for a piece of master (or override) furniture.
fn write_line_story(
    story_id: &str,
    paragraph_style: &str,
    justification: &str,
    segments: &[Seg],
) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start(
        "ParagraphStyleRange",
        &[
            ("AppliedParagraphStyle", paragraph_style),
            ("Justification", justification),
        ],
    );
    b.start(
        "CharacterStyleRange",
        &[("AppliedCharacterStyle", NO_CHAR_STYLE)],
    );
    b.start("Content", &[]);
    for seg in segments {
        match seg {
            Seg::Text(t) => b.text(t),
            // Emitted inside <Content> with a run already open — the
            // only shape the story parser recognises.
            Seg::PageNumber => b.write_pi("ACE", "18"),
        }
    }
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

// ── Page items ───────────────────────────────────────────────────────

/// A furniture text frame: no fill, no stroke, hosting `story_id`.
/// Three call sites (master header, master footer, the page-11 override
/// header) all want the same shape, so the 20-field `Rect` literal is
/// spelled once.
fn furniture_frame(self_id: String, y_pt: f32, story_id: &str) -> Rect {
    Rect {
        self_id,
        width_pt: CONTENT_W_PT,
        height_pt: FURNITURE_H_PT,
        item_transform: translate(MARGIN_PT, y_pt),
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

/// Emit a master spread whose `<MasterSpread>` carries a real `Name`
/// (plus InDesign's `NamePrefix` / `BaseName` split).
///
/// The shared [`write_master`] builder pins `Name="$ID/None"` because
/// every prior sample's masters are anonymous one-per-page throwaways;
/// widening its `Master` struct would touch ~30 existing literals for
/// one caller's benefit. This is the first sample whose masters are
/// NAMED furniture the demo and the editor's Pages panel address, so
/// the attribute is spliced in locally — the same cheap, sample-local
/// rewrite `masters.rs` uses for `ShowMasterItems`.
fn write_named_master(m: &Master, prefix: &str, base_name: &str) -> Vec<u8> {
    let bare = m
        .self_id
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(m.self_id.as_str());
    let xml = String::from_utf8(write_master(m)).expect("master xml is utf-8");
    let needle = format!("<MasterSpread Self=\"{bare}\" Name=\"$ID/None\"");
    let replacement = format!(
        "<MasterSpread Self=\"{bare}\" Name=\"{prefix}-{base_name}\" \
NamePrefix=\"{prefix}\" BaseName=\"{base_name}\""
    );
    xml.replacen(&needle, &replacement, 1).into_bytes()
}

// ── Build ────────────────────────────────────────────────────────────

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    // Masters.
    let editorial_master_id = self_id(SAMPLE, "MasterSpread", 0);
    let plate_master_id = self_id(SAMPLE, "MasterSpread", 1);
    let editorial_master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let plate_master_page_id = self_id(SAMPLE, "MasterPage", 1);

    // Master furniture frames + their stories.
    let header_frame_id = self_id(SAMPLE, "MasterHeader", 0);
    let footer_frame_id = self_id(SAMPLE, "MasterFooter", 0);
    let override_frame_id = self_id(SAMPLE, "OverrideHeader", 0);
    let header_story_id = self_id(SAMPLE, "Story", 0);
    let footer_story_id = self_id(SAMPLE, "Story", 1);
    let override_story_id = self_id(SAMPLE, "Story", 2);

    let stories: Vec<(String, Vec<u8>)> = vec![
        (
            header_story_id.clone(),
            write_line_story(
                &header_story_id,
                STYLE_CAPTION,
                "LeftAlign",
                &[Seg::Text("Paged Showcase Report")],
            ),
        ),
        (
            footer_story_id.clone(),
            write_line_story(
                &footer_story_id,
                STYLE_CAPTION,
                "RightAlign",
                &[Seg::PageNumber],
            ),
        ),
        (
            override_story_id.clone(),
            // The page-11 override needs its OWN story: the master-text
            // pass skips a master frame whose story already has a body
            // frame anywhere in the document, so reusing the header
            // story here would suppress the running header on all nine
            // editorial pages instead of just this one.
            write_line_story(
                &override_story_id,
                STYLE_HEADING,
                "LeftAlign",
                &[Seg::Text("Part Two · Plates and Notes")],
            ),
        ),
    ];

    let editorial_master = write_named_master(
        &Master {
            self_id: format!("MasterSpread/{editorial_master_id}"),
            page_self_id: editorial_master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: vec![
                furniture_frame(header_frame_id.clone(), HEADER_Y_PT, &header_story_id).into(),
                furniture_frame(footer_frame_id, FOOTER_Y_PT, &footer_story_id).into(),
            ],
        },
        "A",
        "Editorial",
    );
    // `B-Plate` carries no furniture at all — full-bleed pages inherit
    // nothing to suppress.
    let plate_master = write_named_master(
        &Master {
            self_id: format!("MasterSpread/{plate_master_id}"),
            page_self_id: plate_master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        },
        "B",
        "Plate",
    );

    let plan = page_plan();
    debug_assert_eq!(plan.len(), PAGE_COUNT);
    let mut spreads: Vec<(String, Vec<u8>)> = Vec::with_capacity(plan.len());
    let mut spread_refs: Vec<String> = Vec::with_capacity(plan.len());

    for (i, (role, detail)) in plan.iter().enumerate() {
        let seq = i as u32;
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);

        let (applied_master, margins) = match role {
            Role::Editorial => (
                &editorial_master_id,
                MarginPreference {
                    top: MARGIN_PT,
                    bottom: MARGIN_PT,
                    left: MARGIN_PT,
                    right: MARGIN_PT,
                    column_count: COLUMN_COUNT,
                    column_gutter: COLUMN_GUTTER_PT,
                },
            ),
            // Full bleed: an explicit zero margin box, single column.
            Role::Plate => (
                &plate_master_id,
                MarginPreference::symmetric(0.0, 0.0, 0.0, 0.0),
            ),
        };

        // One page overrides the master's running-header frame and
        // supplies its own — the override-suppression path (Q-14).
        let (page_items, override_list): (Vec<PageItem>, Vec<String>) = if i == OVERRIDE_PAGE_IDX {
            (
                vec![
                    furniture_frame(override_frame_id.clone(), HEADER_Y_PT, &override_story_id)
                        .into(),
                ],
                vec![header_frame_id.clone()],
            )
        } else {
            (Vec::new(), Vec::new())
        };

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: format!("{SAMPLE} · p{:02} · {detail}", i + 1),
                applied_master: format!("MasterSpread/{applied_master}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items,
                override_list,
                margins: Some(margins),
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
    }

    // Layers are declared BOTTOM-FIRST (see the module doc). No page
    // item binds to one: the generator assigns them live over
    // `PropertyPath::ItemLayer`.
    let markers = MarkerResources {
        layers: vec![
            LayerDef {
                self_id: self_id(SAMPLE, "Layer", 0),
                name: LAYER_BACKGROUND_NAME.to_string(),
            },
            LayerDef {
                self_id: self_id(SAMPLE, "Layer", 1),
                name: LAYER_CONTENT_NAME.to_string(),
            },
            LayerDef {
                self_id: self_id(SAMPLE, "Layer", 2),
                name: LAYER_NOTES_NAME.to_string(),
            },
        ],
        // There is no create-footnote-option op on the wire either, so
        // the separator rule has to be authored here.
        footnote_option: Some(FootnoteOptionDef {
            rule_on: Some(true),
            rule_color: Some(SWATCH_ACCENT.to_string()),
            rule_line_weight: Some(0.5),
            rule_width: Some(144.0),
            rule_left_indent: Some(0.0),
            rule_offset: Some(3.0),
            separator_text: Some(" ".to_string()),
            space_between: Some(2.0),
            spacer: Some(6.0),
            ..FootnoteOptionDef::default()
        }),
        ..MarkerResources::default()
    };

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![editorial_master_id.clone(), plate_master_id.clone()],
            spreads: spread_refs,
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        },
        &markers,
    );

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic(),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![
            (editorial_master_id, editorial_master),
            (plate_master_id, plate_master),
        ],
        spreads,
        stories,
    }
}
