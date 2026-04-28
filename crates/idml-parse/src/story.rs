//! Story_*.xml parser.
//!
//! An IDML Story is a tree:
//! ```text
//! <Story>
//!   <ParagraphStyleRange AppliedParagraphStyle="...">
//!     <CharacterStyleRange AppliedCharacterStyle="..." PointSize="12" AppliedFont="...">
//!       <Content>Some text</Content>
//!       <Br/>
//!       <Content>more text</Content>
//!     </CharacterStyleRange>
//!     <CharacterStyleRange ...>
//!       <Content>bold bit</Content>
//!     </CharacterStyleRange>
//!   </ParagraphStyleRange>
//!   <ParagraphStyleRange>...</ParagraphStyleRange>
//! </Story>
//! ```
//!
//! The parser collapses all `<Content>` children of a character range
//! into a single string, preserving paragraph boundaries. Full style
//! resolution (font cascade, local overrides, etc.) is the job of
//! `idml-scene`; this module stays focused on shape extraction.

use quick_xml::events::Event;
use serde::Serialize;

use crate::util::{attr, parse_tint_attr};
use crate::ParseError;

/// Private-use Unicode codepoint placed inline by the story parser
/// where IDML carries `<?ACE 18?>` (auto current-page-number).
/// Renderers substitute this with the live page's number / Name
/// at emit time. Picked from the U+E0xx Tag block — outside any
/// rendered glyph plane, never produced by real text.
pub const AUTO_PAGE_NUMBER_MARKER: char = '\u{E018}';
/// Same idea for `<?ACE 19?>` (next-page-number marker; used in
/// "continued on page" footers).
pub const NEXT_PAGE_NUMBER_MARKER: char = '\u{E019}';

