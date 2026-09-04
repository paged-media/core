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

//! `annual-base.idml` — the BASE DOCUMENT for the 134-page "Paged
//! Annual" specimen book.
//!
//! **This is a Tier-3 realistic document, NOT a fidelity fixture** —
//! the same standing as `showcase_base`: deliberately absent from
//! `corpus/generated/fidelity-thresholds.json` (one plausible
//! publication, no InDesign reference PDF, no pixel gate). A live
//! generator opens it in the editor and authors the whole specimen
//! book over the mutation wire, so the fixture supplies exactly what
//! the wire cannot author — masters, margins, styles, swatches,
//! conditions, navigation resources — and nothing else. Every body
//! page is EMPTY except the handful of fixture-authored exhibit
//! frames listed below.
//!
//! ## Facing spreads — the real thing, verified
//!
//! This is the first sample built on the facing-spread builders
//! (`spread::FacingSpread` / `master::FacingMaster`): p1 is a
//! single-page cover spread, p2–p133 are 66 verso+recto reader
//! spreads, p134 a single closing verso. The lane was proved through
//! the real pipeline BEFORE this sample existed (the
//! `facing_spread_routes_items_and_master_furniture_per_side`
//! snapshot test): body items route to the page containing their
//! centroid, a facing master's per-side furniture stamps onto
//! same-side body pages only, and per-page margins mirror across the
//! fold. No fallback to single-page spreads was needed.
//!
//! ## Geometry
//!
//! Trim 540×720 pt portrait on a 13 pt baseline rhythm. Margins
//! (recto): top 54, bottom 81, inside 48, outside 60 — MIRRORED on the
//! verso. Live width 432 pt carries three grids: body pages 6
//! columns / 12 pt gutter (6×62 + 5×12), data pages 12 columns / 12 pt
//! (12×25 + 11×12), appendix pages 2 columns. Every style's leading is
//! a multiple of 13 and space-before + space-after sums to multiples
//! of 13, so live-authored text can sit the grid.
//!
//! **No bleed is declared.** `preferences_xml()` emits an empty
//! `Resources/Preferences.xml` manifest — the builder family has no
//! `DocumentPreference` bleed knobs — so trim = media here; bleed is
//! export business (paged-export-pdf's `BleedOptions` carries it onto
//! the PDF/X box chain).
//!
//! ## Masters (all seven FACING, distinct verso/recto furniture)
//!
//!   * `A-Front` — front matter: single centred column, a centred drop
//!     folio per side, no running head.
//!   * `B-Body` — the workhorse: verso running head "THE PAGED ANNUAL
//!     · MMXXVI" + folio at the outside edge; recto running head is a
//!     real **`RunningHeaderType` text variable** (pickup style
//!     `Chapter Title`, FirstOnPage) + outside folio. The variable's
//!     baked `ResultText` ("Chapter") shows until the live generator
//!     authors Chapter Title paragraphs for it to pick up.
//!   * `C-Opener` — chapter openers: drop folios, a recto slug band,
//!     no running head.
//!   * `D-Plate` — zero furniture (full-bleed plate pages inherit
//!     nothing to suppress).
//!   * `E-Data` — B-style furniture over the 12-column fine grid.
//!   * `F-Vertical` — folios only (the vertical-writing exhibit pages
//!     author their own tall frame live).
//!   * `G-Appendix` — small furniture: a small-caps section head +
//!     folio per side over the 2-column reference grid.
//!
//! p15 carries the fixture-authored master-item OVERRIDE: its
//! `OverrideList` names `B-Body`'s recto running-head frame and the
//! page supplies its own replacement head — the same Q-14
//! override-suppression shape as `showcase_base` p11 (and the
//! replacement has its OWN story, for the same master-text-pass
//! reason documented there).
//!
//! ## Behaviours worth knowing before you render it
//!
//!   * `Page.Name` doubles as the page LABEL (`annual-base · pNNN ·
//!     <role>` here, for per-page attribution), and
//!     `SectionWalk::next_label` treats a present `Name` as
//!     authoritative — so the folio `<?ACE 18?>` markers resolve to
//!     these descriptors until a section exists. The fixture ships
//!     with **no `<Section>` on purpose**: a live `insertSection` on
//!     p1 re-bakes every `Page@Name` from its start page on (the
//!     sections mutation re-derives labels), which is how the demo
//!     turns the descriptors into clean numeric folios.
//!   * `Vermilion 20%` is a `<Tint>` element with `BaseColor` pointing
//!     at the spot, which is the only tint spelling InDesign reads. It
//!     is built in CMYK, not RGB, because the tint scales through
//!     `ColorEntry::effective_cmyk`, which is CMYK-only — an RGB swatch
//!     carrying a tint paints at full strength (the same gotcha
//!     `showcase_base` documents). It used to be a `<Color>` carrying a
//!     `TintValue` attribute, which our own reader honours and Adobe's
//!     silently discards: opened in real InDesign, every panel wearing
//!     it printed at full strength.
//!   * `Screen Blue` is the deliberate RGB-warning specimen (an RGB
//!     swatch in a print document) and is therefore NOT a member of
//!     the "Annual Brand" colour group.
//!
//! ## Honest limitations (checked against the parser, not guessed)
//!
//!   * ◪ **Condition sets carry membership only.** The parsed
//!     `ConditionSetDef` models `Conditions="…"` (member list) — a
//!     per-set per-condition visibility snapshot is not expressible.
//!     So "Press" lists just its ON condition (Print-only) and
//!     "Working Copy" lists all three; the OFF states the brief wants
//!     are implied by absence, not stored.
//!   * ◪ **Object styles model fill / stroke / corner only** — "Spec
//!     Panel"'s text inset is not on `ObjectStyleDef`, so the style
//!     ships fill + rounded corners without an inset.
//!   * ◪ **Style-level leading is carried, not cascaded**: `<Leading>`
//!     is emitted as the typed `<Properties>` child (where InDesign
//!     puts it) for round-trip fidelity, but the engine's paragraph
//!     cascade doesn't model style leading — runs carry leading at
//!     authoring time. Same for `KeepWithNext` on `Head 1`.

use crate::builders::designmap::{
    write_designmap_with_markers, BookmarkDef, DesignMap, FootnoteOptionDef, HyperlinkDef,
    HyperlinkDestinationDef, IndexTopicDef, LayerDef, MarkerResources, TextVariableDef,
};
use crate::builders::master::{write_facing_master, FacingMaster};
use crate::builders::page_item::{PageItem, Rect};
use crate::builders::resources::{
    container_xml, graphic_xml_rich_full, preferences_xml, styles_xml_with_raw, ColorGroupSpec,
    ExtraGradient, GradientStop, RichColor,
};
use crate::builders::spread::{
    write_facing_spread, write_spread, FacingPage, FacingSpread, MarginPreference, Spread,
};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;
use std::collections::HashMap;

const SAMPLE: &str = "annual-base";

/// Trim size, portrait.
const PAGE_W_PT: f32 = 540.0;
const PAGE_H_PT: f32 = 720.0;
pub const PAGE_COUNT: usize = 134;

/// The mirrored margin box. Inside = spine side (recto left / verso
/// right), outside = fore-edge.
const MARGIN_TOP_PT: f32 = 54.0;
const MARGIN_BOTTOM_PT: f32 = 81.0;
const MARGIN_INSIDE_PT: f32 = 48.0;
const MARGIN_OUTSIDE_PT: f32 = 60.0;
/// 540 − 48 − 60 — shared by the 6-col body grid (6×62 + 5×12) and
/// the 12-col data grid (12×25 + 11×12).
const LIVE_W_PT: f32 = PAGE_W_PT - MARGIN_INSIDE_PT - MARGIN_OUTSIDE_PT;
/// Front-matter pages centre a single column instead of mirroring.
const FRONT_MARGIN_PT: f32 = 54.0;
const COLUMN_GUTTER_PT: f32 = 12.0;

/// Master furniture geometry: a running-head band above the margin
/// box, a folio band inside the bottom margin. 27 + 14 clears the
/// 54 pt head margin; 647 sits in the 81 pt foot (639…720).
const FURNITURE_H_PT: f32 = 14.0;
const HEAD_Y_PT: f32 = 27.0;
const FOLIO_Y_PT: f32 = 647.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

// ── Names the live generator (and the guard tests) address ──────────

pub const MASTER_FRONT_NAME: &str = "A-Front";
pub const MASTER_BODY_NAME: &str = "B-Body";
pub const MASTER_OPENER_NAME: &str = "C-Opener";
pub const MASTER_PLATE_NAME: &str = "D-Plate";
pub const MASTER_DATA_NAME: &str = "E-Data";
pub const MASTER_VERTICAL_NAME: &str = "F-Vertical";
pub const MASTER_APPENDIX_NAME: &str = "G-Appendix";
pub const MASTER_NAMES: [&str; 7] = [
    MASTER_FRONT_NAME,
    MASTER_BODY_NAME,
    MASTER_OPENER_NAME,
    MASTER_PLATE_NAME,
    MASTER_DATA_NAME,
    MASTER_VERTICAL_NAME,
    MASTER_APPENDIX_NAME,
];

