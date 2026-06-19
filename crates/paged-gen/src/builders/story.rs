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

//! Story builder. A `Story` carries one or more paragraphs; each
//! paragraph carries one or more character runs. Per-paragraph and
//! per-run attribute overrides let samples exercise alignment, point
//! size, fill colour, and font style without re-writing the whole
//! resource cascade.

use crate::xml::XmlBuilder;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub struct Story {
    pub self_id: String,
    pub paragraphs: Vec<Paragraph>,
}

pub struct Paragraph {
    /// `Justification` attribute on `ParagraphStyleRange` — e.g.
    /// `"LeftAlign"`, `"CenterAlign"`, `"RightAlign"`,
    /// `"LeftJustified"`. `None` ⇒ inherit from the applied style.
    pub justification: Option<&'static str>,
    /// `SpaceBefore` (pt) on the paragraph style range.
    pub space_before: Option<f32>,
    /// `SpaceAfter` (pt).
    pub space_after: Option<f32>,
    /// Numeric leading override on the first run, in pt. IDML carries
    /// leading on the character style range as a `Leading` attribute
    /// of type `Number` (with magic `Auto` not modelled here).
    pub leading: Option<f32>,
    /// `FirstLineIndent` in pt (positive shifts the first line right;
    /// negative produces an outdent that pairs with `left_indent` for
    /// hanging-indent layouts).
    pub first_line_indent: Option<f32>,
    /// `LeftIndent` in pt — narrows the column from the left.
    pub left_indent: Option<f32>,
    /// `RightIndent` in pt — narrows the column from the right.
    pub right_indent: Option<f32>,
    /// `DropCapCharacters` — number of leading characters that drop.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` — number of lines the dropped characters span.
    pub drop_cap_lines: Option<u32>,
    /// `<TabList>` — emitted as `<Properties><TabList><ListItem>
    /// <TabStop .../></ListItem>…</TabList></Properties>` matching
    /// the parser's expected shape.
    pub tab_list: Vec<TabStop>,
    /// `BulletsAndNumberingListType` attribute on the
    /// `<ParagraphStyleRange>` — `"BulletList"`, `"NumberedList"`,
    /// or `"NoList"`. None ⇒ no bullet/number marker.
    pub bullets_list_type: Option<&'static str>,
    /// W1.22 (engine gap 22) — `AppliedNumberingList` reference on the
    /// `<ParagraphStyleRange>` (e.g. `"NumberingList/Shared"`). Binds
    /// the paragraph to a named `<NumberingList>` resource so the
    /// renderer can resolve its cross-story continuity flag. `None` ⇒
    /// omit the attribute.
    pub applied_numbering_list: Option<&'static str>,
    /// Inline bullet glyph override emitted as
    /// `<Properties><BulletChar BulletCharacterType="UnicodeWithFont"
    /// BulletCharacterValue="…"/></Properties>`. Useful for non-default
    /// bullet glyphs (e.g. U+00BB `»`). When `None`, BulletList
    /// paragraphs render the renderer's default `•` (U+2022).
    pub bullet_character: Option<u32>,
    pub runs: Vec<Run>,
    /// Optional `<Table>` payload — when set, a table is emitted
    /// nested inside this paragraph's first CharacterStyleRange,
    /// matching IDML's "table host paragraph" structure. Cells own
    /// their own paragraph content; the surrounding paragraph's runs
    /// remain (typically empty) for the parser's sake.
    pub table: Option<Table>,
    /// `MinimumLetterSpacing` (pt) on the paragraph style range —
    /// cycle-7 Track 1, used to seed Q-20 calibration fixtures.
    /// `None` ⇒ omit (renderer uses 0).
    pub minimum_letter_spacing: Option<f32>,
    /// `DesiredLetterSpacing` (pt). `None` ⇒ omit.
    pub desired_letter_spacing: Option<f32>,
    /// `MaximumLetterSpacing` (pt). `None` ⇒ omit.
    pub maximum_letter_spacing: Option<f32>,
    /// Escape hatch for paragraph-level attributes the typed fields don't
    /// cover (e.g. `RuleAbove*`, `ParagraphShading*`, `ParagraphBorder*`).
    /// Emitted verbatim onto `<ParagraphStyleRange>` after the typed attrs.
    /// `paged-write` carries these through the round-trip untouched, so a
    /// fixture using them reaches the conformance round-trips level.
    pub extra_paragraph_attrs: Vec<(&'static str, &'static str)>,
}

