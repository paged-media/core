//! `Resources/Styles.xml` — paragraph and character style sheet.
//!
//! IDML's typical layout:
//!
//! ```xml
//! <idPkg:Styles>
//!   <RootCharacterStyleGroup>
//!     <CharacterStyle Self="CharacterStyle/$ID/[No character style]" .../>
//!     <CharacterStyle Self="CharacterStyle/Bold" FontStyle="Bold" .../>
//!   </RootCharacterStyleGroup>
//!   <RootParagraphStyleGroup>
//!     <ParagraphStyle Self="ParagraphStyle/Body"
//!                     AppliedFont="Body Font"
//!                     PointSize="11" .../>
//!   </RootParagraphStyleGroup>
//! </idPkg:Styles>
//! ```
//!
//! Only the cascadable attributes the renderer currently consumes
//! land here (font / style / size / fill / tracking + paragraph
//! geometry knobs). `BasedOn` chains are followed at resolve time;
//! cycles are bounded by `MAX_BASED_ON_DEPTH`.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use serde::Serialize;

use crate::spread::{CornerOption, CornerSpec};
use crate::story::{Justification, TabStop};
use crate::util::{attr, parse_tint_attr};
use crate::ParseError;

/// Maximum BasedOn chain length. IDML doesn't forbid cycles, so the
/// resolver short-circuits once it hits this depth — typical real-
/// world chains are 1–3 hops.
const MAX_BASED_ON_DEPTH: usize = 16;

#[derive(Debug, Default, Clone, Serialize)]
pub struct StyleSheet {
    pub character_styles: BTreeMap<String, CharacterStyleDef>,
    pub paragraph_styles: BTreeMap<String, ParagraphStyleDef>,
    /// `<ObjectStyle>` definitions from `<RootObjectStyleGroup>`.
    /// Page-item shapes (TextFrame, Rectangle, Oval, GraphicLine,
    /// Polygon) reference these via `AppliedObjectStyle="..."` to
    /// inherit fill / stroke / etc. when their own attributes are
    /// absent. Real-world IDMLs use this almost exclusively for
    /// rectangle fills.
    pub object_styles: BTreeMap<String, ObjectStyleDef>,
    /// `<CellStyle>` definitions from `<RootCellStyleGroup>`. Cells
    /// reference these via `AppliedCellStyle="..."` to inherit
    /// fill / VJ / per-edge strokes when their own attributes are
    /// absent.
    pub cell_styles: BTreeMap<String, CellStyleDef>,
    /// `<TableStyle>` definitions. Tables reference one via
    /// `AppliedTableStyle="..."`; the style nominates a default
    /// CellStyle per region (header, body, footer, left column,
    /// right column) plus the table-level border strokes.
    pub table_styles: BTreeMap<String, TableStyleDef>,
    /// `<TOCStyle>` definitions from `Resources/Styles.xml`. Each
    /// carries an ordered list of `<TOCStyleEntry>` children
    /// declaring which paragraph styles feed the TOC, the format
    /// style applied to each rendered entry, and the page-number /
    /// separator settings. Real-world IDMLs commonly serialise a
    /// single default empty TOCStyle (no entries) alongside any
    /// user-defined ones.
    pub toc_styles: BTreeMap<String, TOCStyleDef>,
    /// Track 4a: custom `<DashedStrokeStyle>` / `<DottedStrokeStyle>` /
    /// `<StripedStrokeStyle>` definitions from `Resources/Styles.xml`.
    /// Page items reference these via `StrokeType="StrokeStyle/<id>"`;
    /// without this table the renderer fell back to `Solid` for every
    /// user-defined stroke (e.g. business-proposal-template's
    /// diagonal-stripe cover, which is a dense custom dash).
    pub stroke_styles: BTreeMap<String, StrokeStyleDef>,
}

/// Custom stroke-style definition. Today the renderer consumes the
/// `Dashed` variant (the others are captured so we don't lose them
/// during round-trips and so a future track can grow into them).
#[derive(Debug, Clone, Serialize)]
pub struct StrokeStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub kind: StrokeStyleKind,
    /// On/off pattern in pt for `Dashed` (the `Pattern` attribute
    /// parsed as space-separated floats). Empty for the other kinds.
    pub pattern: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StrokeStyleKind {
    Dashed,
    Dotted,
    Striped,
    Wavy,
}

/// `<TOCStyle>` — Table of Contents style. Carries the heading text,
/// the paragraph style for the title, and an ordered list of
/// `<TOCStyleEntry>` children declaring which paragraph styles
/// should be picked up as TOC entries.
#[derive(Debug, Default, Clone, Serialize)]
pub struct TOCStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    /// `Title` attribute — the heading text printed at the top of
    /// the resolved TOC story (e.g. `"Contents"` / `"Inhalt"`).
    /// `None` when omitted; some IDMLs use an empty string.
    pub title: Option<String>,
    /// `TitleStyle` — `ParagraphStyle/<id>` reference applied to
    /// the title paragraph. May resolve to the `[No paragraph
    /// style]` sentinel for the default TOCStyle.
    pub title_style: Option<String>,
    /// `IncludeBookDocuments` — true when entries should be pulled
    /// from sibling book documents in addition to this one. Single-
    /// document renders ignore this; captured for round-trip.
    pub include_book_documents: Option<bool>,
    /// `IncludeHidden` — when true the resolver should also pick up
    /// paragraphs on hidden layers. The renderer currently honours
    /// layer visibility at emission time and matches this default.
    pub include_hidden: Option<bool>,
    /// `RunIn` — when true, sibling entries at the same level
    /// concatenate on the same line separated by a soft separator
    /// rather than each landing on its own line. The current
    /// resolver leaves run-in handling to the renderer; captured
    /// here for round-trip.
    pub run_in: Option<bool>,
    /// Ordered list of `<TOCStyleEntry>` children in document order.
    pub entries: Vec<TOCStyleEntryDef>,
}

/// `<TOCStyleEntry>` — one row in the TOC style table. IDML serialises
/// these in document order under the `<TOCStyle>`.
#[derive(Debug, Default, Clone, Serialize)]
pub struct TOCStyleEntryDef {
    /// `Name` — human-readable label (usually mirrors the paragraph
    /// style name picked up by `IncludeStyle`).
    pub name: Option<String>,
    /// `IncludeStyle` — `ParagraphStyle/<id>` reference. Paragraphs
    /// with this applied paragraph style feed the TOC entry.
    pub include_style: Option<String>,
    /// `FormatStyle` — `ParagraphStyle/<id>` reference applied to
    /// the rendered TOC entry paragraph.
    pub format_style: Option<String>,
    /// `Level` — outline depth (1 is the top level). `None` falls
    /// back to 1 at resolve time.
    pub level: Option<u32>,
    /// `PageNumber` — IDML enum (`On` / `Off` / `NoPageNumber`).
    /// `On` is the default when absent.
    pub page_number: Option<String>,
    /// `Separator` — string placed between the entry text and the
    /// page number. IDML serialises tabs as `^t`; the resolver
    /// expands them at use time. Default `^t` when absent.
    pub separator: Option<String>,
}

/// `<ObjectStyle>` — the page-item analogue of paragraph/character
/// styles. Carries fill + stroke defaults that flow into a frame
/// when it carries no per-element override.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ObjectStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub fill_color: Option<String>,
    /// `FillTint` percentage [0..100] from `<ObjectStyle FillTint="…">`.
    /// `None` ⇒ inherit from BasedOn (and ultimately default to 100%
    /// at the renderer). Cascades into a frame whose own inline
    /// `FillTint` is absent — needed for placeholder rects whose
    /// 15% grey paint comes entirely from the style.
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_tint: Option<f32>,
    pub stroke_weight: Option<f32>,
    /// `CornerRadius` in pt. Only honoured when `CornerOption` is one
    /// of the rounding variants (`Rounded`, `InverseRounded`, `Inset`,
    /// `Bevel`, `Fancy`). `None` ⇒ inherit from BasedOn.
    pub corner_radius: Option<f32>,
    /// `CornerOption` value (`None | Rounded | InverseRounded | Inset
    /// | Bevel | Fancy`). The renderer maps `Rounded` to a rounded-
    /// rect path; the decorative variants currently fall back to
    /// `Rounded` until per-shape parsers land.
    pub corner_option: Option<String>,
}

/// Effective object-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedObject {
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_tint: Option<f32>,
    pub stroke_weight: Option<f32>,
    pub corner_radius: Option<f32>,
    pub corner_option: Option<String>,
}

/// `<CellStyle>` — per-cell defaults for fill, edge strokes, and
/// vertical justification. Cells can override individual fields
/// inline; missing fields cascade through `BasedOn` and finally
/// fall through to renderer defaults.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CellStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub fill_color: Option<String>,
    pub vertical_justification: Option<String>,
    pub top_edge_stroke_color: Option<String>,
    pub top_edge_stroke_weight: Option<f32>,
    pub bottom_edge_stroke_color: Option<String>,
    pub bottom_edge_stroke_weight: Option<f32>,
    pub left_edge_stroke_color: Option<String>,
    pub left_edge_stroke_weight: Option<f32>,
    pub right_edge_stroke_color: Option<String>,
    pub right_edge_stroke_weight: Option<f32>,
}

/// `<TableStyle>` — table-level defaults that flow through to
/// cells. Carries the region → CellStyle map (Header / Body /
/// Footer / Left / Right column regions) plus the table border
/// strokes. BasedOn cascade applies the same way as the other
/// resolvers.
#[derive(Debug, Default, Clone, Serialize)]
pub struct TableStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub header_region_cell_style: Option<String>,
    pub body_region_cell_style: Option<String>,
    pub footer_region_cell_style: Option<String>,
    pub left_column_region_cell_style: Option<String>,
    pub right_column_region_cell_style: Option<String>,
    pub top_border_stroke_color: Option<String>,
    pub top_border_stroke_weight: Option<f32>,
    pub bottom_border_stroke_color: Option<String>,
    pub bottom_border_stroke_weight: Option<f32>,
    pub left_border_stroke_color: Option<String>,
    pub left_border_stroke_weight: Option<f32>,
    pub right_border_stroke_color: Option<String>,
    pub right_border_stroke_weight: Option<f32>,
    /// Alternating-row fill: every Nth body row from the top gets
    /// `start_row_fill_color`. `start_row_fill_count` is the
    /// number of consecutive rows that participate in the
    /// "starting" fill before alternating to the end-row fill.
    pub start_row_fill_color: Option<String>,
    pub start_row_fill_count: Option<u32>,
    pub start_row_fill_tint: Option<f32>,
    pub end_row_fill_color: Option<String>,
    pub end_row_fill_count: Option<u32>,
    pub end_row_fill_tint: Option<f32>,
}

/// Effective table-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedTable {
    pub header_region_cell_style: Option<String>,
    pub body_region_cell_style: Option<String>,
    pub footer_region_cell_style: Option<String>,
    pub left_column_region_cell_style: Option<String>,
    pub right_column_region_cell_style: Option<String>,
    pub top_border_stroke_color: Option<String>,
    pub top_border_stroke_weight: Option<f32>,
    pub bottom_border_stroke_color: Option<String>,
    pub bottom_border_stroke_weight: Option<f32>,
    pub left_border_stroke_color: Option<String>,
    pub left_border_stroke_weight: Option<f32>,
    pub right_border_stroke_color: Option<String>,
    pub right_border_stroke_weight: Option<f32>,
    pub start_row_fill_color: Option<String>,
    pub start_row_fill_count: Option<u32>,
    pub start_row_fill_tint: Option<f32>,
    pub end_row_fill_color: Option<String>,
    pub end_row_fill_count: Option<u32>,
    pub end_row_fill_tint: Option<f32>,
}