// Paragraph styles — the full cascade roots from "Annual Body".
pub const STYLE_BODY: &str = "ParagraphStyle/Annual Body";
pub const STYLE_BODY_FIRST: &str = "ParagraphStyle/Body First";
pub const STYLE_BODY_SMALL: &str = "ParagraphStyle/Body Small";
pub const STYLE_FOOTNOTE: &str = "ParagraphStyle/Footnote";
pub const STYLE_CAPTION: &str = "ParagraphStyle/Caption";
pub const STYLE_MARGIN_NOTE: &str = "ParagraphStyle/Margin Note";
pub const STYLE_CODE_BLOCK: &str = "ParagraphStyle/Code Block";
pub const STYLE_BULLET_LIST: &str = "ParagraphStyle/Bullet List";
pub const STYLE_NUMBERED_1: &str = "ParagraphStyle/Numbered 1";
pub const STYLE_NUMBERED_2: &str = "ParagraphStyle/Numbered 2";
pub const STYLE_CATALOG_ENTRY: &str = "ParagraphStyle/Catalog Entry";
pub const STYLE_TABLE_HEAD: &str = "ParagraphStyle/Table Head";
pub const STYLE_TABLE_BODY: &str = "ParagraphStyle/Table Body";
pub const STYLE_TABLE_NUMBER: &str = "ParagraphStyle/Table Number";
pub const STYLE_TOC_PART: &str = "ParagraphStyle/TOC Part";
pub const STYLE_TOC_CHAPTER: &str = "ParagraphStyle/TOC Chapter";
pub const STYLE_TOC_HEAD: &str = "ParagraphStyle/TOC Head";
pub const STYLE_INDEX_ENTRY: &str = "ParagraphStyle/Index Entry";
pub const STYLE_INDEX_SUB: &str = "ParagraphStyle/Index Sub";
pub const STYLE_SPEC_LABEL: &str = "ParagraphStyle/Spec Label";
pub const STYLE_SPEC_VALUE: &str = "ParagraphStyle/Spec Value";
pub const STYLE_CHAPTER_NUMBER: &str = "ParagraphStyle/Chapter Number";
pub const STYLE_CHAPTER_TITLE: &str = "ParagraphStyle/Chapter Title";
pub const STYLE_DECK: &str = "ParagraphStyle/Deck";
pub const STYLE_HEAD_1: &str = "ParagraphStyle/Head 1";
pub const STYLE_HEAD_2: &str = "ParagraphStyle/Head 2";
pub const STYLE_PULL_QUOTE: &str = "ParagraphStyle/Pull Quote";
pub const STYLE_PART_TITLE: &str = "ParagraphStyle/Part Title";
pub const STYLE_FOLIO: &str = "ParagraphStyle/Folio";
pub const STYLE_RUNNING_HEAD: &str = "ParagraphStyle/Running Head";
pub const STYLE_COLOPHON: &str = "ParagraphStyle/Colophon";
pub const STYLE_SPECIMEN_NO: &str = "ParagraphStyle/Specimen No";
pub const PARAGRAPH_STYLES: [&str; 32] = [
    STYLE_BODY,
    STYLE_BODY_FIRST,
    STYLE_BODY_SMALL,
    STYLE_FOOTNOTE,
    STYLE_CAPTION,
    STYLE_MARGIN_NOTE,
    STYLE_CODE_BLOCK,
    STYLE_BULLET_LIST,
    STYLE_NUMBERED_1,
    STYLE_NUMBERED_2,
    STYLE_CATALOG_ENTRY,
    STYLE_TABLE_HEAD,
    STYLE_TABLE_BODY,
    STYLE_TABLE_NUMBER,
    STYLE_TOC_PART,
    STYLE_TOC_CHAPTER,
    STYLE_TOC_HEAD,
    STYLE_INDEX_ENTRY,
    STYLE_INDEX_SUB,
    STYLE_SPEC_LABEL,
    STYLE_SPEC_VALUE,
    STYLE_CHAPTER_NUMBER,
    STYLE_CHAPTER_TITLE,
    STYLE_DECK,
    STYLE_HEAD_1,
    STYLE_HEAD_2,
    STYLE_PULL_QUOTE,
    STYLE_PART_TITLE,
    STYLE_FOLIO,
    STYLE_RUNNING_HEAD,
    STYLE_COLOPHON,
    STYLE_SPECIMEN_NO,
];

// Character styles.
pub const CHAR_EMPHASIS: &str = "CharacterStyle/Annual Emphasis";
pub const CHAR_STRONG: &str = "CharacterStyle/Annual Strong";
pub const CHAR_SMALL_CAPS: &str = "CharacterStyle/Small Caps";
pub const CHAR_CODE_INLINE: &str = "CharacterStyle/Code Inline";
pub const CHAR_LEAD_IN: &str = "CharacterStyle/Lead-In";
pub const CHAR_SUPERIOR: &str = "CharacterStyle/Superior";
pub const CHAR_URL: &str = "CharacterStyle/URL";
pub const CHAR_ACCENT_INK: &str = "CharacterStyle/Accent Ink";
pub const CHAR_SPECIMEN_NUMBER: &str = "CharacterStyle/Specimen Number";
pub const CHARACTER_STYLES: [&str; 9] = [
    CHAR_EMPHASIS,
    CHAR_STRONG,
    CHAR_SMALL_CAPS,
    CHAR_CODE_INLINE,
    CHAR_LEAD_IN,
    CHAR_SUPERIOR,
    CHAR_URL,
    CHAR_ACCENT_INK,
    CHAR_SPECIMEN_NUMBER,
];

// Object styles.
pub const OBJECT_PLATE_FRAME: &str = "ObjectStyle/Plate Frame";
pub const OBJECT_SPEC_PANEL: &str = "ObjectStyle/Spec Panel";
pub const OBJECT_ANNOTATION_MARKER: &str = "ObjectStyle/Annotation Marker";
pub const OBJECT_STYLES: [&str; 3] = [
    OBJECT_PLATE_FRAME,
    OBJECT_SPEC_PANEL,
    OBJECT_ANNOTATION_MARKER,
];

// Table + cell styles.
pub const TABLE_STYLE_ANNUAL: &str = "TableStyle/Annual Table";
pub const CELL_TH: &str = "CellStyle/Annual TH";
pub const CELL_TD: &str = "CellStyle/Annual TD";
pub const CELL_TD_NUMBER: &str = "CellStyle/Annual TD Number";

/// Swatch self-ids carry no spaces (`ColorGroupSwatches` and friends
/// are space-separated attributes); the user-visible `Name` does.
pub const SWATCH_INK: &str = "Color/AnnualInk";
pub const SWATCH_PAPER_WARM: &str = "Color/AnnualPaperWarm";
pub const SWATCH_VERMILION: &str = "Color/AnnualVermilion";
pub const SWATCH_VERMILION_TINT: &str = "Color/AnnualVermilion20";
pub const SWATCH_SLATE: &str = "Color/AnnualSlate";
pub const SWATCH_LAB_MARIGOLD: &str = "Color/AnnualLabMarigold";
pub const SWATCH_SCREEN_BLUE: &str = "Color/AnnualScreenBlue";
pub const SWATCH_INK_NAME: &str = "Annual Ink";
pub const SWATCH_PAPER_WARM_NAME: &str = "Paper Warm";
pub const SWATCH_VERMILION_NAME: &str = "Vermilion";
pub const SWATCH_VERMILION_TINT_NAME: &str = "Vermilion 20%";
pub const SWATCH_SLATE_NAME: &str = "Slate";
pub const SWATCH_LAB_MARIGOLD_NAME: &str = "Lab Marigold";
pub const SWATCH_SCREEN_BLUE_NAME: &str = "Screen Blue";
pub const GRADIENT_RAMP: &str = "Gradient/AnnualRamp";
pub const GRADIENT_RAMP_NAME: &str = "Annual Ramp";
pub const COLOR_GROUP_BRAND: &str = "ColorGroup/AnnualBrand";
pub const COLOR_GROUP_BRAND_NAME: &str = "Annual Brand";

/// The vermilion CMYK build — the spot's alternate AND the tint
/// swatch's base (tints only scale through the CMYK path).
const VERMILION_CMYK: &str = "0 85 90 5";

// Conditions (ids space-free: `AppliedConditions` is space-separated).
pub const CONDITION_PRINT_ONLY: &str = "Condition/AnnualPrintOnly";
pub const CONDITION_SCREEN_ONLY: &str = "Condition/AnnualScreenOnly";
pub const CONDITION_SPEC_NOTES: &str = "Condition/AnnualSpecNotes";
pub const CONDITION_SET_PRESS: &str = "ConditionSet/AnnualPress";
pub const CONDITION_SET_WORKING_COPY: &str = "ConditionSet/AnnualWorkingCopy";

pub const TOC_STYLE: &str = "TOCStyle/Annual Contents";

/// Bottom-first, IDML's layer order (`designmap[0]` is the backmost
/// band). Declared only — no fixture item binds to one, because layer
/// assignment over `PropertyPath::ItemLayer` is part of the live demo.
pub const LAYER_NAMES: [&str; 5] = ["Grid", "Background", "Content", "Annotations", "Notes"];

// Navigation battery.
pub const BOOKMARK_APPARATUS: &str = "Bookmark/AnnualApparatus";
pub const BOOKMARK_CHAPTER_ONE: &str = "Bookmark/AnnualChapterOne";
pub const BOOKMARK_DATA_TABLES: &str = "Bookmark/AnnualDataTables";
pub const BOOKMARK_NAMES: [&str; 3] = ["Apparatus", "Chapter One", "Data Tables"];

/// `(self_id, name)` for the ten index `<Topic>` definitions.
pub const INDEX_TOPICS: [(&str, &str); 10] = [
    ("Topic/AnnualTypography", "Typography"),
    ("Topic/AnnualBaskerville", "Baskerville"),
    ("Topic/AnnualGrids", "Grids"),
    ("Topic/AnnualSpotColour", "Spot colour"),
    ("Topic/AnnualLabColour", "Lab colour"),
    ("Topic/AnnualFootnotes", "Footnotes"),
    ("Topic/AnnualDataTables", "Data tables"),
    ("Topic/AnnualVerticalWriting", "Vertical writing"),
    ("Topic/AnnualAppendices", "Appendices"),
    ("Topic/AnnualColophon", "Colophon"),
];

// Text variables.
pub const VAR_EDITION: &str = "TextVariable/AnnualEdition";
pub const VAR_RUNNING_HEADER: &str = "TextVariable/AnnualRunningHeader";
pub const VAR_CHAPTER: &str = "TextVariable/AnnualChapter";
pub const VAR_PAGE_COUNT: &str = "TextVariable/AnnualPages";

