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

use crate::story::TabStop;
use crate::util::attr;
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
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
}

/// Effective object-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedObject {
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
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
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
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
    pub tracking: Option<f32>,
    pub justification: Option<String>,
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
}

/// Effective character-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedCharacter {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
}

/// Effective paragraph-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraph {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub tracking: Option<f32>,
    pub justification: Option<String>,
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
                    b"AppliedFont" if current_style.is_some() => {
                        pending_property = Some(CurrentProperty::AppliedFont);
                    }
                    b"BasedOn" if current_style.is_some() => {
                        pending_property = Some(CurrentProperty::BasedOn);
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
                    b"AppliedFont" | b"BasedOn" => {
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
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
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
        self.tracking = self.tracking.or(def.tracking);
        self.underline = self.underline.or(def.underline);
        self.strikethru = self.strikethru.or(def.strikethru);
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
        self.tracking = self.tracking.or(def.tracking);
        if self.justification.is_none() {
            self.justification = def.justification.clone();
        }
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
    }
}

fn parse_character_style(e: &quick_xml::events::BytesStart) -> Option<CharacterStyleDef> {
    Some(CharacterStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        font: attr(e, b"AppliedFont"),
        font_style: attr(e, b"FontStyle"),
        point_size: attr(e, b"PointSize").and_then(|s| s.parse().ok()),
        fill_color: attr(e, b"FillColor"),
        tracking: attr(e, b"Tracking").and_then(|s| s.parse().ok()),
        underline: attr(e, b"Underline").and_then(|s| s.parse().ok()),
        strikethru: attr(e, b"StrikeThru").and_then(|s| s.parse().ok()),
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
        start_row_fill_tint: attr(e, b"StartRowFillTint").and_then(|s| s.parse().ok()),
        end_row_fill_color: normalize(attr(e, b"EndRowFillColor")),
        end_row_fill_count: attr(e, b"EndRowFillCount").and_then(|s| s.parse().ok()),
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
        stroke_color: normalize(attr(e, b"StrokeColor")),
        stroke_weight,
    })
}

fn parse_paragraph_style(e: &quick_xml::events::BytesStart) -> Option<ParagraphStyleDef> {
    Some(ParagraphStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        font: attr(e, b"AppliedFont"),
        font_style: attr(e, b"FontStyle"),
        point_size: attr(e, b"PointSize").and_then(|s| s.parse().ok()),
        fill_color: attr(e, b"FillColor"),
        tracking: attr(e, b"Tracking").and_then(|s| s.parse().ok()),
        justification: attr(e, b"Justification"),
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
        assert_eq!(r.justification.as_deref(), Some("LeftAlign"));
        assert_eq!(r.space_after, Some(6.0));
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
}