/// One stop in a paragraph's `<TabList>`. Position is in pt from
/// the column's left edge. Mirrors `paged_parse::story::TabStop`.
pub struct TabStop {
    pub position_pt: f32,
    /// IDML alignment string — `"LeftAlign"`, `"RightAlign"`,
    /// `"CenterAlign"`, or `"CharacterAlign"`.
    pub alignment: &'static str,
    /// Leader characters (e.g. `"."` for a dotted leader) drawn to
    /// fill the tab's empty span. `None` ⇒ no leader.
    pub leader: Option<String>,
}

/// Minimal `<Table>` payload the generator can emit. Cells must be
/// supplied in column-major IDML order: column 0 rows 0..N, column 1
/// rows 0..N, etc. The builder names cells `"col:row"` automatically
/// and writes one Row + Column entry per index based on `row_count`
/// and `column_count`.
pub struct Table {
    pub self_id: String,
    pub header_row_count: u32,
    pub footer_row_count: u32,
    pub body_row_count: u32,
    pub column_count: u32,
    /// `AppliedTableStyle` reference. `None` ⇒ the built-in
    /// `"TableStyle/$ID/[No table style]"`. Set this to a custom
    /// table-style self-id (declared via
    /// `styles_xml_with_table_styles`) to drive alternating fills.
    pub applied_table_style: Option<String>,
    /// Per-row height in pt. Length must equal
    /// `header + body + footer`. Each entry seeds the row's
    /// `SingleRowHeight` attribute.
    pub row_heights_pt: Vec<f32>,
    /// Per-column width in pt. Length must equal `column_count`.
    pub column_widths_pt: Vec<f32>,
    /// Cells in column-major order. Length must equal
    /// `column_count * total_rows`.
    pub cells: Vec<Cell>,
}

pub struct Cell {
    /// Cell-level paragraphs. Reuses the top-level `Paragraph`/`Run`
    /// shape so cell contents can carry the same styling knobs as
    /// regular story text.
    pub paragraphs: Vec<Paragraph>,
    /// Optional FillColor reference — e.g. for header rows or
    /// alternating-fill demos.
    pub fill_color: Option<String>,
    /// Per-edge stroke colour overrides. Each is independent; absent
    /// edges fall back to the cascade default (typically black 1pt).
    pub top_edge_stroke_color: Option<&'static str>,
    pub bottom_edge_stroke_color: Option<&'static str>,
    pub left_edge_stroke_color: Option<&'static str>,
    pub right_edge_stroke_color: Option<&'static str>,
    /// Per-edge stroke weights in pt. None ⇒ cascade default.
    pub top_edge_stroke_weight: Option<f32>,
    pub bottom_edge_stroke_weight: Option<f32>,
    pub left_edge_stroke_weight: Option<f32>,
    pub right_edge_stroke_weight: Option<f32>,
    /// `RowSpan` attribute (default 1). When > 1, this cell occupies
    /// the next N rows and the cells those rows-down would otherwise
    /// host should be omitted from the cell list.
    pub row_span: u32,
    /// `ColumnSpan` attribute (default 1).
    pub column_span: u32,
    /// `RotationAngle` attribute in degrees — non-zero rotates the
    /// cell's text content (e.g. 90° vertical headers). Real
    /// InDesign exports also emit `TableTextRotation="Rotate90Degrees"`
    /// when the cell carries an explicit angle; we emit both so
    /// strict readers see consistent settings.
    pub rotation_angle: Option<f32>,
    /// Diagonal stroke settings. `LeftLine*` is the TL→BR diagonal,
    /// `RightLine*` the TR→BL diagonal. `diagonal_in_front` maps to
    /// `DiagonalLineInFront`.
    pub diagonal: CellDiagonal,
    /// `VerticalJustification` enum string (`"TopAlign"`,
    /// `"CenterAlign"`, `"BottomAlign"`, `"JustifyAlign"`). `None` ⇒
    /// the attribute is omitted (renderer defaults to Top).
    pub vertical_justification: Option<&'static str>,
    /// Per-edge stroke tint percentages (0..=100). `None` ⇒ omit; the
    /// edge paints at full strength. Pairs with the edge colours above.
    pub top_edge_stroke_tint: Option<f32>,
    pub bottom_edge_stroke_tint: Option<f32>,
    pub left_edge_stroke_tint: Option<f32>,
    pub right_edge_stroke_tint: Option<f32>,
}