/// The B-Body / E-Data verso running head (and the p15 replacement's
/// verification foil).
pub const RUNNING_HEAD_VERSO_TEXT: &str = "THE PAGED ANNUAL · MMXXVI";
/// The p15 override's replacement recto running head.
pub const OVERRIDE_HEAD_TEXT: &str = "SPECIMENS RECONSIDERED · AN OVERRIDE";
/// The RunningHeaderType variable's baked fallback, shown until the
/// live demo authors Chapter Title paragraphs for it to pick up.
pub const RUNNING_HEAD_BAKED_TEXT: &str = "Chapter";

/// Which master a page applies (and therefore its margin grid and
/// furniture).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    /// `A-Front` — centred single column, drop folios only.
    Front,
    /// `B-Body` — the 6-column workhorse.
    Body,
    /// `C-Opener` — chapter openers (all twenty land on rectos).
    Opener,
    /// `D-Plate` — zero margins, zero furniture.
    Plate,
    /// `E-Data` — the 12-column fine grid.
    Data,
    /// `F-Vertical` — folio-only vertical-writing exhibit pages.
    Vertical,
    /// `G-Appendix` — the 2-column reference grid.
    Appendix,
}

/// Physical (1-based) page → master role. `Body` is the default for
/// every page not claimed by a list.
fn role_for_page(p: usize) -> Role {
    const PLATES: &[usize] = &[1, 2, 9, 10, 11, 12, 54, 55, 75, 76, 86, 119, 120, 122];
    const OPENERS: &[usize] = &[
        13, 19, 23, 27, 33, 41, 45, 47, 53, 57, 61, 65, 71, 77, 87, 95, 103, 109, 115, 121,
    ];
    if PLATES.contains(&p) {
        Role::Plate
    } else if (3..=8).contains(&p) {
        Role::Front
    } else if OPENERS.contains(&p) {
        Role::Opener
    } else if p == 16
        || (66..=70).contains(&p)
        || (96..=102).contains(&p)
        || (110..=114).contains(&p)
        || (123..=125).contains(&p)
    {
        Role::Data
    } else if p == 43 || p == 44 {
        Role::Vertical
    } else if (127..=134).contains(&p) {
        Role::Appendix
    } else {
        Role::Body
    }
}

/// The 134-page plan: per physical page, its role + the `Page.Name`
/// detail token (openers are numbered in book order).
fn page_plan() -> Vec<(Role, String)> {
    let mut opener_no = 0u32;
    (1..=PAGE_COUNT)
        .map(|p| {
            let role = role_for_page(p);
            let detail = match role {
                Role::Plate if p == 1 => "cover".to_string(),
                Role::Plate => "plate".to_string(),
                Role::Front => "front".to_string(),
                Role::Opener => {
                    opener_no += 1;
                    format!("opener-ch{opener_no:02}")
                }
                Role::Data => "data".to_string(),
                Role::Vertical => "vertical".to_string(),
                Role::Appendix if p == PAGE_COUNT => "colophon".to_string(),
                Role::Appendix => "appendix".to_string(),
                Role::Body if p == OVERRIDE_PAGE_IDX + 1 => "body-override".to_string(),
                Role::Body => "body".to_string(),
            };
            (role, detail)
        })
        .collect()
}

/// 0-based index of the page that overrides `B-Body`'s recto running
/// head — physical p15.
const OVERRIDE_PAGE_IDX: usize = 14;

// ── Resources ────────────────────────────────────────────────────────

/// `Resources/Graphic.xml` — the brand swatches, the vermilion→paper
/// gradient, and the "Annual Brand" colour group.
fn graphic() -> Vec<u8> {
    let colors = [
        RichColor {
            self_id: SWATCH_INK.to_string(),
            name: SWATCH_INK_NAME.to_string(),
            model: "Process",
            // Rich editorial black, not flat K=100.
            space: "CMYK",
            value: "72 62 58 90".to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
            base_color: None,
        },
        RichColor {
            self_id: SWATCH_PAPER_WARM.to_string(),
            name: SWATCH_PAPER_WARM_NAME.to_string(),
            model: "Process",
            space: "CMYK",
            value: "2 3 6 0".to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
            base_color: None,
        },
        RichColor {
            self_id: SWATCH_VERMILION.to_string(),
            name: SWATCH_VERMILION_NAME.to_string(),
            // A real spot ink. The CMYK alternate doubles as the
            // preview build — `effective_cmyk` for a spot resolves
            // through the ALTERNATE, so it must be present.
            model: "Spot",
            space: "CMYK",
            value: VERMILION_CMYK.to_string(),
            alternate_space: Some("CMYK"),
            alternate_value: Some(VERMILION_CMYK.to_string()),
            tint: None,
            base_color: None,
        },
        RichColor {
            self_id: SWATCH_VERMILION_TINT.to_string(),
            name: SWATCH_VERMILION_TINT_NAME.to_string(),
            model: "Process",
            // CMYK, not RGB — swatch-level tints only scale through
            // `ColorEntry::effective_cmyk` (see the module doc).
            space: "CMYK",
            value: VERMILION_CMYK.to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: Some(20.0),
            // Named, so this writes IDML's `<Tint>` element instead of a
            // `TintValue` attribute on `<Color>`. InDesign reads the
            // element and drops the attribute — every panel wearing this
            // swatch printed full-strength red out of a real InDesign
            // re-export until the spelling changed.
            base_color: Some(SWATCH_VERMILION.to_string()),
        },
        RichColor {
            self_id: SWATCH_SLATE.to_string(),
            name: SWATCH_SLATE_NAME.to_string(),
            model: "Process",
            space: "CMYK",
            value: "65 45 30 10".to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
            base_color: None,
        },
        RichColor {
            self_id: SWATCH_LAB_MARIGOLD.to_string(),
            name: SWATCH_LAB_MARIGOLD_NAME.to_string(),
            model: "Process",
            // Device-independent Lab primary — the renderer resolves
            // Lab swatches analytically (D50→D65 Bradford → sRGB).
            space: "LAB",
            value: "78 15 82".to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
            base_color: None,
        },
        RichColor {
            self_id: SWATCH_SCREEN_BLUE.to_string(),
            name: SWATCH_SCREEN_BLUE_NAME.to_string(),
            model: "Process",
            // The deliberate RGB-warning specimen (see module doc).
            space: "RGB",
            value: "47 111 235".to_string(),
            alternate_space: None,
            alternate_value: None,
            tint: None,
            base_color: None,
        },
    ];
    let gradients = [ExtraGradient {
        self_id: GRADIENT_RAMP.to_string(),
        name: GRADIENT_RAMP_NAME.to_string(),
        kind: "Linear",
        stops: vec![
            GradientStop {
                stop_color: SWATCH_VERMILION.to_string(),
                location_pct: 0.0,
            },
            GradientStop {
                stop_color: SWATCH_PAPER_WARM.to_string(),
                location_pct: 100.0,
            },
        ],
    }];
    let groups = [ColorGroupSpec {
        self_id: COLOR_GROUP_BRAND.to_string(),
        name: COLOR_GROUP_BRAND_NAME.to_string(),
        // Screen Blue is deliberately absent — the warning specimen
        // is not brand.
        members: vec![
            SWATCH_INK.to_string(),
            SWATCH_PAPER_WARM.to_string(),
            SWATCH_VERMILION.to_string(),
            SWATCH_VERMILION_TINT.to_string(),
            SWATCH_SLATE.to_string(),
            SWATCH_LAB_MARIGOLD.to_string(),
        ],
    }];
    graphic_xml_rich_full(&colors, &gradients, &groups, &[])
}

/// `Resources/Fonts.xml` — the annual's editorial palette, declared
/// with the EXACT family strings the engine's `RegisterFont` sees
/// (they match `corpus/fonts/` and the editor's showcase driver:
/// "Source Serif 4", "EB Garamond", "Fraunces", "JetBrains Mono",
/// "Space Grotesk", "Noto Sans Arabic", "Noto Sans JP"). The variable
/// fonts carry their weights (SourceSerif4.ttf spans wght 200–900, so
/// `FontStyle="Bold"` on Annual Strong is real).
fn fonts() -> Vec<u8> {
    type FaceList = &'static [(&'static str, &'static str)];
    let families: &[(&str, &str, FaceList)] = &[
        (
            "FontFamily/SourceSerif4",
            "Source Serif 4",
            &[("SourceSerif4", "Regular")],
        ),
        (
            "FontFamily/EBGaramond",
            "EB Garamond",
            &[("EBGaramond", "Regular"), ("EBGaramond-Italic", "Italic")],
        ),
        (
            "FontFamily/Fraunces",
            "Fraunces",
            &[("Fraunces", "Regular"), ("Fraunces-Italic", "Italic")],
        ),
        (
            "FontFamily/JetBrainsMono",
            "JetBrains Mono",
            &[("JetBrainsMono", "Regular")],
        ),
        (
            "FontFamily/SpaceGrotesk",
            "Space Grotesk",
            &[("SpaceGrotesk", "Regular")],
        ),
        (
            "FontFamily/NotoSansArabic",
            "Noto Sans Arabic",
            &[("NotoSansArabic", "Regular")],
        ),
        (
            "FontFamily/NotoSansJP",
            "Noto Sans JP",
            &[("NotoSansJP", "Regular")],
        ),
        (
            "FontFamily/OpenSans",
            "Open Sans",
            &[("OpenSans", "Regular")],
        ),
    ];
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Fonts", &[PKG_NS, DOM_VERSION]);
    for (family_self, family, faces) in families {
        b.start("FontFamily", &[("Self", family_self), ("Name", family)]);
        for (ps_name, style) in *faces {
            b.empty(
                "Font",
                &[
                    ("Self", &format!("Font/{ps_name}")),
                    ("FontFamily", family),
                    ("Name", family),
                    ("PostScriptName", ps_name),
                    ("Status", "Installed"),
                    ("FontStyleName", style),
                    ("FontType", "TrueType"),
                ],
            );
        }
        b.end("FontFamily");
    }
    b.end("idPkg:Fonts");
    b.into_bytes()
}