/// Effective cell-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedCell {
    pub fill_color: Option<String>,
    pub vertical_justification: Option<String>,
    pub top_edge_stroke_color: Option<String>,
    pub top_edge_stroke_weight: Option<f32>,
    pub bottom_edge_stroke_color: Option<String>,
    pub bottom_edge_stroke_weight: Option<f32>,
    pub left_edge_stroke_color: Option<String>,
    pub left_edge_stroke_weight: Option<f32>,
    pub right_edge_stroke_color: Option<String>,
    pub right_edge_stroke_weight: Option<f32>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct CharacterStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    /// `FillTint` — see `CharacterRun::fill_tint` for semantics.
    pub fill_tint: Option<f32>,
    /// `StrokeColor` declared on the `<CharacterStyle>`. Cascades
    /// through `BasedOn` like every other field. `Swatch/None` is
    /// normalised to `None` at parse time so a cascade can fall
    /// through to a real colour from the base style.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` declared on the `<CharacterStyle>` in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `OverprintFill="true"` declared on the `<CharacterStyle>`.
    /// Cascades through `BasedOn` like every other field. None ⇒
    /// inherit; bottom of cascade = false (IDML's default).
    pub overprint_fill: Option<bool>,
    /// `OverprintStroke="true"` analogue. Currently rare on text
    /// runs (only outlined text carries a stroke) but parsed for
    /// completeness.
    pub overprint_stroke: Option<bool>,
    /// `RubyFlag` — when `true`, this character style carries ruby
    /// annotation. See [`CharacterRun::ruby_flag`]. Parser-only;
    /// renderer integration is queued under Tier 4 — CJK Stage 4.
    pub ruby_flag: Option<bool>,
    /// `RubyType` — `PerCharacter` / `GroupRuby`. See
    /// [`CharacterRun::ruby_type`].
    pub ruby_type: Option<String>,
    /// `RubyString` — the ruby annotation text. See
    /// [`CharacterRun::ruby_string`].
    pub ruby_string: Option<String>,
    /// `KentenKind` — emphasis-mark glyph. See
    /// [`CharacterRun::kenten_kind`].
    pub kenten_kind: Option<String>,
    /// `KentenCharacter` — custom emphasis-mark codepoint when
    /// `kenten_kind == "Custom"`.
    pub kenten_character: Option<String>,
    /// `KentenFontSize` — emphasis-mark size as a % of base size.
    pub kenten_font_size: Option<f32>,
}

/// Q-09: `ParagraphShading*` attributes parsed off a
/// `<ParagraphStyle>` or `<ParagraphStyleRange>`. The renderer emits
/// a coloured rectangle behind each line of the paragraph when `on`
/// is true. `None` for any field means "not set at this level" so the
/// cascade can inherit from `BasedOn`. The decorative per-corner
/// options + radii live alongside the bag in case a future cycle
/// renders rounded shading bands.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ParagraphShading {
    pub on: Option<bool>,
    pub color: Option<String>,
    pub tint: Option<f32>,
    /// `ColumnWidth` | `TextWidth`. None ⇒ ColumnWidth default.
    pub width: Option<String>,
    /// Inset offsets in pt, order `[top, left, bottom, right]`.
    pub offset_top: Option<f32>,
    pub offset_left: Option<f32>,
    pub offset_bottom: Option<f32>,
    pub offset_right: Option<f32>,
    /// `AscentTopOrigin` | `BaselineTopOrigin` | etc. Drives the
    /// shading band's top edge: `None` ⇒ AscentTopOrigin default.
    pub top_origin: Option<String>,
    /// `DescentBottomOrigin` | `BaselineBottomOrigin` | etc.
    pub bottom_origin: Option<String>,
    pub clip_to_frame: Option<bool>,
    pub overprint: Option<bool>,
    pub suppress_printing: Option<bool>,
}

/// Q-09: `RuleAbove*` / `RuleBelow*` rule-line parameters parsed
/// off a `<ParagraphStyle>` or `<ParagraphStyleRange>`. The renderer
/// strokes a horizontal line above the first line (RuleAbove) or
/// below the last line (RuleBelow) of the paragraph when `on` is
/// true. Only the fields actually consumed by the renderer are
/// listed; gap / stroke-style / overprint variants are queued.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ParagraphRule {
    pub on: Option<bool>,
    pub color: Option<String>,
    pub tint: Option<f32>,
    /// Stroke weight in pt.
    pub weight: Option<f32>,
    /// Distance from the paragraph's baseline (RuleAbove) or
    /// descent (RuleBelow) to the rule.
    pub offset: Option<f32>,
    pub left_indent: Option<f32>,
    pub right_indent: Option<f32>,
    /// `ColumnWidth` | `TextWidth`. None ⇒ ColumnWidth default.
    pub width: Option<String>,
}

impl ParagraphRule {
    /// Parse the `<prefix>*` attrs. `prefix` is either `"RuleAbove"`
    /// or `"RuleBelow"` to match IDML's two attribute families.
    pub fn from_attrs(e: &quick_xml::events::BytesStart, prefix: &str) -> Self {
        // Construct attr names on the fly. quick-xml accepts &[u8] keys
        // for `attr()`; building owned Vec<u8> per attr is fine — this
        // runs once per style at parse time.
        let key = |suffix: &str| -> Vec<u8> {
            let mut v = Vec::with_capacity(prefix.len() + suffix.len());
            v.extend_from_slice(prefix.as_bytes());
            v.extend_from_slice(suffix.as_bytes());
            v
        };
        Self {
            on: attr(e, &key("")).and_then(|s| s.parse().ok()),
            color: attr(e, &key("Color")),
            tint: attr(e, &key("Tint")).and_then(|s| s.parse().ok()),
            weight: attr(e, &key("LineWeight"))
                .and_then(|s| s.parse().ok())
                .or_else(|| attr(e, &key("Weight")).and_then(|s| s.parse().ok())),
            offset: attr(e, &key("Offset")).and_then(|s| s.parse().ok()),
            left_indent: attr(e, &key("LeftIndent")).and_then(|s| s.parse().ok()),
            right_indent: attr(e, &key("RightIndent")).and_then(|s| s.parse().ok()),
            width: attr(e, &key("Width")),
        }
    }
}

/// Q-09: `ParagraphBorder*` attributes parsed off a `<ParagraphStyle>`
/// or `<ParagraphStyleRange>`. The renderer strokes a rectangular
/// border around the paragraph's content box when `on` is true.
/// Per-corner `CornerOption` / `CornerRadius` attrs are honoured via
/// `corners` (Track 4d) — order matches `Rectangle::corners`:
/// `[top_left, top_right, bottom_right, bottom_left]`.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ParagraphBorder {
    pub on: Option<bool>,
    pub color: Option<String>,
    pub tint: Option<f32>,
    /// Stroke weight in pt.
    pub weight: Option<f32>,
    /// Inset offsets in pt.
    pub offset_top: Option<f32>,
    pub offset_left: Option<f32>,
    pub offset_bottom: Option<f32>,
    pub offset_right: Option<f32>,
    /// `ColumnWidth` | `TextWidth`. None ⇒ ColumnWidth default.
    pub width: Option<String>,
    /// Per-corner option/radius overrides. `[tl, tr, br, bl]`.
    pub corners: [CornerSpec; 4],
}

impl ParagraphBorder {
    /// Parse the `ParagraphBorder*` attrs off a `<ParagraphStyle>`
    /// (or `<ParagraphStyleRange>`) element. Returns a fully-default
    /// instance when no attrs are present; callers can check `on` to
    /// know whether to emit.
    pub fn from_attrs(e: &quick_xml::events::BytesStart) -> Self {
        // Order matches Rectangle::corners — clockwise from top-left.
        let per = [
            ("ParagraphBorderTopLeftCornerOption", "ParagraphBorderTopLeftCornerRadius"),
            ("ParagraphBorderTopRightCornerOption", "ParagraphBorderTopRightCornerRadius"),
            ("ParagraphBorderBottomRightCornerOption", "ParagraphBorderBottomRightCornerRadius"),
            ("ParagraphBorderBottomLeftCornerOption", "ParagraphBorderBottomLeftCornerRadius"),
        ];
        let mut corners = [CornerSpec::default(); 4];
        for (i, (oname, rname)) in per.iter().enumerate() {
            corners[i].option = attr(e, oname.as_bytes())
                .as_deref()
                .and_then(CornerOption::from_idml);
            corners[i].radius = attr(e, rname.as_bytes()).and_then(|s| s.parse().ok());
        }
        Self {
            on: attr(e, b"ParagraphBorderOn").and_then(|s| s.parse().ok()),
            color: attr(e, b"ParagraphBorderColor"),
            tint: attr(e, b"ParagraphBorderTint").and_then(|s| s.parse().ok()),
            weight: attr(e, b"ParagraphBorderWeight").and_then(|s| s.parse().ok()),
            offset_top: attr(e, b"ParagraphBorderTopOffset").and_then(|s| s.parse().ok()),
            offset_left: attr(e, b"ParagraphBorderLeftOffset").and_then(|s| s.parse().ok()),
            offset_bottom: attr(e, b"ParagraphBorderBottomOffset").and_then(|s| s.parse().ok()),
            offset_right: attr(e, b"ParagraphBorderRightOffset").and_then(|s| s.parse().ok()),
            width: attr(e, b"ParagraphBorderWidth"),
            corners,
        }
    }
}