/// Generator-side mirror of IDML's per-cell diagonal stroke
/// attributes. Defaults to "no diagonal".
#[derive(Default, Clone)]
pub struct CellDiagonal {
    pub left_line_drawn: Option<bool>,
    pub left_line_color: Option<&'static str>,
    pub left_line_weight: Option<f32>,
    pub right_line_drawn: Option<bool>,
    pub right_line_color: Option<&'static str>,
    pub right_line_weight: Option<f32>,
    pub diagonal_in_front: Option<bool>,
}

impl Cell {
    /// Plain text cell with default edges and no fill. Convenience
    /// helper for the common case of a body cell with one paragraph.
    pub fn plain<S: Into<String>>(text: S) -> Self {
        Self {
            paragraphs: vec![Paragraph::plain(text)],
            fill_color: None,
            top_edge_stroke_color: None,
            bottom_edge_stroke_color: None,
            left_edge_stroke_color: None,
            right_edge_stroke_color: None,
            top_edge_stroke_weight: None,
            bottom_edge_stroke_weight: None,
            left_edge_stroke_weight: None,
            right_edge_stroke_weight: None,
            row_span: 1,
            column_span: 1,
            rotation_angle: None,
            diagonal: CellDiagonal::default(),
            vertical_justification: None,
            top_edge_stroke_tint: None,
            bottom_edge_stroke_tint: None,
            left_edge_stroke_tint: None,
            right_edge_stroke_tint: None,
        }
    }

    pub fn with_span(mut self, row_span: u32, column_span: u32) -> Self {
        self.row_span = row_span;
        self.column_span = column_span;
        self
    }

    pub fn with_rotation_angle(mut self, deg: f32) -> Self {
        self.rotation_angle = Some(deg);
        self
    }

    pub fn with_diagonal(mut self, diagonal: CellDiagonal) -> Self {
        self.diagonal = diagonal;
        self
    }

    pub fn with_vertical_justification(mut self, vj: &'static str) -> Self {
        self.vertical_justification = Some(vj);
        self
    }
}

pub struct Run {
    pub text: String,
    /// `PointSize` attribute on `CharacterStyleRange`.
    pub point_size: Option<f32>,
    /// `FillColor` reference (e.g. `"Color/Black"`).
    pub fill_color: Option<String>,
    /// `FontStyle` attribute (e.g. `"Bold"`, `"Italic"`).
    pub font_style: Option<&'static str>,
    /// `Tracking` in 1/1000 em (InDesign's unit; positive widens).
    pub tracking: Option<f32>,
    /// `BaselineShift` in pt; positive lifts.
    pub baseline_shift: Option<f32>,
    /// `Underline="true"`.
    pub underline: Option<bool>,
    /// `AppliedFont` family name (for runs that pin to a different
    /// face from the paragraph default).
    pub applied_font: Option<&'static str>,
    /// Optional anchored object — a `<TextFrame>` or `<Rectangle>`
    /// emitted inside this run's CharacterStyleRange. Together with
    /// the `text` body (typically a single non-breaking space placeholder)
    /// this becomes IDML's anchored-object inline-position shape:
    /// the host story sees a glyph slot the renderer can replace
    /// with the rendered frame. The frame's
    /// `Rect::anchored_setting` is what tells the parser this is an
    /// anchored object — the storey emitter just provides the
    /// nesting site.
    pub anchored_frame: Option<crate::builders::page_item::Rect>,
    /// Escape hatch for character-level attributes the typed fields don't
    /// cover (e.g. `VerticalScale`, `HorizontalScale`, ruby/kenten flags).
    /// Emitted verbatim onto `<CharacterStyleRange>` after the typed attrs;
    /// carried through the round-trip by paged-write.
    pub extra_char_attrs: Vec<(&'static str, &'static str)>,
}