/// `Resources/Styles.xml` — the whole named cascade, conditions, TOC,
/// object/table/cell styles, spliced as one raw fragment (the parser
/// keys every style element by `Self` regardless of wrapper). Leadings
/// are typed `<Properties>` children; all sit the 13 pt rhythm.
/// Split an `AppliedFont="…"` attribute out of a style's attribute
/// string.
///
/// IDML spells the applied font as a typed CHILD of `<Properties>`,
/// never as an attribute — a real InDesign-authored file carries
/// `<AppliedFont type="string">Titillium</AppliedFont>` and not one
/// attribute of that name. Written as an attribute it is silently
/// ignored, which is how this book, set in twenty faces, opened in
/// InDesign entirely in Minion Pro. Splitting it here lets every style
/// below keep writing `AppliedFont="…"` in its attribute string while
/// the FILE carries the spelling Adobe reads.
fn split_applied_font(attrs: &str) -> (String, Option<String>) {
    let Some(at) = attrs.find("AppliedFont=\"") else {
        return (attrs.to_string(), None);
    };
    let value_start = at + "AppliedFont=\"".len();
    let Some(rel_end) = attrs[value_start..].find('"') else {
        return (attrs.to_string(), None);
    };
    let value_end = value_start + rel_end;
    let font = attrs[value_start..value_end].to_string();
    let mut rest = String::with_capacity(attrs.len());
    rest.push_str(attrs[..at].trim_end());
    let tail = attrs[value_end + 1..].trim_start();
    if !rest.is_empty() && !tail.is_empty() {
        rest.push(' ');
    }
    rest.push_str(tail);
    (rest, Some(font))
}

/// `<AppliedFont type="string">…` for a `<Properties>` block, or
/// nothing when the style pins no font.
fn applied_font_property(font: Option<&str>) -> String {
    match font {
        Some(f) => format!("<AppliedFont type=\"string\">{f}</AppliedFont>"),
        None => String::new(),
    }
}