impl ParagraphShading {
    /// Parse the `ParagraphShading*` attrs off a `<ParagraphStyle>`
    /// (or `<ParagraphStyleRange>`) element. Returns a fully-default
    /// instance when no attrs are present; callers can check `on` to
    /// know whether to emit.
    pub fn from_attrs(e: &quick_xml::events::BytesStart) -> Self {
        Self {
            on: attr(e, b"ParagraphShadingOn").and_then(|s| s.parse().ok()),
            color: attr(e, b"ParagraphShadingColor"),
            tint: attr(e, b"ParagraphShadingTint").and_then(|s| s.parse().ok()),
            width: attr(e, b"ParagraphShadingWidth"),
            offset_top: attr(e, b"ParagraphShadingTopOffset").and_then(|s| s.parse().ok()),
            offset_left: attr(e, b"ParagraphShadingLeftOffset").and_then(|s| s.parse().ok()),
            offset_bottom: attr(e, b"ParagraphShadingBottomOffset").and_then(|s| s.parse().ok()),
            offset_right: attr(e, b"ParagraphShadingRightOffset").and_then(|s| s.parse().ok()),
            top_origin: attr(e, b"ParagraphShadingTopOrigin"),
            bottom_origin: attr(e, b"ParagraphShadingBottomOrigin"),
            clip_to_frame: attr(e, b"ParagraphShadingClipToFrame").and_then(|s| s.parse().ok()),
            overprint: attr(e, b"ParagraphShadingOverprint").and_then(|s| s.parse().ok()),
            suppress_printing: attr(e, b"ParagraphShadingSuppressPrinting")
                .and_then(|s| s.parse().ok()),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ParagraphStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    /// `FillTint` — see `CharacterRun::fill_tint` for semantics.
    pub fill_tint: Option<f32>,
    /// `StrokeColor` declared on the `<ParagraphStyle>` — the paint
    /// used to outline glyphs whose run / character style don't
    /// override it. `Swatch/None` normalises to `None`.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` declared on the `<ParagraphStyle>` in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    /// `Justification` from the style. Parsed into the typed
    /// `Justification` enum at XML-read time.
    pub justification: Option<Justification>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `<TabList>` parsed from the style. Empty means "no
    /// declaration" — the cascade may inherit from `BasedOn`.
    pub tab_list: Vec<TabStop>,
    /// `BulletsAndNumberingListType`: `BulletList` /
    /// `NumberedList` / `NoList`. `None` when absent.
    pub bullets_list_type: Option<String>,
    /// `<BulletChar BulletCharacterValue="...">` — Unicode code
    /// point of the bullet glyph. None when no bullet declared.
    pub bullet_character: Option<u32>,
    /// `BulletsTextAfter` — string rendered between the bullet
    /// and the paragraph text (typically a tab `^t` or a space).
    /// IDML serialises tabs as the literal `^t` sequence.
    pub bullets_text_after: Option<String>,
    /// `NumberingFormat` for `NumberedList` paragraphs. IDML
    /// serialises these as the literal sample string, e.g.
    /// `"1, 2, 3, 4..."`, `"I, II, III, IV..."`,
    /// `"01, 02, 03, 04..."`, `"A, B, C, D..."`. The renderer
    /// reads only the prefix before the first comma to decide
    /// the format. `None` falls back to Arabic.
    pub numbering_format: Option<String>,
    /// `BulletsCharacterStyle` — a `CharacterStyle/<id>` reference
    /// that styles the bullet marker (font, size, colour) independently
    /// of the paragraph text. IDML applies this only to `BulletList`
    /// paragraphs. `None` ⇒ the bullet inherits the first run's
    /// formatting (the historical fallback).
    pub bullets_character_style: Option<String>,
    /// `BulletsAndNumberingDigitsCharacterStyle` — a `CharacterStyle/<id>`
    /// reference that styles the digits of a `NumberedList` paragraph's
    /// marker. IDML overloads this same field as the bullet-style
    /// reference for `BulletList` paragraphs when
    /// `bullets_character_style` is absent (the InDesign UI presents
    /// one "Character Style" picker regardless of list kind), so the
    /// renderer falls back to it when shaping bullets.
    pub bullets_and_numbering_digits_character_style: Option<String>,
    /// `NumberingExpression` — the formatting template for the
    /// numbered-list marker. Tokens:
    /// - `^#` substitutes the formatted counter (per
    ///   `numbering_format`),
    /// - `^.` is a literal period,
    /// - `^t` is a literal tab.
    ///
    /// Anything else passes through unchanged. `None` falls back
    /// to the IDML default `^#.^t` (e.g. `"1.\t"`).
    pub numbering_expression: Option<String>,
    /// `NumberingStartAt` — explicit integer the paragraph's
    /// counter starts at. Overrides any continued count from a
    /// previous paragraph. `None` means "no explicit start"; the
    /// counter increments off whatever the story carries.
    pub numbering_start_at: Option<i32>,
    /// `NumberingContinue` — when `true`, the counter persists
    /// across the previous paragraph (even if that paragraph
    /// applied a different style or wasn't a numbered list at all,
    /// up to whatever the previous numbered-list state was). When
    /// `false`, the counter resets at the start of this paragraph.
    /// `None` ⇒ inherit; the renderer's default is "continue".
    pub numbering_continue: Option<bool>,
    /// `Hyphenation` boolean. IDML default is true; the resolver
    /// only flips a paragraph off when an explicit `Hyphenation="false"`
    /// lands on the cascade. Drives whether the composer registers a
    /// language-specific hyphenator with the breaker.
    pub hyphenation: Option<bool>,
    /// `AppliedLanguage` reference (e.g. `$ID/English: USA`). Used to
    /// pick the hyphenation dictionary; unrecognised values fall back
    /// to English-US so we always have *some* dictionary loaded.
    pub applied_language: Option<String>,
    /// `MinimumWordSpacing` percentage (`80` = 80% of normal). Drives
    /// the composer's shrink ratio.
    pub minimum_word_spacing: Option<f32>,
    /// `DesiredWordSpacing` percentage (`100` = 100% of normal). The
    /// renderer scales `Min`/`Max` against this so the composer's
    /// ratios stay relative to the desired baseline.
    pub desired_word_spacing: Option<f32>,
    /// `MaximumWordSpacing` percentage (`133` = 133% of normal).
    /// Drives the composer's stretch ratio.
    pub maximum_word_spacing: Option<f32>,
    /// Q-20: `MinimumLetterSpacing` pt (additive, signed). Allows
    /// the composer to tighten inter-glyph advance up to this much
    /// when justifying lines.
    pub minimum_letter_spacing: Option<f32>,
    /// Q-20: `DesiredLetterSpacing` pt (default 0 = none).
    pub desired_letter_spacing: Option<f32>,
    /// Q-20: `MaximumLetterSpacing` pt (additive, signed).
    pub maximum_letter_spacing: Option<f32>,
    /// Q-20: `MinimumGlyphScaling` percent (default 100 = identity).
    /// Allows per-glyph x-advance scaling for justification.
    pub minimum_glyph_scaling: Option<f32>,
    /// Q-20: `DesiredGlyphScaling` percent.
    pub desired_glyph_scaling: Option<f32>,
    /// Q-20: `MaximumGlyphScaling` percent.
    pub maximum_glyph_scaling: Option<f32>,
    /// `DropCapCharacters` count. 0 / `None` ⇒ no drop cap.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` — vertical extent of the drop cap.
    pub drop_cap_lines: Option<u32>,
    /// `DropCapDetail` — InDesign's scaling-factor integer.
    pub drop_cap_detail: Option<i32>,
    /// `OverprintFill="true"` declared on the `<ParagraphStyle>`. See
    /// [`CharacterStyleDef::overprint_fill`]. Cascades like every other
    /// paragraph attribute via `merge_below`.
    pub overprint_fill: Option<bool>,
    /// `OverprintStroke="true"` analogue.
    pub overprint_stroke: Option<bool>,
    /// `KinsokuSet="KinsokuTable/$ID/PhotoshopKinsokuHard"` ref on the
    /// `<ParagraphStyle>`. Cascades like every other paragraph attribute.
    /// See [`Paragraph::kinsoku_set`].
    pub kinsoku_set: Option<String>,
    /// `KinsokuType` flavour. See [`Paragraph::kinsoku_type`].
    pub kinsoku_type: Option<String>,
    /// `MojikumiTable` ref. See [`Paragraph::mojikumi_table`].
    pub mojikumi_table: Option<String>,
    /// `MojikumiSet` (older IDML attribute name; see
    /// [`Paragraph::mojikumi_set`]).
    pub mojikumi_set: Option<String>,
    /// Q-09: paragraph-level shading band parameters. `on` defaulting
    /// to `None` means "not declared at this style level" so the
    /// `BasedOn` cascade can inherit. Renderer emit module is a
    /// separate follow-up.
    pub shading: ParagraphShading,
    /// Q-09: horizontal rule above the first line of the paragraph.
    pub rule_above: ParagraphRule,
    /// Q-09: horizontal rule below the last line of the paragraph.
    pub rule_below: ParagraphRule,
    /// Q-09: rectangular border around the paragraph's content box.
    pub border: ParagraphBorder,
}

/// Effective character-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedCharacter {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    /// Cascaded text-stroke colour. See
    /// [`CharacterStyleDef::stroke_color`].
    pub stroke_color: Option<String>,
    /// Cascaded text-stroke weight in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// Cascaded `OverprintFill` flag. See
    /// [`CharacterStyleDef::overprint_fill`]. None at the bottom of
    /// the cascade ⇒ false (the IDML default).
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag.
    pub overprint_stroke: Option<bool>,
    /// Cascaded `RubyFlag`. See [`CharacterStyleDef::ruby_flag`].
    pub ruby_flag: Option<bool>,
    /// Cascaded `RubyType`.
    pub ruby_type: Option<String>,
    /// Cascaded `RubyString`.
    pub ruby_string: Option<String>,
    /// Cascaded `KentenKind`.
    pub kenten_kind: Option<String>,
    /// Cascaded `KentenCharacter`.
    pub kenten_character: Option<String>,
    /// Cascaded `KentenFontSize`.
    pub kenten_font_size: Option<f32>,
}

/// Effective paragraph-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraph {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    /// Cascaded text-stroke colour. See
    /// [`ParagraphStyleDef::stroke_color`].
    pub stroke_color: Option<String>,
    /// Cascaded text-stroke weight in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub justification: Option<Justification>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `<TabList>` from the cascade. Empty means inherited / none.
    pub tab_list: Vec<TabStop>,
    pub bullets_list_type: Option<String>,
    pub bullet_character: Option<u32>,
    pub bullets_text_after: Option<String>,
    pub numbering_format: Option<String>,
    /// Cascaded `BulletsCharacterStyle` ref. See
    /// [`ParagraphStyleDef::bullets_character_style`].
    pub bullets_character_style: Option<String>,
    /// Cascaded `BulletsAndNumberingDigitsCharacterStyle` ref. See
    /// [`ParagraphStyleDef::bullets_and_numbering_digits_character_style`].
    pub bullets_and_numbering_digits_character_style: Option<String>,
    /// `NumberingExpression` template (`^#`, `^.`, `^t` tokens
    /// plus literal characters). `None` ⇒ renderer default `^#.^t`.
    pub numbering_expression: Option<String>,
    /// `NumberingStartAt` explicit start integer. See
    /// `ParagraphStyleDef::numbering_start_at`.
    pub numbering_start_at: Option<i32>,
    /// `NumberingContinue` flag. See
    /// `ParagraphStyleDef::numbering_continue`.
    pub numbering_continue: Option<bool>,
    pub hyphenation: Option<bool>,
    pub applied_language: Option<String>,
    pub minimum_word_spacing: Option<f32>,
    pub desired_word_spacing: Option<f32>,
    pub maximum_word_spacing: Option<f32>,
    /// Q-20: cascaded letter / glyph spacing knobs.
    pub minimum_letter_spacing: Option<f32>,
    pub desired_letter_spacing: Option<f32>,
    pub maximum_letter_spacing: Option<f32>,
    pub minimum_glyph_scaling: Option<f32>,
    pub desired_glyph_scaling: Option<f32>,
    pub maximum_glyph_scaling: Option<f32>,
    /// `DropCapCharacters` count (number of leading characters that
    /// drop down across `drop_cap_lines` lines). 0 / `None` ⇒ no
    /// drop cap.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` count (lines the drop cap spans). 0 / `None` ⇒
    /// no drop cap.
    pub drop_cap_lines: Option<u32>,
    /// `DropCapDetail` (the IDML scaling factor InDesign records on
    /// the drop cap's character formatting; an arbitrary integer).
    pub drop_cap_detail: Option<i32>,
    /// Cascaded `OverprintFill` flag from the paragraph style chain.
    /// See [`CharacterStyleDef::overprint_fill`].
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag.
    pub overprint_stroke: Option<bool>,
    /// Cascaded `KinsokuSet` ref. See [`Paragraph::kinsoku_set`].
    pub kinsoku_set: Option<String>,
    /// Cascaded `KinsokuType` flavour.
    pub kinsoku_type: Option<String>,
    /// Cascaded `MojikumiTable` ref.
    pub mojikumi_table: Option<String>,
    /// Cascaded `MojikumiSet` ref.
    pub mojikumi_set: Option<String>,
    /// Q-09: cascaded paragraph shading. Each field falls through
    /// `BasedOn` only when unset at higher levels.
    pub shading: ParagraphShading,
    pub rule_above: ParagraphRule,
    pub rule_below: ParagraphRule,
    pub border: ParagraphBorder,
}

/// Identifies which kind of style is open while we walk
/// `<Properties>` children that carry attributes-as-elements
/// (e.g. `<AppliedFont type="string">…</AppliedFont>`,
/// `<BasedOn type="string">…</BasedOn>`).
#[derive(Debug, Clone, Copy)]
enum CurrentStyle {
    Character,
    Paragraph,
    Object,
    Cell,
    Table,
}

/// Element-form attributes inside `<Properties>` we want to push back
/// into the current style block. Keys are the element name; the
/// next text event lands here.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CurrentProperty {
    AppliedFont,
    BasedOn,
    /// `<NumberingExpression type="string">^#.^t</NumberingExpression>`
    /// inside a `ParagraphStyle`'s `<Properties>` block. Paragraph-only.
    NumberingExpression,
}