impl Run {
    /// A plain text run carrying only the given extra character attributes
    /// (the escape hatch for VerticalScale, ruby, …). All typed fields default.
    pub fn with_char_attrs(
        text: impl Into<String>,
        attrs: Vec<(&'static str, &'static str)>,
    ) -> Self {
        Run {
            text: text.into(),
            point_size: None,
            fill_color: None,
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: None,
            anchored_frame: None,
            extra_char_attrs: attrs,
        }
    }
}

impl Paragraph {
    /// Convenience constructor: one paragraph, one run, default
    /// styling. Used by samples that just want a labelled page.
    pub fn plain<S: Into<String>>(text: S) -> Self {
        Self {
            justification: None,
            space_before: None,
            space_after: None,
            leading: None,
            first_line_indent: None,
            left_indent: None,
            right_indent: None,
            drop_cap_characters: None,
            drop_cap_lines: None,
            tab_list: Vec::new(),
            bullets_list_type: None,
            applied_numbering_list: None,
            bullet_character: None,
            runs: vec![Run {
                text: text.into(),
                point_size: None,
                fill_color: None,
                font_style: None,
                tracking: None,
                baseline_shift: None,
                underline: None,
                applied_font: None,
                anchored_frame: None,
                extra_char_attrs: Vec::new(),
            }],
            table: None,
            minimum_letter_spacing: None,
            desired_letter_spacing: None,
            maximum_letter_spacing: None,
            extra_paragraph_attrs: Vec::new(),
        }
    }

    /// Attach extra `<ParagraphStyleRange>` attributes (the escape hatch
    /// for rules/shading/border). Returns `self` for chaining off `plain`.
    pub fn with_para_attrs(
        mut self,
        attrs: Vec<(&'static str, &'static str)>,
    ) -> Self {
        self.extra_paragraph_attrs = attrs;
        self
    }

    /// Nest a `<Table>` inside this paragraph's first CharacterStyleRange —
    /// the IDML "table host paragraph" shape. A table whose cell paragraphs
    /// carry their own table is a NESTED table.
    pub fn with_table(mut self, table: Table) -> Self {
        self.table = Some(table);
        self
    }
}