fn styles() -> Vec<u8> {
    let mut f = String::new();

    // Character styles first (paragraph styles reference none of them,
    // but Catalog Entry's nested style names Specimen Number).
    f.push_str(&format!(
        "<RootCharacterStyleGroup>\
<CharacterStyle Self=\"{CHAR_EMPHASIS}\" Name=\"Annual Emphasis\" \
FontStyle=\"Italic\"><Properties><AppliedFont type=\"string\">EB Garamond</AppliedFont></Properties></CharacterStyle>\
<CharacterStyle Self=\"{CHAR_STRONG}\" Name=\"Annual Strong\" \
FontStyle=\"Bold\"><Properties><AppliedFont type=\"string\">Source Serif 4</AppliedFont></Properties></CharacterStyle>\
<CharacterStyle Self=\"{CHAR_SMALL_CAPS}\" Name=\"Small Caps\" \
Capitalization=\"SmallCaps\" Tracking=\"20\"><Properties><AppliedFont type=\"string\">EB Garamond</AppliedFont></Properties></CharacterStyle>\
<CharacterStyle Self=\"{CHAR_CODE_INLINE}\" Name=\"Code Inline\"><Properties><AppliedFont type=\"string\">JetBrains Mono</AppliedFont></Properties></CharacterStyle>\
<CharacterStyle Self=\"{CHAR_LEAD_IN}\" Name=\"Lead-In\" \
Capitalization=\"SmallCaps\" Tracking=\"40\"/>\
<CharacterStyle Self=\"{CHAR_SUPERIOR}\" Name=\"Superior\" \
Position=\"Superscript\"/>\
<CharacterStyle Self=\"{CHAR_URL}\" Name=\"URL\" \
FillColor=\"{SWATCH_SCREEN_BLUE}\" Underline=\"true\"/>\
<CharacterStyle Self=\"{CHAR_ACCENT_INK}\" Name=\"Accent Ink\" \
FillColor=\"{SWATCH_VERMILION}\"/>\
<CharacterStyle Self=\"{CHAR_SPECIMEN_NUMBER}\" Name=\"Specimen Number\" \
FillColor=\"{SWATCH_SLATE}\" Tracking=\"20\"><Properties><AppliedFont type=\"string\">JetBrains Mono</AppliedFont></Properties></CharacterStyle>\
</RootCharacterStyleGroup>"
    ));

    // Paragraph styles. Convention: leading as <Properties><Leading>,
    // BasedOn cascading from Annual Body.
    f.push_str("<RootParagraphStyleGroup>");
    let para =
        |f: &mut String, self_id: &str, name: &str, attrs: &str, leading: f32, child: &str| {
            let (attrs, font) = split_applied_font(attrs);
            let font_prop = applied_font_property(font.as_deref());
            f.push_str(&format!(
                "<ParagraphStyle Self=\"{self_id}\" Name=\"{name}\" {attrs}>\
<Properties>{font_prop}<Leading type=\"unit\">{leading}</Leading></Properties>{child}\
</ParagraphStyle>"
            ));
        };
    para(
        &mut f,
        STYLE_BODY,
        "Annual Body",
        &format!(
            "AppliedFont=\"Source Serif 4\" PointSize=\"9.5\" FillColor=\"{SWATCH_INK}\" \
Justification=\"LeftJustified\" Hyphenation=\"true\" HyphenationZone=\"36\" \
FirstLineIndent=\"13\" OTFFigureStyle=\"ProportionalOldStyle\" NextStyle=\"{STYLE_BODY}\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_BODY_FIRST,
        "Body First",
        &format!("BasedOn=\"{STYLE_BODY}\" FirstLineIndent=\"0\" NextStyle=\"{STYLE_BODY}\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_BODY_SMALL,
        "Body Small",
        &format!("BasedOn=\"{STYLE_BODY}\" PointSize=\"8\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_FOOTNOTE,
        "Footnote",
        &format!("BasedOn=\"{STYLE_BODY}\" PointSize=\"7.5\" FirstLineIndent=\"0\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_CAPTION,
        "Caption",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"7\" FillColor=\"{SWATCH_INK}\" \
Tracking=\"40\" SpaceBefore=\"6.5\" SpaceAfter=\"6.5\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_MARGIN_NOTE,
        "Margin Note",
        &format!(
            "AppliedFont=\"EB Garamond\" FontStyle=\"Italic\" PointSize=\"7.5\" \
FillColor=\"{SWATCH_SLATE}\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_CODE_BLOCK,
        "Code Block",
        &format!(
            "AppliedFont=\"JetBrains Mono\" PointSize=\"8.5\" FillColor=\"{SWATCH_INK}\" \
Hyphenation=\"false\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_BULLET_LIST,
        "Bullet List",
        &format!(
            "BasedOn=\"{STYLE_BODY}\" LeftIndent=\"13\" FirstLineIndent=\"-13\" \
BulletsAndNumberingListType=\"BulletList\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_NUMBERED_1,
        "Numbered 1",
        &format!(
            "BasedOn=\"{STYLE_BODY}\" LeftIndent=\"13\" FirstLineIndent=\"-13\" \
BulletsAndNumberingListType=\"NumberedList\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_NUMBERED_2,
        "Numbered 2",
        &format!("BasedOn=\"{STYLE_NUMBERED_1}\" LeftIndent=\"26\""),
        13.0,
        "",
    );
    // Catalog Entry carries a real <NestedStyle> (the parser reads
    // flat NestedStyle children of ParagraphStyle — same shape the
    // typed ParagraphStyleSpec builder emits).
    para(
        &mut f,
        STYLE_CATALOG_ENTRY,
        "Catalog Entry",
        &format!("BasedOn=\"{STYLE_BODY}\" FirstLineIndent=\"0\""),
        13.0,
        &format!(
            "<NestedStyle AppliedCharacterStyle=\"{CHAR_SPECIMEN_NUMBER}\" \
Delimiter=\".\" Repetition=\"1\" Inclusive=\"true\"/>"
        ),
    );
    para(
        &mut f,
        STYLE_TABLE_HEAD,
        "Table Head",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"7.5\" FillColor=\"{SWATCH_INK}\" \
Capitalization=\"AllCaps\" Tracking=\"80\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_TABLE_BODY,
        "Table Body",
        &format!("AppliedFont=\"Source Serif 4\" PointSize=\"8\" FillColor=\"{SWATCH_INK}\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_TABLE_NUMBER,
        "Table Number",
        &format!(
            "AppliedFont=\"Source Serif 4\" PointSize=\"8\" FillColor=\"{SWATCH_INK}\" \
Justification=\"RightAlign\" OTFFigureStyle=\"TabularLining\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_TOC_PART,
        "TOC Part",
        &format!(
            "AppliedFont=\"Fraunces\" PointSize=\"14\" FillColor=\"{SWATCH_INK}\" \
SpaceBefore=\"13\""
        ),
        26.0,
        "",
    );
    para(
        &mut f,
        STYLE_TOC_CHAPTER,
        "TOC Chapter",
        &format!("AppliedFont=\"Source Serif 4\" PointSize=\"9.5\" FillColor=\"{SWATCH_INK}\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_TOC_HEAD,
        "TOC Head",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"8\" FillColor=\"{SWATCH_SLATE}\" \
LeftIndent=\"13\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_INDEX_ENTRY,
        "Index Entry",
        &format!("AppliedFont=\"Source Serif 4\" PointSize=\"8\" FillColor=\"{SWATCH_INK}\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_INDEX_SUB,
        "Index Sub",
        &format!("BasedOn=\"{STYLE_INDEX_ENTRY}\" LeftIndent=\"13\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_SPEC_LABEL,
        "Spec Label",
        &format!(
            "AppliedFont=\"JetBrains Mono\" PointSize=\"6.5\" FillColor=\"{SWATCH_SLATE}\" \
Capitalization=\"AllCaps\" Tracking=\"60\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_SPEC_VALUE,
        "Spec Value",
        &format!("AppliedFont=\"Source Serif 4\" PointSize=\"8.5\" FillColor=\"{SWATCH_INK}\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_CHAPTER_NUMBER,
        "Chapter Number",
        &format!(
            "AppliedFont=\"Fraunces\" PointSize=\"64\" FillColor=\"{SWATCH_VERMILION}\" \
SpaceAfter=\"13\""
        ),
        65.0,
        "",
    );
    para(
        &mut f,
        STYLE_CHAPTER_TITLE,
        "Chapter Title",
        &format!(
            "AppliedFont=\"Fraunces\" PointSize=\"30\" FillColor=\"{SWATCH_INK}\" \
SpaceAfter=\"13\" NextStyle=\"{STYLE_DECK}\""
        ),
        39.0,
        "",
    );
    para(
        &mut f,
        STYLE_DECK,
        "Deck",
        &format!(
            "AppliedFont=\"EB Garamond\" FontStyle=\"Italic\" PointSize=\"14\" \
FillColor=\"{SWATCH_SLATE}\" SpaceAfter=\"26\" NextStyle=\"{STYLE_BODY_FIRST}\""
        ),
        26.0,
        "",
    );
    // KeepWithNext is carried for InDesign round-trip; the engine's
    // ParagraphStyleDef doesn't model it (see module doc).
    para(
        &mut f,
        STYLE_HEAD_1,
        "Head 1",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"15\" FillColor=\"{SWATCH_INK}\" \
SpaceBefore=\"13\" SpaceAfter=\"13\" KeepWithNext=\"1\" NextStyle=\"{STYLE_BODY_FIRST}\""
        ),
        26.0,
        "",
    );
    para(
        &mut f,
        STYLE_HEAD_2,
        "Head 2",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"9.5\" FillColor=\"{SWATCH_INK}\" \
Capitalization=\"AllCaps\" Tracking=\"100\" SpaceBefore=\"13\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_PULL_QUOTE,
        "Pull Quote",
        &format!(
            "AppliedFont=\"Fraunces\" FontStyle=\"Italic\" PointSize=\"18\" \
FillColor=\"{SWATCH_VERMILION}\" LeftIndent=\"13\" RightIndent=\"13\" \
SpaceBefore=\"13\" SpaceAfter=\"13\""
        ),
        26.0,
        "",
    );
    para(
        &mut f,
        STYLE_PART_TITLE,
        "Part Title",
        &format!(
            "AppliedFont=\"Fraunces\" PointSize=\"44\" FillColor=\"{SWATCH_INK}\" \
SpaceAfter=\"26\""
        ),
        52.0,
        "",
    );
    para(
        &mut f,
        STYLE_FOLIO,
        "Folio",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"8\" FillColor=\"{SWATCH_INK}\" \
OTFFigureStyle=\"TabularLining\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_RUNNING_HEAD,
        "Running Head",
        &format!(
            "AppliedFont=\"Space Grotesk\" PointSize=\"7\" FillColor=\"{SWATCH_INK}\" \
Capitalization=\"AllCaps\" Tracking=\"100\""
        ),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_COLOPHON,
        "Colophon",
        &format!("AppliedFont=\"EB Garamond\" PointSize=\"8.5\" FillColor=\"{SWATCH_INK}\""),
        13.0,
        "",
    );
    para(
        &mut f,
        STYLE_SPECIMEN_NO,
        "Specimen No",
        &format!("AppliedFont=\"JetBrains Mono\" PointSize=\"7\" FillColor=\"{SWATCH_SLATE}\""),
        13.0,
        "",
    );
    f.push_str("</RootParagraphStyleGroup>");

    // Object styles ("Spec Panel"'s text inset is not modelled by
    // ObjectStyleDef — fill + rounded corner only, see module doc).
    f.push_str(&format!(
        "<RootObjectStyleGroup>\
<ObjectStyle Self=\"{OBJECT_PLATE_FRAME}\" Name=\"Plate Frame\" \
StrokeColor=\"{SWATCH_INK}\" StrokeWeight=\"0.5\"/>\
<ObjectStyle Self=\"{OBJECT_SPEC_PANEL}\" Name=\"Spec Panel\" \
FillColor=\"{SWATCH_PAPER_WARM}\" CornerOption=\"RoundedCorner\" CornerRadius=\"4\"/>\
<ObjectStyle Self=\"{OBJECT_ANNOTATION_MARKER}\" Name=\"Annotation Marker\" \
FillColor=\"{SWATCH_VERMILION_TINT}\" StrokeColor=\"{SWATCH_VERMILION}\" \
StrokeWeight=\"0.5\"/>\
</RootObjectStyleGroup>"
    ));

    // Table + cell styles.
    f.push_str(&format!(
        "<RootCellStyleGroup>\
<CellStyle Self=\"{CELL_TH}\" Name=\"Annual TH\" FillColor=\"{SWATCH_PAPER_WARM}\" \
VerticalJustification=\"CenterAlign\" \
BottomEdgeStrokeColor=\"{SWATCH_INK}\" BottomEdgeStrokeWeight=\"1\"/>\
<CellStyle Self=\"{CELL_TD}\" Name=\"Annual TD\" \
BottomEdgeStrokeColor=\"{SWATCH_SLATE}\" BottomEdgeStrokeWeight=\"0.25\"/>\
<CellStyle Self=\"{CELL_TD_NUMBER}\" Name=\"Annual TD Number\" BasedOn=\"{CELL_TD}\"/>\
</RootCellStyleGroup>\
<RootTableStyleGroup>\
<TableStyle Self=\"{TABLE_STYLE_ANNUAL}\" Name=\"Annual Table\" \
HeaderRegionCellStyle=\"{CELL_TH}\" BodyRegionCellStyle=\"{CELL_TD}\" \
AlternatingFills=\"AlternatingRows\" \
StartRowFillColor=\"{SWATCH_PAPER_WARM}\" StartRowFillCount=\"1\" \
EndRowFillColor=\"Color/Paper\" EndRowFillCount=\"1\"/>\
</RootTableStyleGroup>"
    ));

    // Conditions + the two sets. ◪ Per-set visibility STATES are not
    // expressible (ConditionSetDef = membership only): "Press" lists
    // its ON condition, "Working Copy" all three — see module doc.
    f.push_str(&format!(
        "<RootConditionalTextGroup>\
<Condition Self=\"{CONDITION_PRINT_ONLY}\" Name=\"Print-only\" Visible=\"true\" \
IndicatorMethod=\"UseUnderline\" IndicatorColor=\"Green\"/>\
<Condition Self=\"{CONDITION_SCREEN_ONLY}\" Name=\"Screen-only\" Visible=\"true\" \
IndicatorMethod=\"UseHighlight\" IndicatorColor=\"Cyan\"/>\
<Condition Self=\"{CONDITION_SPEC_NOTES}\" Name=\"Spec-Notes\" Visible=\"true\" \
IndicatorMethod=\"UseHighlight\" IndicatorColor=\"Magenta\"/>\
<ConditionSet Self=\"{CONDITION_SET_PRESS}\" Name=\"Press\" \
Conditions=\"{CONDITION_PRINT_ONLY}\"/>\
<ConditionSet Self=\"{CONDITION_SET_WORKING_COPY}\" Name=\"Working Copy\" \
Conditions=\"{CONDITION_PRINT_ONLY} {CONDITION_SCREEN_ONLY} {CONDITION_SPEC_NOTES}\"/>\
</RootConditionalTextGroup>"
    ));

    // TOC: three levels, page numbers on, tab separator.
    f.push_str(&format!(
        "<RootTOCStyleGroup>\
<TOCStyle Self=\"{TOC_STYLE}\" Name=\"Annual Contents\" Title=\"Contents\" \
TitleStyle=\"{STYLE_PART_TITLE}\">\
<TOCStyleEntry Name=\"Part Title\" IncludeStyle=\"{STYLE_PART_TITLE}\" \
FormatStyle=\"{STYLE_TOC_PART}\" Level=\"1\" PageNumber=\"On\" Separator=\"^t\"/>\
<TOCStyleEntry Name=\"Chapter Title\" IncludeStyle=\"{STYLE_CHAPTER_TITLE}\" \
FormatStyle=\"{STYLE_TOC_CHAPTER}\" Level=\"2\" PageNumber=\"On\" Separator=\"^t\"/>\
<TOCStyleEntry Name=\"Head 1\" IncludeStyle=\"{STYLE_HEAD_1}\" \
FormatStyle=\"{STYLE_TOC_HEAD}\" Level=\"3\" PageNumber=\"On\" Separator=\"^t\"/>\
</TOCStyle>\
</RootTOCStyleGroup>"
    ));

    styles_xml_with_raw(&f)
}

// ── Stories ──────────────────────────────────────────────────────────

/// One segment of a furniture line.
enum Seg {
    Text(&'static str),
    /// `<?ACE 18?>` — the auto-current-page-number marker.
    PageNumber,
    /// `<TextVariableInstance>` — B-Body's recto running head.
    Variable {
        result_text: &'static str,
        associated: &'static str,
    },
}

/// A one-paragraph story for a piece of master (or override)
/// furniture. Each segment gets its own `CharacterStyleRange` so the
/// variable instance sits at a clean run boundary (how InDesign
/// serialises them).
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
    for seg in segments {
        b.start(
            "CharacterStyleRange",
            &[("AppliedCharacterStyle", NO_CHAR_STYLE)],
        );
        match seg {
            Seg::Text(t) => {
                b.start("Content", &[]);
                b.text(t);
                b.end("Content");
            }
            Seg::PageNumber => {
                b.start("Content", &[]);
                b.write_pi("ACE", "18");
                b.end("Content");
            }
            Seg::Variable {
                result_text,
                associated,
            } => {
                b.empty(
                    "TextVariableInstance",
                    &[
                        ("ResultText", result_text),
                        ("AssociatedTextVariable", associated),
                    ],
                );
            }
        }
        b.end("CharacterStyleRange");
    }
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// One segment of an exhibit paragraph (the marker-bearing fixture
/// content on p45/p46 and the index tabs).
enum XSeg {
    Text(&'static str),
    /// `<HyperlinkTextSource>` wrapping the linked run.
    Link {
        source_self: String,
        text: &'static str,
    },
    /// `<CrossReferenceSource>` wrapping the source run.
    Xref {
        source_self: String,
        text: &'static str,
    },
    /// `<PageReference>` index marker.
    Index {
        applied_topic: &'static str,
        topic_name: &'static str,
    },
}

/// A one-paragraph exhibit story carrying `segs`.
fn write_exhibit_story(story_id: &str, paragraph_style: &str, segs: &[XSeg]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", paragraph_style)],
    );
    let char_style = ("AppliedCharacterStyle", NO_CHAR_STYLE);
    for seg in segs {
        match seg {
            XSeg::Text(t) => {
                b.start("CharacterStyleRange", &[char_style]);
                b.start("Content", &[]);
                b.text(t);
                b.end("Content");
                b.end("CharacterStyleRange");
            }
            XSeg::Link { source_self, text } => {
                b.start(
                    "HyperlinkTextSource",
                    &[
                        ("Self", source_self.as_str()),
                        ("Name", source_self.as_str()),
                        ("Hidden", "false"),
                        char_style,
                    ],
                );
                b.start("CharacterStyleRange", &[char_style]);
                b.start("Content", &[]);
                b.text(text);
                b.end("Content");
                b.end("CharacterStyleRange");
                b.end("HyperlinkTextSource");
            }
            XSeg::Xref { source_self, text } => {
                b.start(
                    "CrossReferenceSource",
                    &[
                        ("Self", source_self.as_str()),
                        ("Name", source_self.as_str()),
                        char_style,
                    ],
                );
                b.start("CharacterStyleRange", &[char_style]);
                b.start("Content", &[]);
                b.text(text);
                b.end("Content");
                b.end("CharacterStyleRange");
                b.end("CrossReferenceSource");
            }
            XSeg::Index {
                applied_topic,
                topic_name,
            } => {
                b.start("CharacterStyleRange", &[char_style]);
                b.empty(
                    "PageReference",
                    &[("AppliedTopic", applied_topic), ("TopicName", topic_name)],
                );
                b.end("CharacterStyleRange");
            }
        }
    }
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// A body paragraph whose run anchors real `<Footnote>`s — the ONE
/// construct no wire op can author (there is no insert-footnote
/// mutation), so the story chapter's footnote page reads its exhibit
/// from the fixture. Bodies are set in the Footnote style; the
/// document `<FootnoteOption>` supplies the rule.
fn write_footnote_exhibit_story(story_id: &str, body: &str, notes: &[(&str, &str)]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", STYLE_BODY)],
    );
    b.start(
        "CharacterStyleRange",
        &[("AppliedCharacterStyle", NO_CHAR_STYLE)],
    );
    b.start("Content", &[]);
    b.text(body);
    b.end("Content");
    b.end("CharacterStyleRange");
    // Anchors ride at the end of the run, footnotes.rs-style; the
    // renderer pools them onto the host page in order.
    for (id, note) in notes {
        b.start(
            "CharacterStyleRange",
            &[("AppliedCharacterStyle", NO_CHAR_STYLE)],
        );
        b.start("Footnote", &[("Self", id), ("Hidden", "false")]);
        b.start(
            "ParagraphStyleRange",
            &[("AppliedParagraphStyle", STYLE_FOOTNOTE)],
        );
        b.start(
            "CharacterStyleRange",
            &[("AppliedCharacterStyle", NO_CHAR_STYLE)],
        );
        b.start("Content", &[]);
        b.text(note);
        b.end("Content");
        b.end("CharacterStyleRange");
        b.end("ParagraphStyleRange");
        b.end("Footnote");
        b.end("CharacterStyleRange");
    }
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// A vertical-writing exhibit: `StoryDirection="VerticalWritingDirection"`
/// with real Japanese set in Noto Sans JP — story direction is content
/// state no mutation writes, so the scripts chapter's vertical pages
/// read from here. One run carries GroupRuby, one a kenten mark (both
/// render at their recorded MVP limits — the chapter's margin notes say
/// so).
fn write_vertical_exhibit_story(story_id: &str, lead: &str, ruby_base: &str, ruby: &str, tail: &str) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start(
        "Story",
        &[
            ("Self", story_id),
            ("StoryDirection", "VerticalWritingDirection"),
        ],
    );
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", STYLE_BODY)],
    );
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("PointSize", "13"),
        ],
    );
    b.start("Properties", &[]);
    b.start("AppliedFont", &[("type", "string")]);
    b.text("Noto Sans JP");
    b.end("AppliedFont");
    b.end("Properties");
    b.start("Content", &[]);
    b.text(lead);
    b.end("Content");
    b.end("CharacterStyleRange");
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("PointSize", "13"),
            ("Ruby", "true"),
            ("RubyString", ruby),
            ("RubyType", "GroupRuby"),
        ],
    );
    b.start("Properties", &[]);
    b.start("AppliedFont", &[("type", "string")]);
    b.text("Noto Sans JP");
    b.end("AppliedFont");
    b.end("Properties");
    b.start("Content", &[]);
    b.text(ruby_base);
    b.end("Content");
    b.end("CharacterStyleRange");
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("PointSize", "13"),
            ("KentenKind", "KentenSesameDot"),
        ],
    );
    b.start("Properties", &[]);
    b.start("AppliedFont", &[("type", "string")]);
    b.text("Noto Sans JP");
    b.end("AppliedFont");
    b.end("Properties");
    b.start("Content", &[]);
    b.text(tail);
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

// ── Page items ───────────────────────────────────────────────────────

/// A furniture / exhibit text frame in SPREAD coords: no fill, no
/// stroke, hosting `story_id`.
fn text_frame(
    self_id: String,
    x_spread: f32,
    y_pt: f32,
    w_pt: f32,
    h_pt: f32,
    story: &str,
) -> Rect {
    Rect {
        self_id,
        width_pt: w_pt,
        height_pt: h_pt,
        item_transform: translate(x_spread, y_pt),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story.to_string()),
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

/// Per-side margins for a role. `verso` mirrors inside/outside.
fn margins_for(role: Role, verso: bool) -> MarginPreference {
    let (left, right) = if verso {
        (MARGIN_OUTSIDE_PT, MARGIN_INSIDE_PT)
    } else {
        (MARGIN_INSIDE_PT, MARGIN_OUTSIDE_PT)
    };
    let mirrored = |column_count: u32| MarginPreference {
        top: MARGIN_TOP_PT,
        bottom: MARGIN_BOTTOM_PT,
        left,
        right,
        column_count,
        column_gutter: COLUMN_GUTTER_PT,
    };
    match role {
        Role::Plate => MarginPreference::symmetric(0.0, 0.0, 0.0, 0.0),
        Role::Front => MarginPreference {
            top: MARGIN_TOP_PT,
            bottom: MARGIN_BOTTOM_PT,
            left: FRONT_MARGIN_PT,
            right: FRONT_MARGIN_PT,
            column_count: 1,
            column_gutter: 0.0,
        },
        Role::Body | Role::Opener => mirrored(6),
        Role::Data => mirrored(12),
        Role::Vertical => MarginPreference {
            column_gutter: 0.0,
            ..mirrored(1)
        },
        Role::Appendix => mirrored(2),
    }
}

// ── Build ────────────────────────────────────────────────────────────

pub fn build() -> Sample {
    // Spread-coord x of each side's live-area left edge.
    let verso_x = MARGIN_OUTSIDE_PT - PAGE_W_PT; // −480
    let recto_x = MARGIN_INSIDE_PT; // 48
    let front_verso_x = FRONT_MARGIN_PT - PAGE_W_PT;
    let front_recto_x = FRONT_MARGIN_PT;

    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut story_seq = 0u32;
    let mut frame_seq = 0u32;

    let add_line_story = |stories: &mut Vec<(String, Vec<u8>)>,
                          seq: &mut u32,
                          style: &str,
                          justification: &str,
                          segs: &[Seg]| {
        let id = self_id(SAMPLE, "Story", *seq);
        *seq += 1;
        stories.push((
            id.clone(),
            write_line_story(&id, style, justification, segs),
        ));
        id
    };
    let frame_id = |seq: &mut u32| {
        let id = self_id(SAMPLE, "MasterFrame", *seq);
        *seq += 1;
        id
    };

    // ── Masters: A..G, all facing. Every furniture frame has its OWN
    // story (the master-text pass skips a master frame whose story
    // already appears on a body frame anywhere in the document, so
    // shared furniture stories would suppress each other).
    let master_ids: Vec<String> = (0..7).map(|i| self_id(SAMPLE, "MasterSpread", i)).collect();
    let master_page_ids: Vec<String> = (0..14).map(|i| self_id(SAMPLE, "MasterPage", i)).collect();
    let master =
        |idx: usize, prefix: &str, base: &str, items: Vec<PageItem>| -> (String, Vec<u8>) {
            (
                master_ids[idx].clone(),
                write_facing_master(&FacingMaster {
                    self_id: format!("MasterSpread/{}", master_ids[idx]),
                    name_prefix: prefix.to_string(),
                    base_name: base.to_string(),
                    verso_page_self_id: master_page_ids[2 * idx].clone(),
                    recto_page_self_id: master_page_ids[2 * idx + 1].clone(),
                    page_width_pt: PAGE_W_PT,
                    page_height_pt: PAGE_H_PT,
                    page_items: items,
                }),
            )
        };

    // A-Front: centred drop folios only.
    let a_folio_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "CenterAlign",
        &[Seg::PageNumber],
    );
    let a_folio_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "CenterAlign",
        &[Seg::PageNumber],
    );
    let front_master = master(
        0,
        "A",
        "Front",
        vec![
            text_frame(
                frame_id(&mut frame_seq),
                front_verso_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &a_folio_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                front_recto_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &a_folio_r,
            )
            .into(),
        ],
    );

    // B-Body: verso title head + recto RunningHeader VARIABLE head,
    // folios at the outside edges. The recto head frame is the p15
    // override target, so keep its id.
    let b_head_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_RUNNING_HEAD,
        "LeftAlign",
        &[Seg::Text(RUNNING_HEAD_VERSO_TEXT)],
    );
    let b_head_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_RUNNING_HEAD,
        "RightAlign",
        &[Seg::Variable {
            result_text: RUNNING_HEAD_BAKED_TEXT,
            associated: VAR_RUNNING_HEADER,
        }],
    );
    let b_folio_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "LeftAlign",
        &[Seg::PageNumber],
    );
    let b_folio_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "RightAlign",
        &[Seg::PageNumber],
    );
    let body_recto_head_frame_id = frame_id(&mut frame_seq);
    let body_master = master(
        1,
        "B",
        "Body",
        vec![
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &b_head_v,
            )
            .into(),
            text_frame(
                body_recto_head_frame_id.clone(),
                recto_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &b_head_r,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &b_folio_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &b_folio_r,
            )
            .into(),
        ],
    );

    // C-Opener: drop folios + a recto slug band (all openers land on
    // rectos), no running head.
    let c_folio_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "LeftAlign",
        &[Seg::PageNumber],
    );
    let c_folio_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "RightAlign",
        &[Seg::PageNumber],
    );
    let c_slug_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_SPEC_LABEL,
        "RightAlign",
        &[Seg::Text("MMXXVI · A SPECIMEN ANNUAL")],
    );
    let opener_master = master(
        2,
        "C",
        "Opener",
        vec![
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &c_folio_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &c_folio_r,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &c_slug_r,
            )
            .into(),
        ],
    );

    // D-Plate: facing, EMPTY.
    let plate_master = master(3, "D", "Plate", Vec::new());

    // E-Data: B-style furniture with its own recto head.
    let e_head_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_RUNNING_HEAD,
        "LeftAlign",
        &[Seg::Text(RUNNING_HEAD_VERSO_TEXT)],
    );
    let e_head_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_RUNNING_HEAD,
        "RightAlign",
        &[Seg::Text("DATA TABLES · MMXXVI")],
    );
    let e_folio_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "LeftAlign",
        &[Seg::PageNumber],
    );
    let e_folio_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "RightAlign",
        &[Seg::PageNumber],
    );
    let data_master = master(
        4,
        "E",
        "Data",
        vec![
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &e_head_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &e_head_r,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &e_folio_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &e_folio_r,
            )
            .into(),
        ],
    );

    // F-Vertical: folios only.
    let f_folio_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "LeftAlign",
        &[Seg::PageNumber],
    );
    let f_folio_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "RightAlign",
        &[Seg::PageNumber],
    );
    let vertical_master = master(
        5,
        "F",
        "Vertical",
        vec![
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &f_folio_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &f_folio_r,
            )
            .into(),
        ],
    );

    // G-Appendix: small heads + folios.
    let g_head_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_SPEC_LABEL,
        "LeftAlign",
        &[Seg::Text("APPARATUS")],
    );
    let g_head_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_SPEC_LABEL,
        "RightAlign",
        &[Seg::Text("INDEX & COLOPHON")],
    );
    let g_folio_v = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "LeftAlign",
        &[Seg::PageNumber],
    );
    let g_folio_r = add_line_story(
        &mut stories,
        &mut story_seq,
        STYLE_FOLIO,
        "RightAlign",
        &[Seg::PageNumber],
    );
    let appendix_master = master(
        6,
        "G",
        "Appendix",
        vec![
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &g_head_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                HEAD_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &g_head_r,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                verso_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &g_folio_v,
            )
            .into(),
            text_frame(
                frame_id(&mut frame_seq),
                recto_x,
                FOLIO_Y_PT,
                LIVE_W_PT,
                FURNITURE_H_PT,
                &g_folio_r,
            )
            .into(),
        ],
    );

    let role_master = |role: Role| -> &String {
        match role {
            Role::Front => &master_ids[0],
            Role::Body => &master_ids[1],
            Role::Opener => &master_ids[2],
            Role::Plate => &master_ids[3],
            Role::Data => &master_ids[4],
            Role::Vertical => &master_ids[5],
            Role::Appendix => &master_ids[6],
        }
    };

    // ── The p15 override: its own replacement recto running head.
    let override_story = self_id(SAMPLE, "Story", story_seq);
    story_seq += 1;
    stories.push((
        override_story.clone(),
        write_line_story(
            &override_story,
            STYLE_RUNNING_HEAD,
            "RightAlign",
            &[Seg::Text(OVERRIDE_HEAD_TEXT)],
        ),
    ));
    let override_frame_id = self_id(SAMPLE, "OverrideHeader", 0);

    // ── Navigation resource ids.
    let page_ids: Vec<String> = (0..PAGE_COUNT)
        .map(|i| self_id(SAMPLE, "Page", i as u32))
        .collect();
    let url_source = format!("HyperlinkTextSource/{}", self_id(SAMPLE, "HLSource", 0));
    let page_source = format!("HyperlinkTextSource/{}", self_id(SAMPLE, "HLSource", 1));
    let xref_source = format!("CrossReferenceSource/{}", self_id(SAMPLE, "Xref", 0));
    let url_dest = format!(
        "HyperlinkURLDestination/{}",
        self_id(SAMPLE, "HLUrlDest", 0)
    );
    let page_dest_ch01 = format!(
        "HyperlinkPageDestination/{}",
        self_id(SAMPLE, "HLPageDest", 0)
    );
    let page_dest_data = format!(
        "HyperlinkPageDestination/{}",
        self_id(SAMPLE, "HLPageDest", 1)
    );
    let text_dest_45 = format!(
        "HyperlinkTextDestination/{}",
        self_id(SAMPLE, "HLTextDest", 0)
    );
    let text_dest_46 = format!(
        "HyperlinkTextDestination/{}",
        self_id(SAMPLE, "HLTextDest", 1)
    );

    // ── Exhibit stories (p45/p46, the apparatus chapter): the
    // cross-reference pair, hyperlink sources, index markers.
    let exhibit_45_story = self_id(SAMPLE, "Story", story_seq);
    story_seq += 1;
    stories.push((
        exhibit_45_story.clone(),
        write_exhibit_story(
            &exhibit_45_story,
            STYLE_BODY_FIRST,
            &[
                XSeg::Text("The apparatus of the annual begins here. "),
                XSeg::Xref {
                    source_self: xref_source.clone(),
                    text: "See the reconsidered specimens",
                },
                XSeg::Text(" overleaf, or visit "),
                XSeg::Link {
                    source_self: url_source.clone(),
                    text: "paged.media",
                },
                XSeg::Text(" for the living registry."),
                XSeg::Index {
                    applied_topic: "Topic/AnnualTypography",
                    topic_name: "Typography",
                },
                XSeg::Index {
                    applied_topic: "Topic/AnnualBaskerville",
                    topic_name: "Baskerville",
                },
            ],
        ),
    ));
    let exhibit_46_story = self_id(SAMPLE, "Story", story_seq);
    story_seq += 1;
    stories.push((
        exhibit_46_story.clone(),
        write_exhibit_story(
            &exhibit_46_story,
            STYLE_BODY_FIRST,
            &[
                XSeg::Text("Specimen notes, reconsidered. Jump back to "),
                XSeg::Link {
                    source_self: page_source.clone(),
                    text: "the first chapter",
                },
                XSeg::Text(" to compare settings."),
                XSeg::Index {
                    applied_topic: "Topic/AnnualColophon",
                    topic_name: "Colophon",
                },
            ],
        ),
    ));

    // ── The six outside-margin index tabs on verso pages: each story
    // carries 1–2 `<PageReference>` markers so the appendix index
    // resolves entries from across the book. `(physical page, label,
    // [(topic id, topic name)])`.
    type TabTopics = &'static [(&'static str, &'static str)];
    let tabs: [(usize, &'static str, TabTopics); 6] = [
        (
            20,
            "a",
            &[
                ("Topic/AnnualTypography", "Typography"),
                ("Topic/AnnualGrids", "Grids"),
            ],
        ),
        (34, "b", &[("Topic/AnnualBaskerville", "Baskerville")]),
        (
            48,
            "c",
            &[
                ("Topic/AnnualSpotColour", "Spot colour"),
                ("Topic/AnnualLabColour", "Lab colour"),
            ],
        ),
        (66, "d", &[("Topic/AnnualDataTables", "Data tables")]),
        (
            78,
            "e",
            &[
                ("Topic/AnnualFootnotes", "Footnotes"),
                ("Topic/AnnualVerticalWriting", "Vertical writing"),
            ],
        ),
        (96, "f", &[("Topic/AnnualAppendices", "Appendices")]),
    ];

    // Fixture-authored page items per 0-based page index.
    let mut extra_items: HashMap<usize, Vec<PageItem>> = HashMap::new();
    // p15 override header (recto side of its spread).
    extra_items.entry(OVERRIDE_PAGE_IDX).or_default().push(
        text_frame(
            override_frame_id.clone(),
            recto_x,
            HEAD_Y_PT,
            LIVE_W_PT,
            FURNITURE_H_PT,
            &override_story,
        )
        .into(),
    );
    // p45 (recto) + p46 (verso) exhibits.
    extra_items.entry(44).or_default().push(
        text_frame(
            self_id(SAMPLE, "Exhibit", 0),
            recto_x,
            104.0,
            336.0,
            182.0,
            &exhibit_45_story,
        )
        .into(),
    );
    extra_items.entry(45).or_default().push(
        text_frame(
            self_id(SAMPLE, "Exhibit", 1),
            verso_x,
            104.0,
            336.0,
            182.0,
            &exhibit_46_story,
        )
        .into(),
    );
    for (i, (page, label, topics)) in tabs.iter().enumerate() {
        let story = self_id(SAMPLE, "Story", story_seq);
        story_seq += 1;
        let mut segs: Vec<XSeg> = vec![XSeg::Text(label)];
        for (topic_id, topic_name) in *topics {
            segs.push(XSeg::Index {
                applied_topic: topic_id,
                topic_name,
            });
        }
        stories.push((
            story.clone(),
            write_exhibit_story(&story, STYLE_SPEC_LABEL, &segs),
        ));
        // A slim tab riding the OUTSIDE (verso left) margin, stepping
        // down the fore-edge tab-index style.
        let y = 130.0 + (i as f32) * 65.0;
        extra_items.entry(page - 1).or_default().push(
            text_frame(
                self_id(SAMPLE, "IndexTab", i as u32),
                12.0 - PAGE_W_PT,
                y,
                36.0,
                52.0,
                &story,
            )
            .into(),
        );
    }
    // p35 (recto): the footnote exhibit — see write_footnote_exhibit_story.
    let footnote_story = self_id(SAMPLE, "Story", story_seq);
    story_seq += 1;
    stories.push((
        footnote_story.clone(),
        write_footnote_exhibit_story(
            &footnote_story,
            "A footnote is the page apologising for an interruption it \
refuses to omit. The engine reserves its space through a compose, \
measure, and re-compose fixpoint that vertical justification then \
respects; the rule above the pool comes from the document footnote \
options, authored in the base fixture because no mutation writes \
them.",
            &[
                (
                    "Footnote/AnnualFn1",
                    "The reservation fixpoint: compose, measure the pool, compose again.",
                ),
                (
                    "Footnote/AnnualFn2",
                    "An oversized footnote does not yet split across frames — a recorded limit.",
                ),
            ],
        ),
    ));
    extra_items.entry(34).or_default().push(
        text_frame(
            self_id(SAMPLE, "Exhibit", 2),
            recto_x,
            104.0,
            336.0,
            240.0,
            &footnote_story,
        )
        .into(),
    );

    // p43 (recto) + p44 (verso): vertical-writing exhibits. Tall narrow
    // frames; columns run top-to-bottom, lines right-to-left.
    for (page_idx, exhibit_seq, x) in [(42usize, 3u32, recto_x + 96.0), (43usize, 4u32, verso_x + 96.0)] {
        let story = self_id(SAMPLE, "Story", story_seq);
        story_seq += 1;
        stories.push((
            story.clone(),
            write_vertical_exhibit_story(
                &story,
                "\u{7e26}\u{66f8}\u{304d}\u{306f}\u{6587}\u{5b57}\u{3092}\u{4e0a}\u{304b}\u{3089}\u{4e0b}\u{3078}\u{7d44}\u{307f}\u{3001}\u{884c}\u{306f}\u{53f3}\u{304b}\u{3089}\u{5de6}\u{3078}\u{9032}\u{3080}\u{3002}",
                "\u{6f22}\u{5b57}",
                "\u{304b}\u{3093}\u{3058}",
                "\u{5f37}\u{8abf}\u{70b9}\u{3002}",
            ),
        ));
        extra_items.entry(page_idx).or_default().push(
            text_frame(
                self_id(SAMPLE, "Exhibit", exhibit_seq),
                x,
                104.0,
                240.0,
                480.0,
                &story,
            )
            .into(),
        );
    }

    let _ = story_seq;

    // ── Spreads: p1 single, 66 facing, p134 single.
    let plan = page_plan();
    debug_assert_eq!(plan.len(), PAGE_COUNT);
    let page_name =
        |idx: usize| -> String { format!("{SAMPLE} · p{:03} · {}", idx + 1, plan[idx].1) };

    let mut spreads: Vec<(String, Vec<u8>)> = Vec::with_capacity(68);
    let mut spread_refs: Vec<String> = Vec::with_capacity(68);
    let mut spread_seq = 0u32;

    // Spread 0 — the single-page cover (p1, recto).
    {
        let spread_id = self_id(SAMPLE, "Spread", spread_seq);
        spread_seq += 1;
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_ids[0].clone(),
                page_name: page_name(0),
                applied_master: format!("MasterSpread/{}", role_master(plan[0].0)),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
                override_list: Vec::new(),
                margins: Some(margins_for(plan[0].0, false)),
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
    }

    // Spreads 1..=66 — facing: verso p(2k), recto p(2k+1).
    for k in 1..=66usize {
        let vi = 2 * k - 1; // 0-based verso index (physical 2k)
        let ri = 2 * k; // 0-based recto index (physical 2k+1)
        let spread_id = self_id(SAMPLE, "Spread", spread_seq);
        spread_seq += 1;
        let mut page_items: Vec<PageItem> = Vec::new();
        if let Some(items) = extra_items.remove(&vi) {
            page_items.extend(items);
        }
        if let Some(items) = extra_items.remove(&ri) {
            page_items.extend(items);
        }
        let recto_override = if ri == OVERRIDE_PAGE_IDX {
            vec![body_recto_head_frame_id.clone()]
        } else {
            Vec::new()
        };
        spreads.push((
            spread_id.clone(),
            write_facing_spread(&FacingSpread {
                self_id: spread_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                verso: FacingPage {
                    self_id: page_ids[vi].clone(),
                    name: page_name(vi),
                    applied_master: format!("MasterSpread/{}", role_master(plan[vi].0)),
                    override_list: Vec::new(),
                    margins: Some(margins_for(plan[vi].0, true)),
                },
                recto: FacingPage {
                    self_id: page_ids[ri].clone(),
                    name: page_name(ri),
                    applied_master: format!("MasterSpread/{}", role_master(plan[ri].0)),
                    override_list: recto_override,
                    margins: Some(margins_for(plan[ri].0, false)),
                },
                page_items,
            }),
        ));
        spread_refs.push(spread_id);
    }

    // Spread 67 — the single closing verso (p134, colophon).
    {
        let idx = PAGE_COUNT - 1;
        let spread_id = self_id(SAMPLE, "Spread", spread_seq);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_ids[idx].clone(),
                page_name: page_name(idx),
                applied_master: format!("MasterSpread/{}", role_master(plan[idx].0)),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
                override_list: Vec::new(),
                // A single-page spread's page sits at local index 0,
                // which routes it to the facing master's VERSO side —
                // correct for p134 — so it takes verso margins too.
                margins: Some(margins_for(plan[idx].0, true)),
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
    }
    debug_assert!(extra_items.is_empty(), "every exhibit found its spread");

    // ── Document-level marker resources.
    let markers = MarkerResources {
        layers: LAYER_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| LayerDef {
                self_id: self_id(SAMPLE, "Layer", i as u32),
                name: name.to_string(),
            })
            .collect(),
        text_variables: vec![
            TextVariableDef {
                self_id: VAR_EDITION.to_string(),
                name: "Edition".to_string(),
                variable_type: "CustomTextType".to_string(),
                contents: Some("Edition: MMXXVI".to_string()),
                ..Default::default()
            },
            TextVariableDef {
                self_id: VAR_RUNNING_HEADER.to_string(),
                name: "Running Header".to_string(),
                variable_type: "RunningHeaderType".to_string(),
                running_header_style: Some(STYLE_CHAPTER_TITLE.to_string()),
                running_header_use: Some("FirstOnPage".to_string()),
                ..Default::default()
            },
            TextVariableDef {
                self_id: VAR_CHAPTER.to_string(),
                name: "Chapter".to_string(),
                variable_type: "ChapterNumberType".to_string(),
                ..Default::default()
            },
            TextVariableDef {
                self_id: VAR_PAGE_COUNT.to_string(),
                name: "Page Count".to_string(),
                variable_type: "PageCountType".to_string(),
                ..Default::default()
            },
        ],
        hyperlink_destinations: vec![
            HyperlinkDestinationDef::Page {
                self_id: page_dest_ch01.clone(),
                // p13 — the first chapter opener.
                page: page_ids[12].clone(),
            },
            HyperlinkDestinationDef::Page {
                self_id: page_dest_data.clone(),
                // p123 — the data-table block.
                page: page_ids[122].clone(),
            },
            HyperlinkDestinationDef::TextAnchor {
                self_id: text_dest_45.clone(),
                story: exhibit_45_story.clone(),
            },
            HyperlinkDestinationDef::TextAnchor {
                self_id: text_dest_46.clone(),
                story: exhibit_46_story.clone(),
            },
            HyperlinkDestinationDef::Url {
                self_id: url_dest.clone(),
                url: "https://paged.media".to_string(),
            },
        ],
        hyperlinks: vec![
            HyperlinkDef {
                self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 0)),
                name: "registry-url".to_string(),
                source: url_source,
                destination: url_dest,
            },
            HyperlinkDef {
                self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 1)),
                name: "back-to-ch01".to_string(),
                source: page_source,
                destination: page_dest_ch01.clone(),
            },
            HyperlinkDef {
                // The cross-reference rides the hyperlink machinery:
                // source span → the p46 text anchor.
                self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 2)),
                name: "xref-overleaf".to_string(),
                source: xref_source,
                destination: text_dest_46.clone(),
            },
        ],
        bookmarks: vec![
            BookmarkDef {
                self_id: BOOKMARK_APPARATUS.to_string(),
                name: BOOKMARK_NAMES[0].to_string(),
                destination: text_dest_45,
            },
            BookmarkDef {
                self_id: BOOKMARK_CHAPTER_ONE.to_string(),
                name: BOOKMARK_NAMES[1].to_string(),
                destination: page_dest_ch01,
            },
            BookmarkDef {
                self_id: BOOKMARK_DATA_TABLES.to_string(),
                name: BOOKMARK_NAMES[2].to_string(),
                destination: page_dest_data,
            },
        ],
        index_topics: INDEX_TOPICS
            .iter()
            .map(|(id, name)| IndexTopicDef {
                self_id: id.to_string(),
                name: name.to_string(),
            })
            .collect(),
        // The full FootnoteOptionDef battery (no create-footnote-option
        // op exists on the wire).
        footnote_option: Some(FootnoteOptionDef {
            rule_on: Some(true),
            rule_color: Some(SWATCH_VERMILION.to_string()),
            rule_tint: Some(100.0),
            rule_line_weight: Some(0.5),
            rule_width: Some(62.0),
            rule_left_indent: Some(0.0),
            rule_offset: Some(4.0),
            separator_text: Some(" ".to_string()),
            spacer: Some(13.0),
            space_between: Some(6.5),
        }),
        // NO sections — a live `insertSection` on p1 re-bakes the
        // Page.Name labels to numeric folios (see module doc).
        sections: Vec::new(),
    };

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: master_ids.clone(),
            spreads: spread_refs,
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        },
        &markers,
    );

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic(),
        fonts_xml: fonts(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![
            front_master,
            body_master,
            opener_master,
            plate_master,
            data_master,
            vertical_master,
            appendix_master,
        ],
        spreads,
        stories,
    }
}