impl StyleSheet {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut out = StyleSheet::default();
        let mut buf = Vec::new();
        // Track the open ParagraphStyle's id so nested <TabStop>
        // children attach to the right entry.
        let mut current_paragraph_style: Option<String> = None;
        // Same idea for <CharacterStyle>, used when we read
        // <AppliedFont> as an element inside <Properties>.
        let mut current_character_style: Option<String> = None;
        let mut current_object_style: Option<String> = None;
        let mut current_cell_style: Option<String> = None;
        let mut current_table_style: Option<String> = None;
        // Track an open `<TOCStyle>` so nested `<TOCStyleEntry>` /
        // `<Properties>` text events attach to the right entry. TOC
        // styles aren't part of the cascade-tracking `CurrentStyle`
        // because they don't share the AppliedFont / BasedOn /
        // NumberingExpression property elements the others do.
        let mut current_toc_style: Option<String> = None;
        let mut current_style: Option<CurrentStyle> = None;
        let mut pending_property: Option<CurrentProperty> = None;
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => match e.name().as_ref() {
                    b"CharacterStyle" => {
                        if let Some(s) = parse_character_style(&e) {
                            current_character_style = Some(s.self_id.clone());
                            current_style = Some(CurrentStyle::Character);
                            out.character_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"ParagraphStyle" => {
                        if let Some(s) = parse_paragraph_style(&e) {
                            current_paragraph_style = Some(s.self_id.clone());
                            current_style = Some(CurrentStyle::Paragraph);
                            out.paragraph_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"ObjectStyle" => {
                        if let Some(s) = parse_object_style(&e) {
                            current_object_style = Some(s.self_id.clone());
                            current_style = Some(CurrentStyle::Object);
                            out.object_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"CellStyle" => {
                        if let Some(s) = parse_cell_style(&e) {
                            current_cell_style = Some(s.self_id.clone());
                            current_style = Some(CurrentStyle::Cell);
                            out.cell_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"TableStyle" => {
                        if let Some(s) = parse_table_style(&e) {
                            current_table_style = Some(s.self_id.clone());
                            current_style = Some(CurrentStyle::Table);
                            out.table_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"TOCStyle" => {
                        if let Some(s) = parse_toc_style(&e) {
                            current_toc_style = Some(s.self_id.clone());
                            out.toc_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"DashedStrokeStyle"
                    | b"DottedStrokeStyle"
                    | b"StripedStrokeStyle"
                    | b"WavyStrokeStyle" => {
                        // Real-world IDMLs emit these as self-closing
                        // (handled in the Empty branch) but the schema
                        // permits child `<Properties>`; accept either.
                        if let Some(def) = parse_stroke_style(&e) {
                            out.stroke_styles.insert(def.self_id.clone(), def);
                        }
                    }
                    b"TOCStyleEntry" => {
                        // Element-form `<TOCStyleEntry>...</TOCStyleEntry>`
                        // appears when InDesign attaches `<Properties>`
                        // children. The attributes we care about all live
                        // on the start tag, so reuse the same parser.
                        if let (Some(id), Some(entry)) = (
                            current_toc_style.as_deref(),
                            parse_toc_style_entry(&e),
                        ) {
                            if let Some(t) = out.toc_styles.get_mut(id) {
                                t.entries.push(entry);
                            }
                        }
                    }
                    b"AppliedFont" if current_style.is_some() => {
                        pending_property = Some(CurrentProperty::AppliedFont);
                    }
                    b"BasedOn" if current_style.is_some() => {
                        pending_property = Some(CurrentProperty::BasedOn);
                    }
                    b"NumberingExpression"
                        if matches!(current_style, Some(CurrentStyle::Paragraph)) =>
                    {
                        pending_property = Some(CurrentProperty::NumberingExpression);
                    }
                    _ => {}
                },
                Event::Text(t) if pending_property.is_some() => {
                    let text = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    if text.is_empty() {
                        pending_property = None;
                    } else {
                        match (current_style, pending_property) {
                            (Some(CurrentStyle::Paragraph), Some(prop)) => {
                                if let Some(id) = current_paragraph_style.as_deref() {
                                    if let Some(p) = out.paragraph_styles.get_mut(id) {
                                        match prop {
                                            CurrentProperty::AppliedFont => {
                                                if p.font.is_none() {
                                                    p.font = Some(text);
                                                }
                                            }
                                            CurrentProperty::BasedOn => {
                                                if p.based_on.is_none() {
                                                    p.based_on = Some(text);
                                                }
                                            }
                                            CurrentProperty::NumberingExpression => {
                                                if p.numbering_expression.is_none() {
                                                    p.numbering_expression = Some(text);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            (Some(CurrentStyle::Character), Some(prop)) => {
                                if let Some(id) = current_character_style.as_deref() {
                                    if let Some(c) = out.character_styles.get_mut(id) {
                                        match prop {
                                            CurrentProperty::AppliedFont => {
                                                if c.font.is_none() {
                                                    c.font = Some(text);
                                                }
                                            }
                                            CurrentProperty::BasedOn => {
                                                if c.based_on.is_none() {
                                                    c.based_on = Some(text);
                                                }
                                            }
                                            // NumberingExpression is paragraph-only.
                                            CurrentProperty::NumberingExpression => {}
                                        }
                                    }
                                }
                            }
                            (Some(CurrentStyle::Object), Some(CurrentProperty::BasedOn)) => {
                                if let Some(id) = current_object_style.as_deref() {
                                    if let Some(o) = out.object_styles.get_mut(id) {
                                        if o.based_on.is_none() {
                                            o.based_on = Some(text);
                                        }
                                    }
                                }
                            }
                            (Some(CurrentStyle::Cell), Some(CurrentProperty::BasedOn)) => {
                                if let Some(id) = current_cell_style.as_deref() {
                                    if let Some(c) = out.cell_styles.get_mut(id) {
                                        if c.based_on.is_none() {
                                            c.based_on = Some(text);
                                        }
                                    }
                                }
                            }
                            (Some(CurrentStyle::Table), Some(CurrentProperty::BasedOn)) => {
                                if let Some(id) = current_table_style.as_deref() {
                                    if let Some(t) = out.table_styles.get_mut(id) {
                                        if t.based_on.is_none() {
                                            t.based_on = Some(text);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                        pending_property = None;
                    }
                }
                Event::Empty(e) => match e.name().as_ref() {
                    b"CharacterStyle" => {
                        if let Some(s) = parse_character_style(&e) {
                            out.character_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"ParagraphStyle" => {
                        if let Some(s) = parse_paragraph_style(&e) {
                            out.paragraph_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"TOCStyle" => {
                        // Self-closing `<TOCStyle ... />` — common for
                        // the document's default empty TOCStyle that
                        // carries no entries (real-world IDMLs ship this
                        // even when the document has no TOC).
                        if let Some(s) = parse_toc_style(&e) {
                            out.toc_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"TOCStyleEntry" => {
                        if let (Some(id), Some(entry)) = (
                            current_toc_style.as_deref(),
                            parse_toc_style_entry(&e),
                        ) {
                            if let Some(t) = out.toc_styles.get_mut(id) {
                                t.entries.push(entry);
                            }
                        }
                    }
                    b"TabStop" => {
                        if let (Some(id), Some(stop)) = (
                            current_paragraph_style.as_deref(),
                            parse_tab_stop_styles(&e),
                        ) {
                            if let Some(p) = out.paragraph_styles.get_mut(id) {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    b"DashedStrokeStyle"
                    | b"DottedStrokeStyle"
                    | b"StripedStrokeStyle"
                    | b"WavyStrokeStyle" => {
                        if let Some(def) = parse_stroke_style(&e) {
                            out.stroke_styles.insert(def.self_id.clone(), def);
                        }
                    }
                    b"BulletChar" => {
                        if let (Some(id), Some(cp)) = (
                            current_paragraph_style.as_deref(),
                            attr(&e, b"BulletCharacterValue").and_then(|s| s.parse::<u32>().ok()),
                        ) {
                            if let Some(p) = out.paragraph_styles.get_mut(id) {
                                p.bullet_character = Some(cp);
                            }
                        }
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"ParagraphStyle" => {
                        current_paragraph_style = None;
                        if matches!(current_style, Some(CurrentStyle::Paragraph)) {
                            current_style = None;
                        }
                    }
                    b"CharacterStyle" => {
                        current_character_style = None;
                        if matches!(current_style, Some(CurrentStyle::Character)) {
                            current_style = None;
                        }
                    }
                    b"ObjectStyle" => {
                        current_object_style = None;
                        if matches!(current_style, Some(CurrentStyle::Object)) {
                            current_style = None;
                        }
                    }
                    b"CellStyle" => {
                        current_cell_style = None;
                        if matches!(current_style, Some(CurrentStyle::Cell)) {
                            current_style = None;
                        }
                    }
                    b"TableStyle" => {
                        current_table_style = None;
                        if matches!(current_style, Some(CurrentStyle::Table)) {
                            current_style = None;
                        }
                    }
                    b"TOCStyle" => {
                        current_toc_style = None;
                    }
                    b"AppliedFont" | b"BasedOn" | b"NumberingExpression" => {
                        // Pending property is consumed by the next
                        // Text event; clearing here prevents
                        // mismatched-tag leaks if the element was
                        // empty (no text content).
                        pending_property = None;
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }

    /// Walk a CharacterStyle's `BasedOn` chain, folding each hop's
    /// unset attributes from its parent. Missing or cyclic chains
    /// short-circuit at `MAX_BASED_ON_DEPTH`.
    pub fn resolve_character(&self, id: &str) -> ResolvedCharacter {
        let mut acc = ResolvedCharacter::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.character_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    pub fn resolve_paragraph(&self, id: &str) -> ResolvedParagraph {
        let mut acc = ResolvedParagraph::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.paragraph_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    /// Walk an ObjectStyle's `BasedOn` chain. Same depth-bounded
    /// pattern as `resolve_paragraph` / `resolve_character`.
    pub fn resolve_object(&self, id: &str) -> ResolvedObject {
        let mut acc = ResolvedObject::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.object_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    /// Walk a CellStyle's BasedOn chain. Cell strokes / fills /
    /// vertical justification cascade through it.
    pub fn resolve_cell(&self, id: &str) -> ResolvedCell {
        let mut acc = ResolvedCell::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.cell_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    /// Walk a TableStyle's BasedOn chain. Resolves region →
    /// CellStyle assignments + table border strokes + alternating
    /// row fills.
    pub fn resolve_table(&self, id: &str) -> ResolvedTable {
        let mut acc = ResolvedTable::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.table_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }
}

impl ResolvedObject {
    pub fn merge_below(&mut self, def: &ObjectStyleDef) {
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(def.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_tint = self.stroke_tint.or(def.stroke_tint);
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
        self.corner_radius = self.corner_radius.or(def.corner_radius);
        if self.corner_option.is_none() {
            self.corner_option = def.corner_option.clone();
        }
    }
}

impl ResolvedTable {
    pub fn merge_below(&mut self, def: &TableStyleDef) {
        macro_rules! merge_str {
            ($field:ident) => {
                if self.$field.is_none() {
                    self.$field = def.$field.clone();
                }
            };
        }
        merge_str!(header_region_cell_style);
        merge_str!(body_region_cell_style);
        merge_str!(footer_region_cell_style);
        merge_str!(left_column_region_cell_style);
        merge_str!(right_column_region_cell_style);
        merge_str!(top_border_stroke_color);
        merge_str!(bottom_border_stroke_color);
        merge_str!(left_border_stroke_color);
        merge_str!(right_border_stroke_color);
        merge_str!(start_row_fill_color);
        merge_str!(end_row_fill_color);
        self.top_border_stroke_weight = self
            .top_border_stroke_weight
            .or(def.top_border_stroke_weight);
        self.bottom_border_stroke_weight = self
            .bottom_border_stroke_weight
            .or(def.bottom_border_stroke_weight);
        self.left_border_stroke_weight = self
            .left_border_stroke_weight
            .or(def.left_border_stroke_weight);
        self.right_border_stroke_weight = self
            .right_border_stroke_weight
            .or(def.right_border_stroke_weight);
        self.start_row_fill_count = self.start_row_fill_count.or(def.start_row_fill_count);
        self.start_row_fill_tint = self.start_row_fill_tint.or(def.start_row_fill_tint);
        self.end_row_fill_count = self.end_row_fill_count.or(def.end_row_fill_count);
        self.end_row_fill_tint = self.end_row_fill_tint.or(def.end_row_fill_tint);
    }
}

impl ResolvedCell {
    pub fn merge_below(&mut self, def: &CellStyleDef) {
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        if self.vertical_justification.is_none() {
            self.vertical_justification = def.vertical_justification.clone();
        }
        if self.top_edge_stroke_color.is_none() {
            self.top_edge_stroke_color = def.top_edge_stroke_color.clone();
        }
        self.top_edge_stroke_weight = self.top_edge_stroke_weight.or(def.top_edge_stroke_weight);
        if self.bottom_edge_stroke_color.is_none() {
            self.bottom_edge_stroke_color = def.bottom_edge_stroke_color.clone();
        }
        self.bottom_edge_stroke_weight = self
            .bottom_edge_stroke_weight
            .or(def.bottom_edge_stroke_weight);
        if self.left_edge_stroke_color.is_none() {
            self.left_edge_stroke_color = def.left_edge_stroke_color.clone();
        }
        self.left_edge_stroke_weight = self.left_edge_stroke_weight.or(def.left_edge_stroke_weight);
        if self.right_edge_stroke_color.is_none() {
            self.right_edge_stroke_color = def.right_edge_stroke_color.clone();
        }
        self.right_edge_stroke_weight = self
            .right_edge_stroke_weight
            .or(def.right_edge_stroke_weight);
    }
}

impl ResolvedCharacter {
    /// Fill any unset (`None`) field from `def`. Cascade convention:
    /// already-set fields on `self` win; `def` only patches gaps.
    pub fn merge_below(&mut self, def: &CharacterStyleDef) {
        if self.font.is_none() {
            self.font = def.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = def.font_style.clone();
        }
        self.point_size = self.point_size.or(def.point_size);
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(def.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = def.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(def.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(def.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(def.vertical_scale);
        self.skew = self.skew.or(def.skew);
        if self.position.is_none() {
            self.position = def.position.clone();
        }
        self.tracking = self.tracking.or(def.tracking);
        self.underline = self.underline.or(def.underline);
        self.strikethru = self.strikethru.or(def.strikethru);
        self.overprint_fill = self.overprint_fill.or(def.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(def.overprint_stroke);
        self.ruby_flag = self.ruby_flag.or(def.ruby_flag);
        if self.ruby_type.is_none() {
            self.ruby_type = def.ruby_type.clone();
        }
        if self.ruby_string.is_none() {
            self.ruby_string = def.ruby_string.clone();
        }
        if self.kenten_kind.is_none() {
            self.kenten_kind = def.kenten_kind.clone();
        }
        if self.kenten_character.is_none() {
            self.kenten_character = def.kenten_character.clone();
        }
        self.kenten_font_size = self.kenten_font_size.or(def.kenten_font_size);
    }
}

impl ResolvedParagraph {
    /// Fill any unset field from `def` (BasedOn cascade). For
    /// `tab_list` "unset" means empty — IDML has no
    /// distinction between "no tabs" and "tab list inherited".
    pub fn merge_below(&mut self, def: &ParagraphStyleDef) {
        if self.font.is_none() {
            self.font = def.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = def.font_style.clone();
        }
        self.point_size = self.point_size.or(def.point_size);
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(def.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = def.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(def.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(def.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(def.vertical_scale);
        self.skew = self.skew.or(def.skew);
        if self.position.is_none() {
            self.position = def.position.clone();
        }
        self.tracking = self.tracking.or(def.tracking);
        self.justification = self.justification.or(def.justification);
        self.first_line_indent = self.first_line_indent.or(def.first_line_indent);
        self.space_before = self.space_before.or(def.space_before);
        self.space_after = self.space_after.or(def.space_after);
        self.underline = self.underline.or(def.underline);
        self.strikethru = self.strikethru.or(def.strikethru);
        if self.tab_list.is_empty() && !def.tab_list.is_empty() {
            self.tab_list = def.tab_list.clone();
        }
        if self.bullets_list_type.is_none() {
            self.bullets_list_type = def.bullets_list_type.clone();
        }
        self.bullet_character = self.bullet_character.or(def.bullet_character);
        if self.bullets_text_after.is_none() {
            self.bullets_text_after = def.bullets_text_after.clone();
        }
        if self.numbering_format.is_none() {
            self.numbering_format = def.numbering_format.clone();
        }
        if self.bullets_character_style.is_none() {
            self.bullets_character_style = def.bullets_character_style.clone();
        }
        if self.bullets_and_numbering_digits_character_style.is_none() {
            self.bullets_and_numbering_digits_character_style =
                def.bullets_and_numbering_digits_character_style.clone();
        }
        if self.numbering_expression.is_none() {
            self.numbering_expression = def.numbering_expression.clone();
        }
        self.numbering_start_at = self.numbering_start_at.or(def.numbering_start_at);
        self.numbering_continue = self.numbering_continue.or(def.numbering_continue);
        self.hyphenation = self.hyphenation.or(def.hyphenation);
        if self.applied_language.is_none() {
            self.applied_language = def.applied_language.clone();
        }
        self.minimum_word_spacing = self.minimum_word_spacing.or(def.minimum_word_spacing);
        self.desired_word_spacing = self.desired_word_spacing.or(def.desired_word_spacing);
        self.maximum_word_spacing = self.maximum_word_spacing.or(def.maximum_word_spacing);
        // Q-20: letter / glyph spacing per-field inheritance.
        self.minimum_letter_spacing =
            self.minimum_letter_spacing.or(def.minimum_letter_spacing);
        self.desired_letter_spacing =
            self.desired_letter_spacing.or(def.desired_letter_spacing);
        self.maximum_letter_spacing =
            self.maximum_letter_spacing.or(def.maximum_letter_spacing);
        self.minimum_glyph_scaling =
            self.minimum_glyph_scaling.or(def.minimum_glyph_scaling);
        self.desired_glyph_scaling =
            self.desired_glyph_scaling.or(def.desired_glyph_scaling);
        self.maximum_glyph_scaling =
            self.maximum_glyph_scaling.or(def.maximum_glyph_scaling);
        self.drop_cap_characters = self.drop_cap_characters.or(def.drop_cap_characters);
        self.drop_cap_lines = self.drop_cap_lines.or(def.drop_cap_lines);
        self.drop_cap_detail = self.drop_cap_detail.or(def.drop_cap_detail);
        self.overprint_fill = self.overprint_fill.or(def.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(def.overprint_stroke);
        if self.kinsoku_set.is_none() {
            self.kinsoku_set = def.kinsoku_set.clone();
        }
        if self.kinsoku_type.is_none() {
            self.kinsoku_type = def.kinsoku_type.clone();
        }
        if self.mojikumi_table.is_none() {
            self.mojikumi_table = def.mojikumi_table.clone();
        }
        if self.mojikumi_set.is_none() {
            self.mojikumi_set = def.mojikumi_set.clone();
        }
        // Q-09: per-field shading inheritance. Each Option survives
        // the cascade independently so a child can override `tint`
        // without dragging in the parent's `width`, etc.
        let s = &mut self.shading;
        let p = &def.shading;
        s.on = s.on.or(p.on);
        if s.color.is_none() {
            s.color = p.color.clone();
        }
        s.tint = s.tint.or(p.tint);
        if s.width.is_none() {
            s.width = p.width.clone();
        }
        s.offset_top = s.offset_top.or(p.offset_top);
        s.offset_left = s.offset_left.or(p.offset_left);
        s.offset_bottom = s.offset_bottom.or(p.offset_bottom);
        s.offset_right = s.offset_right.or(p.offset_right);
        if s.top_origin.is_none() {
            s.top_origin = p.top_origin.clone();
        }
        if s.bottom_origin.is_none() {
            s.bottom_origin = p.bottom_origin.clone();
        }
        s.clip_to_frame = s.clip_to_frame.or(p.clip_to_frame);
        s.overprint = s.overprint.or(p.overprint);
        s.suppress_printing = s.suppress_printing.or(p.suppress_printing);
        // Q-09: per-field rule_above / rule_below inheritance.
        merge_rule(&mut self.rule_above, &def.rule_above);
        merge_rule(&mut self.rule_below, &def.rule_below);
        // Q-09: per-field border inheritance.
        merge_border(&mut self.border, &def.border);
    }
}

fn merge_rule(child: &mut ParagraphRule, parent: &ParagraphRule) {
    child.on = child.on.or(parent.on);
    if child.color.is_none() {
        child.color = parent.color.clone();
    }
    child.tint = child.tint.or(parent.tint);
    child.weight = child.weight.or(parent.weight);
    child.offset = child.offset.or(parent.offset);
    child.left_indent = child.left_indent.or(parent.left_indent);
    child.right_indent = child.right_indent.or(parent.right_indent);
    if child.width.is_none() {
        child.width = parent.width.clone();
    }
}

fn merge_border(child: &mut ParagraphBorder, parent: &ParagraphBorder) {
    child.on = child.on.or(parent.on);
    if child.color.is_none() {
        child.color = parent.color.clone();
    }
    child.tint = child.tint.or(parent.tint);
    child.weight = child.weight.or(parent.weight);
    child.offset_top = child.offset_top.or(parent.offset_top);
    child.offset_left = child.offset_left.or(parent.offset_left);
    child.offset_bottom = child.offset_bottom.or(parent.offset_bottom);
    child.offset_right = child.offset_right.or(parent.offset_right);
    if child.width.is_none() {
        child.width = parent.width.clone();
    }
    for i in 0..4 {
        child.corners[i].option = child.corners[i].option.or(parent.corners[i].option);
        child.corners[i].radius = child.corners[i].radius.or(parent.corners[i].radius);
    }
}

fn parse_character_style(e: &quick_xml::events::BytesStart) -> Option<CharacterStyleDef> {
    // `Swatch/None` is IDML's literal for "no stroke" — normalise to
    // None so a `BasedOn` cascade can fall through to a real colour.
    let normalize = |c: Option<String>| match c.as_deref() {
        Some("Swatch/None") | Some("n") | Some("") => None,
        _ => c,
    };
    Some(CharacterStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        font: attr(e, b"AppliedFont"),
        font_style: attr(e, b"FontStyle"),
        point_size: attr(e, b"PointSize").and_then(|s| s.parse().ok()),
        fill_color: attr(e, b"FillColor"),
        fill_tint: parse_tint_attr(e, b"FillTint"),
        stroke_color: normalize(attr(e, b"StrokeColor")),
        stroke_weight: attr(e, b"StrokeWeight").and_then(|s| s.parse().ok()),
        capitalization: attr(e, b"Capitalization"),
        baseline_shift: attr(e, b"BaselineShift").and_then(|s| s.parse().ok()),
        horizontal_scale: attr(e, b"HorizontalScale").and_then(|s| s.parse().ok()),
        vertical_scale: attr(e, b"VerticalScale").and_then(|s| s.parse().ok()),
        skew: attr(e, b"Skew").and_then(|s| s.parse().ok()),
        position: attr(e, b"Position"),
        tracking: attr(e, b"Tracking").and_then(|s| s.parse().ok()),
        underline: attr(e, b"Underline").and_then(|s| s.parse().ok()),
        strikethru: attr(e, b"StrikeThru").and_then(|s| s.parse().ok()),
        overprint_fill: attr(e, b"OverprintFill").and_then(|s| s.parse().ok()),
        overprint_stroke: attr(e, b"OverprintStroke").and_then(|s| s.parse().ok()),
        ruby_flag: attr(e, b"RubyFlag").and_then(|s| s.parse().ok()),
        ruby_type: attr(e, b"RubyType"),
        ruby_string: attr(e, b"RubyString"),
        kenten_kind: attr(e, b"KentenKind"),
        kenten_character: attr(e, b"KentenCharacter"),
        kenten_font_size: attr(e, b"KentenFontSize").and_then(|s| s.parse().ok()),
    })
}

fn parse_tab_stop_styles(e: &quick_xml::events::BytesStart) -> Option<TabStop> {
    let position = attr(e, b"Position").and_then(|s| s.parse::<f32>().ok())?;
    Some(TabStop {
        position,
        alignment: attr(e, b"Alignment"),
        alignment_character: attr(e, b"AlignmentCharacter"),
        leader: attr(e, b"Leader"),
    })
}

fn parse_table_style(e: &quick_xml::events::BytesStart) -> Option<TableStyleDef> {
    let self_id = attr(e, b"Self")?;
    let normalize = |c: Option<String>| match c.as_deref() {
        Some("Swatch/None") | Some("n") | Some("") => None,
        _ => c,
    };
    Some(TableStyleDef {
        self_id,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        header_region_cell_style: normalize(attr(e, b"HeaderRegionCellStyle")),
        body_region_cell_style: normalize(attr(e, b"BodyRegionCellStyle")),
        footer_region_cell_style: normalize(attr(e, b"FooterRegionCellStyle")),
        left_column_region_cell_style: normalize(attr(e, b"LeftColumnRegionCellStyle")),
        right_column_region_cell_style: normalize(attr(e, b"RightColumnRegionCellStyle")),
        top_border_stroke_color: normalize(attr(e, b"TopBorderStrokeColor")),
        top_border_stroke_weight: attr(e, b"TopBorderStrokeWeight").and_then(|s| s.parse().ok()),
        bottom_border_stroke_color: normalize(attr(e, b"BottomBorderStrokeColor")),
        bottom_border_stroke_weight: attr(e, b"BottomBorderStrokeWeight")
            .and_then(|s| s.parse().ok()),
        left_border_stroke_color: normalize(attr(e, b"LeftBorderStrokeColor")),
        left_border_stroke_weight: attr(e, b"LeftBorderStrokeWeight").and_then(|s| s.parse().ok()),
        right_border_stroke_color: normalize(attr(e, b"RightBorderStrokeColor")),
        right_border_stroke_weight: attr(e, b"RightBorderStrokeWeight")
            .and_then(|s| s.parse().ok()),
        start_row_fill_color: normalize(attr(e, b"StartRowFillColor")),
        start_row_fill_count: attr(e, b"StartRowFillCount").and_then(|s| s.parse().ok()),
        start_row_fill_tint: parse_tint_attr(e, b"StartRowFillTint"),
        end_row_fill_color: normalize(attr(e, b"EndRowFillColor")),
        end_row_fill_count: attr(e, b"EndRowFillCount").and_then(|s| s.parse().ok()),
        end_row_fill_tint: parse_tint_attr(e, b"EndRowFillTint"),
    })
}

fn parse_cell_style(e: &quick_xml::events::BytesStart) -> Option<CellStyleDef> {
    let self_id = attr(e, b"Self")?;
    let normalize = |c: Option<String>| match c.as_deref() {
        Some("Swatch/None") | Some("n") | Some("") => None,
        _ => c,
    };
    Some(CellStyleDef {
        self_id,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        fill_color: normalize(attr(e, b"FillColor")),
        vertical_justification: attr(e, b"VerticalJustification"),
        top_edge_stroke_color: normalize(attr(e, b"TopEdgeStrokeColor")),
        top_edge_stroke_weight: attr(e, b"TopEdgeStrokeWeight").and_then(|s| s.parse().ok()),
        bottom_edge_stroke_color: normalize(attr(e, b"BottomEdgeStrokeColor")),
        bottom_edge_stroke_weight: attr(e, b"BottomEdgeStrokeWeight").and_then(|s| s.parse().ok()),
        left_edge_stroke_color: normalize(attr(e, b"LeftEdgeStrokeColor")),
        left_edge_stroke_weight: attr(e, b"LeftEdgeStrokeWeight").and_then(|s| s.parse().ok()),
        right_edge_stroke_color: normalize(attr(e, b"RightEdgeStrokeColor")),
        right_edge_stroke_weight: attr(e, b"RightEdgeStrokeWeight").and_then(|s| s.parse().ok()),
    })
}

fn parse_object_style(e: &quick_xml::events::BytesStart) -> Option<ObjectStyleDef> {
    let self_id = attr(e, b"Self")?;
    let stroke_weight = attr(e, b"StrokeWeight").and_then(|s| s.parse().ok());
    // IDML stores "no stroke" as the literal "Swatch/None"; treat
    // that as missing so the cascade can fall through to a real
    // colour from BasedOn rather than handing the renderer a paint
    // it can't resolve.
    let normalize = |c: Option<String>| match c.as_deref() {
        Some("Swatch/None") | Some("n") | Some("") => None,
        _ => c,
    };
    Some(ObjectStyleDef {
        self_id,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        fill_color: normalize(attr(e, b"FillColor")),
        fill_tint: parse_tint_attr(e, b"FillTint"),
        stroke_color: normalize(attr(e, b"StrokeColor")),
        stroke_tint: parse_tint_attr(e, b"StrokeTint"),
        stroke_weight,
        corner_radius: attr(e, b"CornerRadius").and_then(|s| s.parse().ok()),
        corner_option: attr(e, b"CornerOption"),
    })
}

fn parse_toc_style(e: &quick_xml::events::BytesStart) -> Option<TOCStyleDef> {
    Some(TOCStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        title: attr(e, b"Title"),
        title_style: attr(e, b"TitleStyle"),
        include_book_documents: attr(e, b"IncludeBookDocuments").and_then(|s| s.parse().ok()),
        include_hidden: attr(e, b"IncludeHidden").and_then(|s| s.parse().ok()),
        run_in: attr(e, b"RunIn").and_then(|s| s.parse().ok()),
        entries: Vec::new(),
    })
}

fn parse_toc_style_entry(e: &quick_xml::events::BytesStart) -> Option<TOCStyleEntryDef> {
    Some(TOCStyleEntryDef {
        name: attr(e, b"Name"),
        include_style: attr(e, b"IncludeStyle"),
        format_style: attr(e, b"FormatStyle"),
        level: attr(e, b"Level").and_then(|s| s.parse().ok()),
        page_number: attr(e, b"PageNumber"),
        separator: attr(e, b"Separator"),
    })
}

/// Track 4a: parse a `<DashedStrokeStyle>` / `<DottedStrokeStyle>` /
/// `<StripedStrokeStyle>` / `<WavyStrokeStyle>` element. Pulls the
/// `Self` id and (for dashed) the `Pattern` attribute as a list of
/// on/off lengths in pt. Returns `None` only when `Self` is missing
/// — unrecognised element shapes are still useful to remember.
fn parse_stroke_style(e: &quick_xml::events::BytesStart) -> Option<StrokeStyleDef> {
    let self_id = attr(e, b"Self")?;
    let kind = match e.name().as_ref() {
        b"DashedStrokeStyle" => StrokeStyleKind::Dashed,
        b"DottedStrokeStyle" => StrokeStyleKind::Dotted,
        b"StripedStrokeStyle" => StrokeStyleKind::Striped,
        b"WavyStrokeStyle" => StrokeStyleKind::Wavy,
        _ => return None,
    };
    let pattern = attr(e, b"Pattern")
        .map(|s| {
            s.split_ascii_whitespace()
                .filter_map(|tok| tok.parse::<f32>().ok())
                .collect()
        })
        .unwrap_or_default();
    Some(StrokeStyleDef {
        self_id,
        name: attr(e, b"Name"),
        kind,
        pattern,
    })
}

fn parse_paragraph_style(e: &quick_xml::events::BytesStart) -> Option<ParagraphStyleDef> {
    // `Swatch/None` is IDML's literal for "no stroke" — normalise to
    // None so a `BasedOn` cascade can fall through to a real colour.
    let normalize = |c: Option<String>| match c.as_deref() {
        Some("Swatch/None") | Some("n") | Some("") => None,
        _ => c,
    };
    Some(ParagraphStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        font: attr(e, b"AppliedFont"),
        font_style: attr(e, b"FontStyle"),
        point_size: attr(e, b"PointSize").and_then(|s| s.parse().ok()),
        fill_color: attr(e, b"FillColor"),
        fill_tint: parse_tint_attr(e, b"FillTint"),
        stroke_color: normalize(attr(e, b"StrokeColor")),
        stroke_weight: attr(e, b"StrokeWeight").and_then(|s| s.parse().ok()),
        capitalization: attr(e, b"Capitalization"),
        baseline_shift: attr(e, b"BaselineShift").and_then(|s| s.parse().ok()),
        horizontal_scale: attr(e, b"HorizontalScale").and_then(|s| s.parse().ok()),
        vertical_scale: attr(e, b"VerticalScale").and_then(|s| s.parse().ok()),
        skew: attr(e, b"Skew").and_then(|s| s.parse().ok()),
        position: attr(e, b"Position"),
        tracking: attr(e, b"Tracking").and_then(|s| s.parse().ok()),
        justification: attr(e, b"Justification")
            .as_deref()
            .and_then(Justification::from_idml),
        first_line_indent: attr(e, b"FirstLineIndent").and_then(|s| s.parse().ok()),
        space_before: attr(e, b"SpaceBefore").and_then(|s| s.parse().ok()),
        space_after: attr(e, b"SpaceAfter").and_then(|s| s.parse().ok()),
        underline: attr(e, b"Underline").and_then(|s| s.parse().ok()),
        strikethru: attr(e, b"StrikeThru").and_then(|s| s.parse().ok()),
        tab_list: Vec::new(),
        bullets_list_type: attr(e, b"BulletsAndNumberingListType"),
        bullet_character: None,
        bullets_text_after: attr(e, b"BulletsTextAfter"),
        numbering_format: attr(e, b"NumberingFormat"),
        bullets_character_style: attr(e, b"BulletsCharacterStyle"),
        bullets_and_numbering_digits_character_style: attr(
            e,
            b"BulletsAndNumberingDigitsCharacterStyle",
        ),
        numbering_expression: attr(e, b"NumberingExpression"),
        numbering_start_at: attr(e, b"NumberingStartAt").and_then(|s| s.parse().ok()),
        numbering_continue: attr(e, b"NumberingContinue").and_then(|s| s.parse().ok()),
        hyphenation: attr(e, b"Hyphenation").and_then(|s| s.parse().ok()),
        applied_language: attr(e, b"AppliedLanguage"),
        minimum_word_spacing: attr(e, b"MinimumWordSpacing").and_then(|s| s.parse().ok()),
        desired_word_spacing: attr(e, b"DesiredWordSpacing").and_then(|s| s.parse().ok()),
        maximum_word_spacing: attr(e, b"MaximumWordSpacing").and_then(|s| s.parse().ok()),
        minimum_letter_spacing: attr(e, b"MinimumLetterSpacing").and_then(|s| s.parse().ok()),
        desired_letter_spacing: attr(e, b"DesiredLetterSpacing").and_then(|s| s.parse().ok()),
        maximum_letter_spacing: attr(e, b"MaximumLetterSpacing").and_then(|s| s.parse().ok()),
        minimum_glyph_scaling: attr(e, b"MinimumGlyphScaling").and_then(|s| s.parse().ok()),
        desired_glyph_scaling: attr(e, b"DesiredGlyphScaling").and_then(|s| s.parse().ok()),
        maximum_glyph_scaling: attr(e, b"MaximumGlyphScaling").and_then(|s| s.parse().ok()),
        drop_cap_characters: attr(e, b"DropCapCharacters").and_then(|s| s.parse().ok()),
        drop_cap_lines: attr(e, b"DropCapLines").and_then(|s| s.parse().ok()),
        drop_cap_detail: attr(e, b"DropCapDetail").and_then(|s| s.parse().ok()),
        overprint_fill: attr(e, b"OverprintFill").and_then(|s| s.parse().ok()),
        overprint_stroke: attr(e, b"OverprintStroke").and_then(|s| s.parse().ok()),
        kinsoku_set: attr(e, b"KinsokuSet"),
        kinsoku_type: attr(e, b"KinsokuType"),
        mojikumi_table: attr(e, b"MojikumiTable"),
        mojikumi_set: attr(e, b"MojikumiSet"),
        shading: ParagraphShading::from_attrs(e),
        rule_above: ParagraphRule::from_attrs(e, "RuleAbove"),
        rule_below: ParagraphRule::from_attrs(e, "RuleBelow"),
        border: ParagraphBorder::from_attrs(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootCharacterStyleGroup>
    <CharacterStyle Self="CharacterStyle/Base"
                    Name="Base"
                    AppliedFont="Body Font"
                    PointSize="11"
                    FillColor="Color/Black"/>
    <CharacterStyle Self="CharacterStyle/Bold"
                    Name="Bold"
                    BasedOn="CharacterStyle/Base"
                    FontStyle="Bold"/>
  </RootCharacterStyleGroup>
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Body"
                    Name="Body"
                    AppliedFont="Body Font"
                    PointSize="11"
                    Justification="LeftAlign"
                    SpaceAfter="6"/>
    <ParagraphStyle Self="ParagraphStyle/Heading"
                    Name="Heading"
                    BasedOn="ParagraphStyle/Body"
                    PointSize="22"
                    FontStyle="Bold"/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#;

    #[test]
    fn parses_styles_table() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        assert_eq!(s.character_styles.len(), 2);
        assert_eq!(s.paragraph_styles.len(), 2);
        let bold = s.character_styles.get("CharacterStyle/Bold").unwrap();
        assert_eq!(bold.based_on.as_deref(), Some("CharacterStyle/Base"));
        assert_eq!(bold.font_style.as_deref(), Some("Bold"));
    }

    #[test]
    fn resolve_character_walks_based_on_chain() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        let r = s.resolve_character("CharacterStyle/Bold");
        // FontStyle from Bold itself; AppliedFont + PointSize +
        // FillColor inherited from Base.
        assert_eq!(r.font_style.as_deref(), Some("Bold"));
        assert_eq!(r.font.as_deref(), Some("Body Font"));
        assert_eq!(r.point_size, Some(11.0));
        assert_eq!(r.fill_color.as_deref(), Some("Color/Black"));
    }

    #[test]
    fn resolve_paragraph_walks_based_on_chain() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Heading");
        assert_eq!(r.point_size, Some(22.0)); // override
        assert_eq!(r.font.as_deref(), Some("Body Font")); // inherited
        assert_eq!(r.justification, Some(Justification::LeftAlign));
        assert_eq!(r.space_after, Some(6.0));
    }

    #[test]
    fn q09_parses_paragraph_shading_attrs() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Banner"
                            ParagraphShadingOn="true"
                            ParagraphShadingColor="Color/Brand"
                            ParagraphShadingTint="20"
                            ParagraphShadingWidth="ColumnWidth"
                            ParagraphShadingTopOffset="2"
                            ParagraphShadingBottomOffset="2"
                            ParagraphShadingLeftOffset="6"
                            ParagraphShadingRightOffset="6"
                            ParagraphShadingTopOrigin="AscentTopOrigin"
                            ParagraphShadingClipToFrame="false"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Banner").unwrap();
        let sh = &p.shading;
        assert_eq!(sh.on, Some(true));
        assert_eq!(sh.color.as_deref(), Some("Color/Brand"));
        assert_eq!(sh.tint, Some(20.0));
        assert_eq!(sh.width.as_deref(), Some("ColumnWidth"));
        assert_eq!(sh.offset_top, Some(2.0));
        assert_eq!(sh.offset_bottom, Some(2.0));
        assert_eq!(sh.offset_left, Some(6.0));
        assert_eq!(sh.offset_right, Some(6.0));
        assert_eq!(sh.top_origin.as_deref(), Some("AscentTopOrigin"));
        assert_eq!(sh.clip_to_frame, Some(false));
    }

    #[test]
    fn q09_paragraph_shading_inherits_from_based_on() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Base"
                            ParagraphShadingOn="true"
                            ParagraphShadingColor="Color/Brand"
                            ParagraphShadingTint="50"/>
            <ParagraphStyle Self="ParagraphStyle/Child"
                            BasedOn="ParagraphStyle/Base"
                            ParagraphShadingTint="20"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Child");
        // tint overridden, color + on inherited.
        assert_eq!(r.shading.on, Some(true));
        assert_eq!(r.shading.color.as_deref(), Some("Color/Brand"));
        assert_eq!(r.shading.tint, Some(20.0));
    }

    #[test]
    fn q09_parses_paragraph_border_attrs() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Boxed"
                            ParagraphBorderOn="true"
                            ParagraphBorderColor="Color/Brand"
                            ParagraphBorderTint="40"
                            ParagraphBorderWeight="1"
                            ParagraphBorderTopOffset="2"
                            ParagraphBorderBottomOffset="3"
                            ParagraphBorderLeftOffset="4"
                            ParagraphBorderRightOffset="5"
                            ParagraphBorderWidth="ColumnWidth"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Boxed").unwrap();
        let b = &p.border;
        assert_eq!(b.on, Some(true));
        assert_eq!(b.color.as_deref(), Some("Color/Brand"));
        assert_eq!(b.tint, Some(40.0));
        assert_eq!(b.weight, Some(1.0));
        assert_eq!(b.offset_top, Some(2.0));
        assert_eq!(b.offset_bottom, Some(3.0));
        assert_eq!(b.offset_left, Some(4.0));
        assert_eq!(b.offset_right, Some(5.0));
        assert_eq!(b.width.as_deref(), Some("ColumnWidth"));
    }

    #[test]
    fn q09_paragraph_border_inherits_from_based_on() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Base"
                            ParagraphBorderOn="true"
                            ParagraphBorderColor="Color/Brand"
                            ParagraphBorderWeight="2"/>
            <ParagraphStyle Self="ParagraphStyle/Child"
                            BasedOn="ParagraphStyle/Base"
                            ParagraphBorderWeight="1"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Child");
        // weight overridden, color + on inherited.
        assert_eq!(r.border.on, Some(true));
        assert_eq!(r.border.color.as_deref(), Some("Color/Brand"));
        assert_eq!(r.border.weight, Some(1.0));
    }

    #[test]
    fn q09_paragraph_border_per_corner_attrs_round_trip() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Boxed"
                            ParagraphBorderOn="true"
                            ParagraphBorderTopLeftCornerOption="RoundedCorner"
                            ParagraphBorderTopLeftCornerRadius="6"
                            ParagraphBorderTopRightCornerOption="RoundedCorner"
                            ParagraphBorderTopRightCornerRadius="7"
                            ParagraphBorderBottomRightCornerOption="None"
                            ParagraphBorderBottomRightCornerRadius="0"
                            ParagraphBorderBottomLeftCornerOption="RoundedCorner"
                            ParagraphBorderBottomLeftCornerRadius="9"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Boxed").unwrap();
        let c = &p.border.corners;
        assert_eq!(c[0].radius, Some(6.0));
        assert_eq!(c[0].option, Some(crate::CornerOption::Rounded));
        assert_eq!(c[1].radius, Some(7.0));
        assert_eq!(c[2].radius, Some(0.0));
        assert_eq!(c[2].option, Some(crate::CornerOption::None));
        assert_eq!(c[3].radius, Some(9.0));
        assert_eq!(c[3].option, Some(crate::CornerOption::Rounded));
    }

    #[test]
    fn q09_paragraph_border_corner_inherits_from_based_on() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Base"
                            ParagraphBorderOn="true"
                            ParagraphBorderTopLeftCornerOption="RoundedCorner"
                            ParagraphBorderTopLeftCornerRadius="5"
                            ParagraphBorderTopRightCornerOption="RoundedCorner"
                            ParagraphBorderTopRightCornerRadius="5"/>
            <ParagraphStyle Self="ParagraphStyle/Child"
                            BasedOn="ParagraphStyle/Base"
                            ParagraphBorderTopRightCornerRadius="8"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Child");
        // top-left inherited fully; top-right radius overridden but
        // option still inherited from parent.
        assert_eq!(r.border.corners[0].radius, Some(5.0));
        assert_eq!(r.border.corners[0].option, Some(crate::CornerOption::Rounded));
        assert_eq!(r.border.corners[1].radius, Some(8.0));
        assert_eq!(r.border.corners[1].option, Some(crate::CornerOption::Rounded));
    }

    #[test]
    fn q20_parses_letter_glyph_spacing_attrs() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Tight"
                            MinimumLetterSpacing="-5"
                            DesiredLetterSpacing="0"
                            MaximumLetterSpacing="10"
                            MinimumGlyphScaling="95"
                            DesiredGlyphScaling="100"
                            MaximumGlyphScaling="105"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Tight").unwrap();
        assert_eq!(p.minimum_letter_spacing, Some(-5.0));
        assert_eq!(p.desired_letter_spacing, Some(0.0));
        assert_eq!(p.maximum_letter_spacing, Some(10.0));
        assert_eq!(p.minimum_glyph_scaling, Some(95.0));
        assert_eq!(p.desired_glyph_scaling, Some(100.0));
        assert_eq!(p.maximum_glyph_scaling, Some(105.0));
    }

    #[test]
    fn q20_letter_glyph_spacing_inherits_from_based_on() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Base"
                            MinimumLetterSpacing="-3"
                            MaximumLetterSpacing="8"
                            MinimumGlyphScaling="97"
                            MaximumGlyphScaling="103"/>
            <ParagraphStyle Self="ParagraphStyle/Child"
                            BasedOn="ParagraphStyle/Base"
                            MaximumLetterSpacing="15"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Child");
        assert_eq!(r.minimum_letter_spacing, Some(-3.0));   // inherited
        assert_eq!(r.maximum_letter_spacing, Some(15.0));   // overridden
        assert_eq!(r.minimum_glyph_scaling, Some(97.0));    // inherited
        assert_eq!(r.maximum_glyph_scaling, Some(103.0));   // inherited
    }

    #[test]
    fn parses_bullets_on_paragraph_style() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Bulleted"
                            BulletsAndNumberingListType="BulletList"
                            BulletsTextAfter=" ">
              <Properties>
                <BulletChar BulletCharacterValue="8226"/>
              </Properties>
            </ParagraphStyle>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Bulleted").unwrap();
        assert_eq!(p.bullets_list_type.as_deref(), Some("BulletList"));
        assert_eq!(p.bullet_character, Some(8226)); // U+2022 BULLET
        assert_eq!(p.bullets_text_after.as_deref(), Some(" "));
    }

    #[test]
    fn parses_bullets_character_style_attrs() {
        // Both `BulletsCharacterStyle` (bullets) and
        // `BulletsAndNumberingDigitsCharacterStyle` (numbered-list
        // digits) survive the parser as plain string refs.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Bulleted"
                            BulletsAndNumberingListType="BulletList"
                            BulletsCharacterStyle="CharacterStyle/RedDot"/>
            <ParagraphStyle Self="ParagraphStyle/Numbered"
                            BulletsAndNumberingListType="NumberedList"
                            BulletsAndNumberingDigitsCharacterStyle="CharacterStyle/BlueDigit"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let b = s.paragraph_styles.get("ParagraphStyle/Bulleted").unwrap();
        assert_eq!(
            b.bullets_character_style.as_deref(),
            Some("CharacterStyle/RedDot")
        );
        assert!(b.bullets_and_numbering_digits_character_style.is_none());
        let n = s.paragraph_styles.get("ParagraphStyle/Numbered").unwrap();
        assert_eq!(
            n.bullets_and_numbering_digits_character_style.as_deref(),
            Some("CharacterStyle/BlueDigit")
        );
        assert!(n.bullets_character_style.is_none());
    }

    #[test]
    fn resolve_paragraph_propagates_bullets_character_style_through_based_on() {
        // A child style without its own BulletsCharacterStyle should
        // inherit it via BasedOn so cascade-only IDMLs continue
        // working.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Base"
                            BulletsAndNumberingListType="BulletList"
                            BulletsCharacterStyle="CharacterStyle/RedDot"
                            BulletsAndNumberingDigitsCharacterStyle="CharacterStyle/BlueDigit"/>
            <ParagraphStyle Self="ParagraphStyle/Child"
                            BasedOn="ParagraphStyle/Base"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Child");
        assert_eq!(
            r.bullets_character_style.as_deref(),
            Some("CharacterStyle/RedDot")
        );
        assert_eq!(
            r.bullets_and_numbering_digits_character_style.as_deref(),
            Some("CharacterStyle/BlueDigit")
        );
    }

    #[test]
    fn resolve_unknown_id_returns_default() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        let r = s.resolve_character("CharacterStyle/Missing");
        assert!(r.font.is_none());
        assert!(r.point_size.is_none());
    }

    #[test]
    fn resolve_terminates_on_cyclic_based_on() {
        // Two styles BasedOn each other — resolution must not hang.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootCharacterStyleGroup>
            <CharacterStyle Self="CharacterStyle/A" BasedOn="CharacterStyle/B" PointSize="10"/>
            <CharacterStyle Self="CharacterStyle/B" BasedOn="CharacterStyle/A" FontStyle="Bold"/>
          </RootCharacterStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_character("CharacterStyle/A");
        // Both were folded in once; the depth limiter prevents looping.
        assert_eq!(r.point_size, Some(10.0));
        assert_eq!(r.font_style.as_deref(), Some("Bold"));
    }

    #[test]
    fn parses_numbering_expression_start_at_and_continue_attrs() {
        // Real-world IDML carries these as attributes on the
        // ParagraphStyle start tag for the simple cases.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Numbered"
                            BulletsAndNumberingListType="NumberedList"
                            NumberingExpression="Step ^# of 5^t"
                            NumberingStartAt="5"
                            NumberingContinue="false"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Numbered").unwrap();
        assert_eq!(p.numbering_expression.as_deref(), Some("Step ^# of 5^t"));
        assert_eq!(p.numbering_start_at, Some(5));
        assert_eq!(p.numbering_continue, Some(false));
    }

    #[test]
    fn parses_numbering_expression_as_property_element() {
        // InDesign often emits NumberingExpression as an element-form
        // child of <Properties> (typed string), not as an attribute.
        // The parser must pick that up so the cascade carries the
        // template forward.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Numbered"
                            BulletsAndNumberingListType="NumberedList">
              <Properties>
                <NumberingExpression type="string">^#)^t</NumberingExpression>
              </Properties>
            </ParagraphStyle>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Numbered").unwrap();
        assert_eq!(p.numbering_expression.as_deref(), Some("^#)^t"));
    }

    #[test]
    fn resolve_paragraph_propagates_numbering_overrides_through_based_on() {
        // Numbered base style sets the expression + start; a child
        // style only flips Continue. Cascade should pull the
        // expression and StartAt from the parent while overriding
        // Continue locally.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Base"
                            NumberingExpression="^#.^t"
                            NumberingStartAt="3"/>
            <ParagraphStyle Self="ParagraphStyle/Child"
                            BasedOn="ParagraphStyle/Base"
                            NumberingContinue="true"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Child");
        assert_eq!(r.numbering_expression.as_deref(), Some("^#.^t"));
        assert_eq!(r.numbering_start_at, Some(3));
        assert_eq!(r.numbering_continue, Some(true));
    }

    /// InDesign exports `AppliedFont` and `BasedOn` as element-form
    /// children of `<Properties>` rather than attributes on the
    /// style element. Without this path the cascade reads
    /// `font: None` for every paragraph style and runs that only
    /// inherit a font through their applied paragraph style end up
    /// fontless — and therefore unshaped — in real-world IDMLs.
    #[test]
    fn parses_applied_font_and_based_on_as_property_elements() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Body" Name="Body"
                            FontStyle="Italic" PointSize="8"
                            FillColor="Color/Red">
              <Properties>
                <BasedOn type="string">$ID/[No paragraph style]</BasedOn>
                <Leading type="unit">12</Leading>
                <AppliedFont type="string">Open Sans</AppliedFont>
              </Properties>
            </ParagraphStyle>
          </RootParagraphStyleGroup>
          <RootCharacterStyleGroup>
            <CharacterStyle Self="CharacterStyle/Emph" Name="Emph">
              <Properties>
                <BasedOn type="string">CharacterStyle/Base</BasedOn>
                <AppliedFont type="string">Minion Pro</AppliedFont>
              </Properties>
            </CharacterStyle>
          </RootCharacterStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Body").unwrap();
        assert_eq!(p.font.as_deref(), Some("Open Sans"));
        assert_eq!(p.based_on.as_deref(), Some("$ID/[No paragraph style]"));
        let c = s.character_styles.get("CharacterStyle/Emph").unwrap();
        assert_eq!(c.font.as_deref(), Some("Minion Pro"));
        assert_eq!(c.based_on.as_deref(), Some("CharacterStyle/Base"));

        // Cascade pulls font through to the resolved paragraph attrs.
        let r = s.resolve_paragraph("ParagraphStyle/Body");
        assert_eq!(r.font.as_deref(), Some("Open Sans"));
    }

    #[test]
    fn parses_toc_style_with_entries() {
        // Real-world `<TOCStyle>` carries a `<TOCStyleEntry>` per
        // outline level. The parser must capture the title, the
        // title-style ref, and each entry's IncludeStyle /
        // FormatStyle / Level / PageNumber / Separator (separator
        // defaults to a tab `^t` at resolve time when absent).
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/TocTitle" Name="TocTitle"/>
            <ParagraphStyle Self="ParagraphStyle/Heading1" Name="Heading1"/>
            <ParagraphStyle Self="ParagraphStyle/Heading2" Name="Heading2"/>
            <ParagraphStyle Self="ParagraphStyle/TocFormat1" Name="TocFormat1"/>
            <ParagraphStyle Self="ParagraphStyle/TocFormat2" Name="TocFormat2"/>
          </RootParagraphStyleGroup>
          <TOCStyle Self="TOCStyle/Main" Name="Main" Title="Contents"
                    TitleStyle="ParagraphStyle/TocTitle"
                    IncludeBookDocuments="false"
                    IncludeHidden="false"
                    RunIn="false">
            <TOCStyleEntry Name="Heading1"
                           IncludeStyle="ParagraphStyle/Heading1"
                           FormatStyle="ParagraphStyle/TocFormat1"
                           Level="1"
                           PageNumber="On"
                           Separator="^t"/>
            <TOCStyleEntry Name="Heading2"
                           IncludeStyle="ParagraphStyle/Heading2"
                           FormatStyle="ParagraphStyle/TocFormat2"
                           Level="2"
                           PageNumber="On"
                           Separator=" -- "/>
          </TOCStyle>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let toc = s.toc_styles.get("TOCStyle/Main").unwrap();
        assert_eq!(toc.title.as_deref(), Some("Contents"));
        assert_eq!(toc.title_style.as_deref(), Some("ParagraphStyle/TocTitle"));
        assert_eq!(toc.include_book_documents, Some(false));
        assert_eq!(toc.include_hidden, Some(false));
        assert_eq!(toc.run_in, Some(false));
        assert_eq!(toc.entries.len(), 2);
        let e1 = &toc.entries[0];
        assert_eq!(e1.include_style.as_deref(), Some("ParagraphStyle/Heading1"));
        assert_eq!(
            e1.format_style.as_deref(),
            Some("ParagraphStyle/TocFormat1")
        );
        assert_eq!(e1.level, Some(1));
        assert_eq!(e1.page_number.as_deref(), Some("On"));
        assert_eq!(e1.separator.as_deref(), Some("^t"));
        let e2 = &toc.entries[1];
        assert_eq!(e2.level, Some(2));
        assert_eq!(e2.separator.as_deref(), Some(" -- "));
    }

    #[test]
    fn parses_self_closing_empty_toc_style() {
        // InDesign always emits a default `<TOCStyle .../>` even when
        // the document has no TOC. The parser must accept the self-
        // closing form and produce an entry with no children.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <TOCStyle Self="TOCStyle/$ID/DefaultTOCStyleName"
                    Name="$ID/DefaultTOCStyleName"
                    Title="Contents"
                    TitleStyle="ParagraphStyle/$ID/[No paragraph style]"
                    RunIn="false"
                    IncludeHidden="false"
                    IncludeBookDocuments="false"/>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let toc = s
            .toc_styles
            .get("TOCStyle/$ID/DefaultTOCStyleName")
            .unwrap();
        assert_eq!(toc.title.as_deref(), Some("Contents"));
        assert!(toc.entries.is_empty());
    }

    // ---- CJK Stage 1 (parser surface) ----

    #[test]
    fn paragraph_style_captures_kinsoku_and_mojikumi_attributes() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/Japanese"
                            KinsokuSet="KinsokuTable/$ID/PhotoshopKinsokuHard"
                            KinsokuType="PushOut"
                            MojikumiTable="MojikumiTable/$ID/PhotoshopMojikumiSet4"
                            MojikumiSet="MojikumiSet/$ID/OldSet"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let p = s.paragraph_styles.get("ParagraphStyle/Japanese").unwrap();
        assert_eq!(
            p.kinsoku_set.as_deref(),
            Some("KinsokuTable/$ID/PhotoshopKinsokuHard")
        );
        assert_eq!(p.kinsoku_type.as_deref(), Some("PushOut"));
        assert_eq!(
            p.mojikumi_table.as_deref(),
            Some("MojikumiTable/$ID/PhotoshopMojikumiSet4")
        );
        assert_eq!(p.mojikumi_set.as_deref(), Some("MojikumiSet/$ID/OldSet"));
    }

    #[test]
    fn character_style_captures_ruby_and_kenten_attributes() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootCharacterStyleGroup>
            <CharacterStyle Self="CharacterStyle/RubyBase"
                            RubyFlag="true"
                            RubyType="GroupRuby"
                            RubyString="furigana"
                            KentenKind="BlackCircle"
                            KentenFontSize="50"/>
          </RootCharacterStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let c = s.character_styles.get("CharacterStyle/RubyBase").unwrap();
        assert_eq!(c.ruby_flag, Some(true));
        assert_eq!(c.ruby_type.as_deref(), Some("GroupRuby"));
        assert_eq!(c.ruby_string.as_deref(), Some("furigana"));
        assert_eq!(c.kenten_kind.as_deref(), Some("BlackCircle"));
        assert_eq!(c.kenten_font_size, Some(50.0));
    }

    #[test]
    fn resolve_paragraph_propagates_kinsoku_through_based_on() {
        // Base style sets the kinsoku ref; child overrides only one
        // field. Cascade should pull the rest from BasedOn.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootParagraphStyleGroup>
            <ParagraphStyle Self="ParagraphStyle/JpBase"
                            KinsokuSet="KinsokuTable/$ID/PhotoshopKinsokuHard"
                            KinsokuType="PushIn"
                            MojikumiTable="MojikumiTable/$ID/PhotoshopMojikumiSet4"/>
            <ParagraphStyle Self="ParagraphStyle/JpChild"
                            BasedOn="ParagraphStyle/JpBase"
                            KinsokuType="PushOut"/>
          </RootParagraphStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/JpChild");
        // Local override wins for KinsokuType.
        assert_eq!(r.kinsoku_type.as_deref(), Some("PushOut"));
        // Other fields cascade from BasedOn.
        assert_eq!(
            r.kinsoku_set.as_deref(),
            Some("KinsokuTable/$ID/PhotoshopKinsokuHard")
        );
        assert_eq!(
            r.mojikumi_table.as_deref(),
            Some("MojikumiTable/$ID/PhotoshopMojikumiSet4")
        );
    }

    // ---- Track 4a: custom StrokeStyle parsing ----

    #[test]
    fn dashed_stroke_style_parses_pattern_into_floats() {
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
            <DashedStrokeStyle Self="StrokeStyle/u163" Name="Diag"
                               StartCap="ButtEndCap" CornerAdjustment="None"
                               GapColor="Swatch/None" GapTint="100"
                               Pattern="3.5 2 1 4"/>
            <DottedStrokeStyle Self="StrokeStyle/u164" Name="Tight"
                               GapColor="Swatch/None" GapTint="100"/>
          </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let dash = s.stroke_styles.get("StrokeStyle/u163").unwrap();
        assert_eq!(dash.kind, StrokeStyleKind::Dashed);
        assert_eq!(dash.name.as_deref(), Some("Diag"));
        assert_eq!(dash.pattern, vec![3.5, 2.0, 1.0, 4.0]);
        let dot = s.stroke_styles.get("StrokeStyle/u164").unwrap();
        assert_eq!(dot.kind, StrokeStyleKind::Dotted);
        assert!(dot.pattern.is_empty());
    }
}