#[derive(Debug, Default, Clone, Serialize)]
pub struct Story {
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Paragraph {
    pub paragraph_style: Option<String>,
    /// `Justification` attribute from IDML. Common values:
    /// `LeftAlign`, `CenterAlign`, `RightAlign`, `FullyJustified`,
    /// `LeftJustified`, `CenterJustified`, `RightJustified`.
    pub justification: Option<String>,
    /// `FirstLineIndent` in pt.
    pub first_line_indent: Option<f32>,
    /// `SpaceBefore` in pt.
    pub space_before: Option<f32>,
    /// `SpaceAfter` in pt.
    pub space_after: Option<f32>,
    /// `<TabList>` parsed from `<Properties>`. Empty when none is
    /// declared on this paragraph (the cascade fills in from the
    /// applied paragraph style if available).
    pub tab_list: Vec<TabStop>,
    pub runs: Vec<CharacterRun>,
    /// `<Table>` nested inside the paragraph's CharacterStyleRange.
    /// When present, the paragraph is rendered as a table at the
    /// current y_cursor; `runs` is typically empty for these.
    /// Tables can't currently nest inside tables — only one per
    /// paragraph.
    pub table: Option<Table>,
}

/// `<Table>` element parsed from a Story. Cells reference rows /
/// columns by their `Name` (the IDML index, "0"..n-1). Cells in
/// `cells` are stored in document order — IDML serialises them
/// column-major (all cells in column 0, then column 1, etc.).
#[derive(Debug, Default, Clone, Serialize)]
pub struct Table {
    pub self_id: Option<String>,
    pub header_row_count: u32,
    pub footer_row_count: u32,
    pub body_row_count: u32,
    pub column_count: u32,
    /// `AppliedTableStyle="TableStyle/..."` reference. Currently
    /// recorded; cell rendering uses TextTopInset etc. directly off
    /// the cell rather than resolving styles.
    pub applied_table_style: Option<String>,
    pub rows: Vec<TableRow>,
    pub columns: Vec<TableColumn>,
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct TableRow {
    pub self_id: Option<String>,
    /// IDML index ("0" .. row_count - 1).
    pub name: Option<String>,
    pub single_row_height: Option<f32>,
    pub minimum_height: Option<f32>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct TableColumn {
    pub self_id: Option<String>,
    pub name: Option<String>,
    pub single_column_width: Option<f32>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct TableCell {
    pub self_id: Option<String>,
    /// `Name="col:row"` (zero-indexed). The `row()` and `column()`
    /// helpers parse this.
    pub name: Option<String>,
    pub row_span: u32,
    pub column_span: u32,
    pub text_top_inset: f32,
    pub text_left_inset: f32,
    pub text_bottom_inset: f32,
    pub text_right_inset: f32,
    pub applied_cell_style: Option<String>,
    /// Cell content — paragraphs, parsed identically to top-level
    /// story paragraphs.
    pub paragraphs: Vec<Paragraph>,
}

impl TableCell {
    /// Parse `(column, row)` from `Name`. Returns `None` if the
    /// attribute is absent or doesn't match `col:row`.
    pub fn coords(&self) -> Option<(u32, u32)> {
        let name = self.name.as_deref()?;
        let (c, r) = name.split_once(':')?;
        Some((c.parse().ok()?, r.parse().ok()?))
    }
}

/// One stop in a paragraph's `<TabList>`. Position is in pt from
/// the column's left edge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabStop {
    pub position: f32,
    /// IDML alignment string: `LeftAlign`, `RightAlign`,
    /// `CenterAlign`, `CharacterAlign`.
    pub alignment: Option<String>,
    /// `AlignmentCharacter` for `CharacterAlign` stops (rare).
    pub alignment_character: Option<String>,
    /// `Leader` string rendered in the tab gap.
    pub leader: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CharacterRun {
    pub character_style: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    /// `FillColor="Color/..."` on the CharacterStyleRange; resolved
    /// against `Graphic`.
    pub fill_color: Option<String>,
    /// `FillTint` percentage (0..=100). IDML semantics: 100% = use the
    /// swatch at full strength, lower values blend toward paper white.
    /// `-1` (or absent) means "use the swatch as-is" — translates to
    /// `None`. The renderer applies the tint after CMYK→RGB so the
    /// result matches InDesign's preview, where tints sit on top of
    /// the colour-managed pipeline.
    pub fill_tint: Option<f32>,
    /// `Tracking` in 1/1000 em (InDesign's unit — divide by 1000 to
    /// get the em fraction that should be added to every glyph's
    /// advance).
    pub tracking: Option<f32>,
    /// `Underline="true"` on the CharacterStyleRange.
    pub underline: Option<bool>,
    /// `StrikeThru="true"` on the CharacterStyleRange.
    pub strikethru: Option<bool>,
    pub text: String,
}

impl Story {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(false);

        let mut out = Story::default();
        let mut current_paragraph: Option<Paragraph> = None;
        let mut current_run: Option<CharacterRun> = None;
        let mut current_table: Option<Table> = None;
        let mut current_cell: Option<TableCell> = None;
        // While parsing inside a Cell, the outer-paragraph state
        // (the paragraph that *hosts* the table) is parked here so
        // cell paragraphs can use the same `current_paragraph` slot
        // without losing the outer's accumulated metadata.
        let mut outer_paragraph: Option<Paragraph> = None;
        let mut outer_run: Option<CharacterRun> = None;
        let mut in_content = false;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => match e.name().as_ref() {
                    b"ParagraphStyleRange" => {
                        current_paragraph = Some(Paragraph {
                            paragraph_style: attr(&e, b"AppliedParagraphStyle"),
                            justification: attr(&e, b"Justification"),
                            first_line_indent: attr(&e, b"FirstLineIndent")
                                .and_then(|s| s.parse().ok()),
                            space_before: attr(&e, b"SpaceBefore").and_then(|s| s.parse().ok()),
                            space_after: attr(&e, b"SpaceAfter").and_then(|s| s.parse().ok()),
                            tab_list: Vec::new(),
                            runs: Vec::new(),
                            table: None,
                        });
                    }
                    b"Table" => {
                        // Tables nest inside a CharacterStyleRange; the
                        // run that hosts the table is typically
                        // contentless, so we let it pass through as-is.
                        current_table = Some(Table {
                            self_id: attr(&e, b"Self"),
                            header_row_count: attr(&e, b"HeaderRowCount")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            footer_row_count: attr(&e, b"FooterRowCount")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            body_row_count: attr(&e, b"BodyRowCount")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            column_count: attr(&e, b"ColumnCount")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            applied_table_style: attr(&e, b"AppliedTableStyle"),
                            rows: Vec::new(),
                            columns: Vec::new(),
                            cells: Vec::new(),
                        });
                    }
                    b"Cell" => {
                        // Park outer paragraph/run so cell content
                        // can re-use the same slots without leaking.
                        outer_paragraph = current_paragraph.take();
                        outer_run = current_run.take();
                        current_cell = Some(TableCell {
                            self_id: attr(&e, b"Self"),
                            name: attr(&e, b"Name"),
                            row_span: attr(&e, b"RowSpan")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1),
                            column_span: attr(&e, b"ColumnSpan")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1),
                            text_top_inset: attr(&e, b"TextTopInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            text_left_inset: attr(&e, b"TextLeftInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            text_bottom_inset: attr(&e, b"TextBottomInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            text_right_inset: attr(&e, b"TextRightInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            applied_cell_style: attr(&e, b"AppliedCellStyle"),
                            paragraphs: Vec::new(),
                        });
                    }
                    b"TabStop" => {
                        // <TabStop Position="..." Alignment="..."/>
                        // appears nested inside <TabList><ListItem>.
                        // Append to the open paragraph's list.
                        if let Some(stop) = parse_tab_stop(&e) {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    b"CharacterStyleRange" => {
                        current_run = Some(CharacterRun {
                            character_style: attr(&e, b"AppliedCharacterStyle"),
                            font: attr(&e, b"AppliedFont"),
                            font_style: attr(&e, b"FontStyle"),
                            point_size: attr(&e, b"PointSize").and_then(|s| s.parse().ok()),
                            fill_color: attr(&e, b"FillColor"),
                            fill_tint: parse_tint_attr(&e, b"FillTint"),
                            tracking: attr(&e, b"Tracking").and_then(|s| s.parse().ok()),
                            underline: attr(&e, b"Underline").and_then(|s| s.parse::<bool>().ok()),
                            strikethru: attr(&e, b"StrikeThru")
                                .and_then(|s| s.parse::<bool>().ok()),
                            text: String::new(),
                        });
                    }
                    b"Content" => {
                        in_content = true;
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"Content" => {
                        in_content = false;
                    }
                    b"CharacterStyleRange" => {
                        if let (Some(run), Some(para)) =
                            (current_run.take(), current_paragraph.as_mut())
                        {
                            if !run.text.is_empty() {
                                para.runs.push(run);
                            }
                        }
                    }
                    b"ParagraphStyleRange" => {
                        if let Some(para) = current_paragraph.take() {
                            // Keep paragraphs that have either a
                            // shaped run or a hosted table; drop
                            // truly empty ones.
                            if !para.runs.is_empty() || para.table.is_some() {
                                if let Some(cell) = current_cell.as_mut() {
                                    cell.paragraphs.push(para);
                                } else {
                                    out.paragraphs.push(para);
                                }
                            }
                        }
                    }
                    b"Cell" => {
                        if let (Some(cell), Some(table)) =
                            (current_cell.take(), current_table.as_mut())
                        {
                            table.cells.push(cell);
                        }
                        // Restore the outer paragraph/run state so
                        // the next Cell or the closing Table sees
                        // the host paragraph again.
                        current_paragraph = outer_paragraph.take();
                        current_run = outer_run.take();
                    }
                    b"Table" => {
                        // Attach the parsed table to its host
                        // paragraph. The host's runs are typically
                        // empty; the ParagraphStyleRange close
                        // above keeps it because table.is_some().
                        if let Some(table) = current_table.take() {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.table = Some(table);
                            }
                        }
                    }
                    _ => {}
                },
                Event::Empty(e) => match e.name().as_ref() {
                    // Line breaks inside a paragraph surface as <Br/> — treat
                    // them as a logical newline in the current run.
                    b"Br" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\n');
                        }
                    }
                    // Tab characters surface as <Tab/>; the layout
                    // pass treats '\t' as wide whitespace until a
                    // proper TabList-aware breaker lands.
                    b"Tab" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\t');
                        }
                    }
                    // Self-closing <TabStop .../> inside the
                    // paragraph's TabList.
                    b"TabStop" => {
                        if let Some(stop) = parse_tab_stop(&e) {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    // <Row Self="..." Name="..." SingleRowHeight="..."/>
                    b"Row" => {
                        if let Some(table) = current_table.as_mut() {
                            table.rows.push(TableRow {
                                self_id: attr(&e, b"Self"),
                                name: attr(&e, b"Name"),
                                single_row_height: attr(&e, b"SingleRowHeight")
                                    .and_then(|s| s.parse().ok()),
                                minimum_height: attr(&e, b"MinimumHeight")
                                    .and_then(|s| s.parse().ok()),
                            });
                        }
                    }
                    // <Column Self="..." Name="..." SingleColumnWidth="..."/>
                    b"Column" => {
                        if let Some(table) = current_table.as_mut() {
                            table.columns.push(TableColumn {
                                self_id: attr(&e, b"Self"),
                                name: attr(&e, b"Name"),
                                single_column_width: attr(&e, b"SingleColumnWidth")
                                    .and_then(|s| s.parse().ok()),
                            });
                        }
                    }
                    _ => {}
                },
                Event::Text(t) => {
                    if in_content {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push_str(&t.unescape().unwrap_or_default());
                        }
                    }
                }
                Event::PI(pi) => {
                    // InDesign serialises auto-page-number markers
                    // inside <Content> as `<?ACE 18?>` processing
                    // instructions. Map them to private-use chars
                    // so the renderer can substitute the actual
                    // page number per emission. ACE 18 is the
                    // current-page-number marker; ACE 19 is the
                    // next-page-number marker.
                    if in_content {
                        if let Some(run) = current_run.as_mut() {
                            let body = pi.as_ref();
                            let body_str = std::str::from_utf8(body).unwrap_or("");
                            if body_str.trim_start().starts_with("ACE 18") {
                                run.text.push(AUTO_PAGE_NUMBER_MARKER);
                            } else if body_str.trim_start().starts_with("ACE 19") {
                                run.text.push(NEXT_PAGE_NUMBER_MARKER);
                            }
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }
}

fn parse_tab_stop(e: &quick_xml::events::BytesStart) -> Option<TabStop> {
    let position = attr(e, b"Position").and_then(|s| s.parse::<f32>().ok())?;
    Some(TabStop {
        position,
        alignment: attr(e, b"Alignment"),
        alignment_character: attr(e, b"AlignmentCharacter"),
        leader: attr(e, b"Leader"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"
                           AppliedFont="Minion Pro" PointSize="11">
        <Content>Hello, </Content>
      </CharacterStyleRange>
      <CharacterStyleRange FontStyle="Bold" AppliedFont="Minion Pro" PointSize="11">
        <Content>world</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Minion Pro" PointSize="11">
        <Content>.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Minion Pro" PointSize="11">
        <Content>Second paragraph.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;

    #[test]
    fn extracts_paragraphs_and_runs() {
        let s = Story::parse(SAMPLE).unwrap();
        assert_eq!(s.paragraphs.len(), 2);

        let p1 = &s.paragraphs[0];
        assert_eq!(p1.paragraph_style.as_deref(), Some("ParagraphStyle/Body"));
        assert_eq!(p1.runs.len(), 3);
        assert_eq!(p1.runs[0].text, "Hello, ");
        assert_eq!(p1.runs[1].text, "world");
        assert_eq!(p1.runs[1].font_style.as_deref(), Some("Bold"));
        assert_eq!(p1.runs[1].point_size, Some(11.0));
        assert_eq!(p1.runs[2].text, ".");

        let p2 = &s.paragraphs[1];
        assert_eq!(p2.runs.len(), 1);
        assert_eq!(p2.runs[0].text, "Second paragraph.");
    }

    #[test]
    fn br_becomes_newline_in_run_text() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>line one</Content>
              <Br/>
              <Content>line two</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = Story::parse(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs[0].text, "line one\nline two");
    }

    #[test]
    fn tab_element_becomes_tab_character() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>name</Content>
              <Tab/>
              <Content>value</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = Story::parse(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs[0].text, "name\tvalue");
    }

    #[test]
    fn tab_list_attaches_to_paragraph() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <Properties>
              <TabList>
                <ListItem><TabStop Position="36" Alignment="LeftAlign"/></ListItem>
                <ListItem><TabStop Position="144" Alignment="RightAlign" Leader="."/></ListItem>
              </TabList>
            </Properties>
            <CharacterStyleRange>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = Story::parse(xml).unwrap();
        let stops = &s.paragraphs[0].tab_list;
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].position, 36.0);
        assert_eq!(stops[0].alignment.as_deref(), Some("LeftAlign"));
        assert_eq!(stops[1].position, 144.0);
        assert_eq!(stops[1].leader.as_deref(), Some("."));
    }

    #[test]
    fn parses_table_with_rows_columns_and_cells() {
        // Mirrors the IDML serialisation: a Table nested in a
        // CharacterStyleRange, with Row/Column/Cell siblings inside
        // the Table. Each cell carries its own paragraph + run.
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange>
              <CharacterStyleRange>
                <Table Self="t1" HeaderRowCount="1" BodyRowCount="2" ColumnCount="2"
                       AppliedTableStyle="TableStyle/Demo">
                  <Row Self="r0" Name="0" SingleRowHeight="20"/>
                  <Row Self="r1" Name="1" SingleRowHeight="18"/>
                  <Column Self="c0" Name="0" SingleColumnWidth="100"/>
                  <Column Self="c1" Name="1" SingleColumnWidth="60"/>
                  <Cell Self="cell00" Name="0:0" RowSpan="1" ColumnSpan="1"
                        TextTopInset="2" TextLeftInset="3"
                        TextBottomInset="2" TextRightInset="3">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Header A</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell10" Name="1:0">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Header B</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell01" Name="0:1">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Body A1</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell11" Name="1:1">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Body B1</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = Story::parse(xml).unwrap();
        assert_eq!(s.paragraphs.len(), 1, "table-host paragraph kept");
        let table = s.paragraphs[0]
            .table
            .as_ref()
            .expect("paragraph hosts a table");
        assert_eq!(table.column_count, 2);
        assert_eq!(table.body_row_count, 2);
        assert_eq!(table.header_row_count, 1);
        assert_eq!(
            table.applied_table_style.as_deref(),
            Some("TableStyle/Demo")
        );
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].single_row_height, Some(20.0));
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.columns[0].single_column_width, Some(100.0));
        assert_eq!(table.cells.len(), 4);
        assert_eq!(table.cells[0].coords(), Some((0, 0)));
        assert_eq!(table.cells[3].coords(), Some((1, 1)));
        // Cell content lives in cell.paragraphs.
        let header_a = &table.cells[0].paragraphs[0].runs[0].text;
        assert_eq!(header_a, "Header A");
        let body_b1 = &table.cells[3].paragraphs[0].runs[0].text;
        assert_eq!(body_b1, "Body B1");
        // Cell insets carried through.
        assert_eq!(table.cells[0].text_top_inset, 2.0);
        assert_eq!(table.cells[0].text_left_inset, 3.0);
    }
}