pub fn write_story(s: &Story) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", s.self_id.as_str())]);
    for paragraph in &s.paragraphs {
        let space_before_str: String;
        let space_after_str: String;
        let first_line_indent_str: String;
        let left_indent_str: String;
        let right_indent_str: String;
        let drop_cap_chars_str: String;
        let drop_cap_lines_str: String;
        let min_ls_str: String;
        let desired_ls_str: String;
        let max_ls_str: String;
        let mut p_attrs: Vec<(&str, &str)> = vec![(
            "AppliedParagraphStyle",
            "ParagraphStyle/$ID/[No paragraph style]",
        )];
        if let Some(j) = paragraph.justification {
            p_attrs.push(("Justification", j));
        }
        if let Some(sb) = paragraph.space_before {
            space_before_str = crate::xml::format_f32(sb);
            p_attrs.push(("SpaceBefore", space_before_str.as_str()));
        }
        if let Some(sa) = paragraph.space_after {
            space_after_str = crate::xml::format_f32(sa);
            p_attrs.push(("SpaceAfter", space_after_str.as_str()));
        }
        if let Some(fli) = paragraph.first_line_indent {
            first_line_indent_str = crate::xml::format_f32(fli);
            p_attrs.push(("FirstLineIndent", first_line_indent_str.as_str()));
        }
        if let Some(li) = paragraph.left_indent {
            left_indent_str = crate::xml::format_f32(li);
            p_attrs.push(("LeftIndent", left_indent_str.as_str()));
        }
        if let Some(ri) = paragraph.right_indent {
            right_indent_str = crate::xml::format_f32(ri);
            p_attrs.push(("RightIndent", right_indent_str.as_str()));
        }
        if let Some(dc) = paragraph.drop_cap_characters {
            drop_cap_chars_str = dc.to_string();
            p_attrs.push(("DropCapCharacters", drop_cap_chars_str.as_str()));
        }
        if let Some(dl) = paragraph.drop_cap_lines {
            drop_cap_lines_str = dl.to_string();
            p_attrs.push(("DropCapLines", drop_cap_lines_str.as_str()));
        }
        if let Some(blt) = paragraph.bullets_list_type {
            p_attrs.push(("BulletsAndNumberingListType", blt));
        }
        if let Some(anl) = paragraph.applied_numbering_list {
            p_attrs.push(("AppliedNumberingList", anl));
        }
        // Escape-hatch attributes (rules/shading/border, …) emitted verbatim.
        for (k, v) in &paragraph.extra_paragraph_attrs {
            p_attrs.push((*k, *v));
        }
        if let Some(v) = paragraph.minimum_letter_spacing {
            min_ls_str = crate::xml::format_f32(v);
            p_attrs.push(("MinimumLetterSpacing", min_ls_str.as_str()));
        }
        if let Some(v) = paragraph.desired_letter_spacing {
            desired_ls_str = crate::xml::format_f32(v);
            p_attrs.push(("DesiredLetterSpacing", desired_ls_str.as_str()));
        }
        if let Some(v) = paragraph.maximum_letter_spacing {
            max_ls_str = crate::xml::format_f32(v);
            p_attrs.push(("MaximumLetterSpacing", max_ls_str.as_str()));
        }
        b.start("ParagraphStyleRange", &p_attrs);
        // Bullet/tab Properties container — combines the inline
        // BulletChar override (when set) with the TabList. Real
        // InDesign emits these as siblings inside one <Properties>
        // block; the parser walks the children so emitting them in
        // order is enough.
        let has_bullet_char = paragraph.bullet_character.is_some();
        if has_bullet_char || !paragraph.tab_list.is_empty() {
            b.start("Properties", &[]);
            if let Some(cp) = paragraph.bullet_character {
                let cp_str = cp.to_string();
                b.empty(
                    "BulletChar",
                    &[
                        ("BulletCharacterType", "UnicodeWithFont"),
                        ("BulletCharacterValue", cp_str.as_str()),
                    ],
                );
            }
            if !paragraph.tab_list.is_empty() {
                b.start("TabList", &[]);
                for stop in &paragraph.tab_list {
                    let pos = crate::xml::format_f32(stop.position_pt);
                    b.start("ListItem", &[("type", "object")]);
                    let mut attrs: Vec<(&str, &str)> =
                        vec![("Position", pos.as_str()), ("Alignment", stop.alignment)];
                    if let Some(ld) = &stop.leader {
                        attrs.push(("Leader", ld.as_str()));
                    }
                    b.empty("TabStop", &attrs);
                    b.end("ListItem");
                }
                b.end("TabList");
            }
            b.end("Properties");
        }
        // Tables nest inside a CharacterStyleRange child of the
        // host paragraph. When a paragraph carries a table, emit
        // exactly that one child and skip the run loop — IDML
        // doesn't carry sibling text alongside a table inside the
        // same character range.
        if let Some(t) = &paragraph.table {
            b.start(
                "CharacterStyleRange",
                &[(
                    "AppliedCharacterStyle",
                    "CharacterStyle/$ID/[No character style]",
                )],
            );
            write_table(&mut b, t);
            b.end("CharacterStyleRange");
            b.end("ParagraphStyleRange");
            continue;
        }
        for (idx, run) in paragraph.runs.iter().enumerate() {
            let point_size_str: String;
            let tracking_str: String;
            let baseline_str: String;
            let mut r_attrs: Vec<(&str, &str)> = vec![(
                "AppliedCharacterStyle",
                "CharacterStyle/$ID/[No character style]",
            )];
            if let Some(size) = run.point_size {
                point_size_str = crate::xml::format_f32(size);
                r_attrs.push(("PointSize", point_size_str.as_str()));
            }
            if let Some(fill) = &run.fill_color {
                r_attrs.push(("FillColor", fill.as_str()));
            }
            if let Some(style) = run.font_style {
                r_attrs.push(("FontStyle", style));
            }
            if let Some(tracking) = run.tracking {
                tracking_str = crate::xml::format_f32(tracking);
                r_attrs.push(("Tracking", tracking_str.as_str()));
            }
            if let Some(shift) = run.baseline_shift {
                baseline_str = crate::xml::format_f32(shift);
                r_attrs.push(("BaselineShift", baseline_str.as_str()));
            }
            if let Some(true) = run.underline {
                r_attrs.push(("Underline", "true"));
            }
            // Escape-hatch character attributes (VerticalScale, ruby, …).
            for (k, v) in &run.extra_char_attrs {
                r_attrs.push((*k, *v));
            }
            b.start("CharacterStyleRange", &r_attrs);
            // AppliedFont + Leading land as typed children of
            // <Properties> — that's how real InDesign serialises them
            // and what paged-parse reads after the Properties slice.
            // Emitting them as attributes works too (the spec allows
            // both forms) but the child-element form is canonical and
            // round-trips cleanly through InDesign's IDML reader.
            let want_leading = idx == 0 && paragraph.leading.is_some();
            if run.applied_font.is_some() || want_leading {
                b.start("Properties", &[]);
                if let Some(font) = run.applied_font {
                    b.start("AppliedFont", &[("type", "string")]);
                    b.text(font);
                    b.end("AppliedFont");
                }
                if want_leading {
                    if let Some(lead) = paragraph.leading {
                        let s = crate::xml::format_f32(lead);
                        b.start("Leading", &[("type", "unit")]);
                        b.text(&s);
                        b.end("Leading");
                    }
                }
                b.end("Properties");
            }
            // Anchored frame: emit before the textual content so the
            // run's character index lines up with the anchor position
            // the renderer places the frame at. The frame's own
            // emitter writes `<TextFrame>...</TextFrame>` (or
            // Rectangle) plus an `<AnchoredObjectSetting>` child if
            // configured.
            if let Some(frame) = &run.anchored_frame {
                frame.write(&mut b);
            }
            write_run_content(&mut b, &run.text);
            b.end("CharacterStyleRange");
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// Emit a run's text body. Tabs (`\t`) become `<Tab/>` empty elements
/// between `<Content>` segments — matching how IDML serialises tabs
/// and how `paged_parse` rebuilds the run text. Newlines (`\n`) become
/// `<Br/>` line breaks for the same reason.
fn write_run_content(b: &mut XmlBuilder, text: &str) {
    if text.is_empty() {
        b.start("Content", &[]);
        b.end("Content");
        return;
    }
    let mut buf = String::new();
    let flush = |b: &mut XmlBuilder, buf: &mut String| {
        if !buf.is_empty() {
            b.start("Content", &[]);
            b.text(buf);
            b.end("Content");
            buf.clear();
        }
    };
    for ch in text.chars() {
        match ch {
            '\t' => {
                flush(b, &mut buf);
                b.empty("Tab", &[]);
            }
            '\n' => {
                flush(b, &mut buf);
                b.empty("Br", &[]);
            }
            _ => buf.push(ch),
        }
    }
    flush(b, &mut buf);
}

fn write_table(b: &mut XmlBuilder, t: &Table) {
    let header = t.header_row_count.to_string();
    let footer = t.footer_row_count.to_string();
    let body = t.body_row_count.to_string();
    let cols = t.column_count.to_string();
    let applied_style = t
        .applied_table_style
        .as_deref()
        .unwrap_or("TableStyle/$ID/[No table style]");
    b.start(
        "Table",
        &[
            ("Self", t.self_id.as_str()),
            ("HeaderRowCount", header.as_str()),
            ("FooterRowCount", footer.as_str()),
            ("BodyRowCount", body.as_str()),
            ("ColumnCount", cols.as_str()),
            ("AppliedTableStyle", applied_style),
        ],
    );
    let total_rows = t.header_row_count + t.body_row_count + t.footer_row_count;
    for r in 0..total_rows {
        let r_str = r.to_string();
        let h_str = t
            .row_heights_pt
            .get(r as usize)
            .copied()
            .map(crate::xml::format_f32)
            .unwrap_or_else(|| "20".to_string());
        let row_self = format!("{}_R{r}", t.self_id);
        b.empty(
            "Row",
            &[
                ("Self", row_self.as_str()),
                ("Name", r_str.as_str()),
                ("SingleRowHeight", h_str.as_str()),
            ],
        );
    }
    for c in 0..t.column_count {
        let c_str = c.to_string();
        let w_str = t
            .column_widths_pt
            .get(c as usize)
            .copied()
            .map(crate::xml::format_f32)
            .unwrap_or_else(|| "60".to_string());
        let col_self = format!("{}_C{c}", t.self_id);
        b.empty(
            "Column",
            &[
                ("Self", col_self.as_str()),
                ("Name", c_str.as_str()),
                ("SingleColumnWidth", w_str.as_str()),
            ],
        );
    }
    // Cells: column-major. Each next cell in the input list lands in
    // the next free slot in column-major order, marking slots covered
    // by RowSpan / ColumnSpan as occupied so spans don't double-stamp
    // a covered grid position.
    let mut idx = 0usize;
    let mut occupied: Vec<bool> = vec![false; (t.column_count as usize) * (total_rows as usize)];
    let slot = |col: u32, row: u32| (col as usize) * (total_rows as usize) + (row as usize);
    for c in 0..t.column_count {
        for r in 0..total_rows {
            if occupied[slot(c, r)] {
                continue;
            }
            let cell = match t.cells.get(idx) {
                Some(c) => c,
                None => break,
            };
            idx += 1;
            // Mark the slots this cell covers as occupied so future
            // (col, row) iterations skip them.
            for dc in 0..cell.column_span.max(1) {
                for dr in 0..cell.row_span.max(1) {
                    let cc = c + dc;
                    let rr = r + dr;
                    if cc < t.column_count && rr < total_rows {
                        occupied[slot(cc, rr)] = true;
                    }
                }
            }
            let name = format!("{c}:{r}");
            let cell_self = format!("{}_{c}_{r}", t.self_id);
            let mut a: Vec<(&str, String)> = vec![
                ("Self", cell_self),
                ("Name", name),
                ("RowSpan", cell.row_span.max(1).to_string()),
                ("ColumnSpan", cell.column_span.max(1).to_string()),
            ];
            if let Some(fc) = &cell.fill_color {
                a.push(("FillColor", fc.clone()));
            }
            if let Some(c) = cell.top_edge_stroke_color {
                a.push(("TopEdgeStrokeColor", c.to_string()));
            }
            if let Some(w) = cell.top_edge_stroke_weight {
                a.push(("TopEdgeStrokeWeight", crate::xml::format_f32(w)));
            }
            if let Some(t) = cell.top_edge_stroke_tint {
                a.push(("TopEdgeStrokeTint", crate::xml::format_f32(t)));
            }
            if let Some(c) = cell.bottom_edge_stroke_color {
                a.push(("BottomEdgeStrokeColor", c.to_string()));
            }
            if let Some(w) = cell.bottom_edge_stroke_weight {
                a.push(("BottomEdgeStrokeWeight", crate::xml::format_f32(w)));
            }
            if let Some(t) = cell.bottom_edge_stroke_tint {
                a.push(("BottomEdgeStrokeTint", crate::xml::format_f32(t)));
            }
            if let Some(c) = cell.left_edge_stroke_color {
                a.push(("LeftEdgeStrokeColor", c.to_string()));
            }
            if let Some(w) = cell.left_edge_stroke_weight {
                a.push(("LeftEdgeStrokeWeight", crate::xml::format_f32(w)));
            }
            if let Some(t) = cell.left_edge_stroke_tint {
                a.push(("LeftEdgeStrokeTint", crate::xml::format_f32(t)));
            }
            if let Some(c) = cell.right_edge_stroke_color {
                a.push(("RightEdgeStrokeColor", c.to_string()));
            }
            if let Some(w) = cell.right_edge_stroke_weight {
                a.push(("RightEdgeStrokeWeight", crate::xml::format_f32(w)));
            }
            if let Some(t) = cell.right_edge_stroke_tint {
                a.push(("RightEdgeStrokeTint", crate::xml::format_f32(t)));
            }
            if let Some(vj) = cell.vertical_justification {
                a.push(("VerticalJustification", vj.to_string()));
            }
            if let Some(deg) = cell.rotation_angle {
                a.push(("RotationAngle", crate::xml::format_f32(deg)));
                // Real InDesign also writes the corresponding text-
                // rotation enum string. The renderer's table path can
                // branch on either; emitting both keeps strict
                // readers happy.
                let trot = match deg as i32 {
                    90 => Some("Rotate90Degrees"),
                    180 => Some("Rotate180Degrees"),
                    270 => Some("Rotate270Degrees"),
                    _ => None,
                };
                if let Some(t) = trot {
                    a.push(("TableTextRotation", t.to_string()));
                }
            }
            let d = &cell.diagonal;
            if let Some(v) = d.left_line_drawn {
                a.push(("LeftLineDrawn", v.to_string()));
            }
            if let Some(c) = d.left_line_color {
                a.push(("LeftLineStrokeColor", c.to_string()));
            }
            if let Some(w) = d.left_line_weight {
                a.push(("LeftLineStrokeWeight", crate::xml::format_f32(w)));
            }
            if let Some(v) = d.right_line_drawn {
                a.push(("RightLineDrawn", v.to_string()));
            }
            if let Some(c) = d.right_line_color {
                a.push(("RightLineStrokeColor", c.to_string()));
            }
            if let Some(w) = d.right_line_weight {
                a.push(("RightLineStrokeWeight", crate::xml::format_f32(w)));
            }
            if let Some(v) = d.diagonal_in_front {
                a.push(("DiagonalLineInFront", v.to_string()));
            }
            let attr_refs: Vec<(&str, &str)> = a.iter().map(|(k, v)| (*k, v.as_str())).collect();
            b.start("Cell", &attr_refs);
            for p in &cell.paragraphs {
                write_cell_paragraph(b, p);
            }
            b.end("Cell");
        }
    }
    b.end("Table");
}

/// Cell-content paragraph emitter — same shape as the top-level
/// loop in `write_story` but inlined so the table path stays
/// self-contained.
fn write_cell_paragraph(b: &mut XmlBuilder, paragraph: &Paragraph) {
    let mut p_attrs: Vec<(&str, &str)> = vec![(
        "AppliedParagraphStyle",
        "ParagraphStyle/$ID/[No paragraph style]",
    )];
    if let Some(j) = paragraph.justification {
        p_attrs.push(("Justification", j));
    }
    b.start("ParagraphStyleRange", &p_attrs);
    // A cell paragraph can itself host a table — the nested-table shape.
    if let Some(t) = &paragraph.table {
        b.start(
            "CharacterStyleRange",
            &[(
                "AppliedCharacterStyle",
                "CharacterStyle/$ID/[No character style]",
            )],
        );
        write_table(b, t);
        b.end("CharacterStyleRange");
        b.end("ParagraphStyleRange");
        return;
    }
    for run in &paragraph.runs {
        let point_size_str: String;
        let mut r_attrs: Vec<(&str, &str)> = vec![(
            "AppliedCharacterStyle",
            "CharacterStyle/$ID/[No character style]",
        )];
        if let Some(size) = run.point_size {
            point_size_str = crate::xml::format_f32(size);
            r_attrs.push(("PointSize", point_size_str.as_str()));
        }
        if let Some(fill) = &run.fill_color {
            r_attrs.push(("FillColor", fill.as_str()));
        }
        if let Some(style) = run.font_style {
            r_attrs.push(("FontStyle", style));
        }
        b.start("CharacterStyleRange", &r_attrs);
        if let Some(font) = run.applied_font {
            b.start("Properties", &[]);
            b.start("AppliedFont", &[("type", "string")]);
            b.text(font);
            b.end("AppliedFont");
            b.end("Properties");
        }
        write_run_content(b, &run.text);
        b.end("CharacterStyleRange");
    }
    b.end("ParagraphStyleRange");
}
