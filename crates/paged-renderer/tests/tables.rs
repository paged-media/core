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

//! Table render tests at the display-list level. Hand-construct a
//! minimal IDML carrying a `<TableStyle>` with alternating fills and a
//! `<Cell>` with diagonal strokes, then assert the emitted FillPath /
//! StrokePath commands. No fonts are loaded, so cell text doesn't
//! shape — the only commands the table emits are the cell-background
//! fills (FillPath) and the diagonal lines (StrokePath), which keeps
//! the count assertions deterministic.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Build a 2-column × 3-row table IDML. `table_attrs` is spliced onto
/// the `<Table>` element (e.g. `AppliedTableStyle="TableStyle/Alt"`)
/// and `cell00_attrs` onto the first `<Cell>` (for diagonals).
fn build_table_idml(styles_body: &str, table_attrs: &str, cell00_attrs: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let put = |zip: &mut ZipWriter<_>, name: &str, body: &[u8]| {
        zip.start_file(name, deflated).unwrap();
        zip.write_all(body).unwrap();
    };

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();

    put(
        &mut zip,
        "designmap.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    );

    put(
        &mut zip,
        "Resources/Graphic.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Cyan" Name="Cyan" Space="CMYK" ColorValue="100 0 0 0"/>
    <Color Self="Color/Magenta" Name="Magenta" Space="CMYK" ColorValue="0 100 0 0"/>
  </Graphic>
</idPkg:Graphic>"#,
    );

    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
{styles_body}
</idPkg:Styles>"#
    );
    put(&mut zip, "Resources/Styles.xml", styles.as_bytes());

    put(
        &mut zip,
        "Spreads/Spread_sp1.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="0 0 400 400"
               FillColor="Swatch/None" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    );

    // 2 columns × 3 rows, column-major cells. Cell 0:0 receives the
    // optional diagonal attributes.
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Table Self="t1" BodyRowCount="3" ColumnCount="2" {table_attrs}>
          <Row Self="r0" Name="0" SingleRowHeight="30"/>
          <Row Self="r1" Name="1" SingleRowHeight="30"/>
          <Row Self="r2" Name="2" SingleRowHeight="30"/>
          <Column Self="c0" Name="0" SingleColumnWidth="100"/>
          <Column Self="c1" Name="1" SingleColumnWidth="100"/>
          <Cell Self="c00" Name="0:0" {cell00_attrs}><ParagraphStyleRange><CharacterStyleRange><Content>a</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c01" Name="0:1"><ParagraphStyleRange><CharacterStyleRange><Content>b</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c02" Name="0:2"><ParagraphStyleRange><CharacterStyleRange><Content>c</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c10" Name="1:0"><ParagraphStyleRange><CharacterStyleRange><Content>d</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c11" Name="1:1"><ParagraphStyleRange><CharacterStyleRange><Content>e</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c12" Name="1:2"><ParagraphStyleRange><CharacterStyleRange><Content>f</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    put(&mut zip, "Stories/Story_u10.xml", story.as_bytes());

    zip.finish().unwrap().into_inner()
}

/// Build via the full `build_document` path (the one the inspect CLI
/// and fidelity harness use) — it emits story content + tables, unlike
/// the legacy `pipeline::build` which only paints frame rectangles.
/// Returns page 0's display-list commands.
fn build_commands(bytes: &[u8]) -> Vec<DisplayCommand> {
    let document = Document::open(bytes).unwrap();
    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&document, &opts).unwrap();
    built.pages[0].list.commands.clone()
}

fn count_fills(cmds: &[DisplayCommand]) -> usize {
    cmds.iter()
        .filter(|c| matches!(c, DisplayCommand::FillPath { .. }))
        .count()
}

fn count_strokes(cmds: &[DisplayCommand]) -> usize {
    cmds.iter()
        .filter(|c| matches!(c, DisplayCommand::StrokePath { .. }))
        .count()
}

#[test]
fn alternating_row_fill_emits_fill_per_filled_body_row() {
    // AlternatingRows with a 1-row start (Cyan) / 1-row end (None)
    // cycle over 3 body rows → rows 0 and 2 are Cyan, row 1 is None.
    // The frame itself paints no fill (Swatch/None), so the only
    // FillPath commands are the two cyan row backgrounds.
    let styles = r#"  <RootTableStyleGroup>
    <TableStyle Self="TableStyle/Alt" Name="Alt"
                AlternatingFills="AlternatingRows"
                StartRowFillColor="Color/Cyan" StartRowFillCount="1"
                EndRowFillColor="Swatch/None" EndRowFillCount="1"/>
  </RootTableStyleGroup>"#;
    let bytes = build_table_idml(styles, r#"AppliedTableStyle="TableStyle/Alt""#, "");
    let cmds = build_commands(&bytes);
    assert_eq!(
        count_fills(&cmds),
        2,
        "two filled body rows (0 and 2), got {cmds:#?}"
    );
    assert_eq!(count_strokes(&cmds), 0, "no diagonals in this table");
}

#[test]
fn alternating_column_fill_emits_fill_per_filled_column() {
    // AlternatingColumns: column 0 Cyan, column 1 None. The fill spans
    // all 3 physical rows → 3 row-segments × 1 filled column = 3 fills.
    let styles = r#"  <RootTableStyleGroup>
    <TableStyle Self="TableStyle/AltC" Name="AltC"
                AlternatingFills="AlternatingColumns"
                StartColumnFillColor="Color/Cyan" StartColumnFillCount="1"
                EndColumnFillColor="Swatch/None" EndColumnFillCount="1"/>
  </RootTableStyleGroup>"#;
    let bytes = build_table_idml(styles, r#"AppliedTableStyle="TableStyle/AltC""#, "");
    let cmds = build_commands(&bytes);
    assert_eq!(
        count_fills(&cmds),
        3,
        "one filled column over 3 row segments, got {cmds:#?}"
    );
}

#[test]
fn no_alternating_fill_without_discriminator_for_columns() {
    // Column fills require the explicit AlternatingColumns
    // discriminator — a bare StartColumnFillColor must NOT paint.
    let styles = r#"  <RootTableStyleGroup>
    <TableStyle Self="TableStyle/Bare" Name="Bare"
                StartColumnFillColor="Color/Cyan" StartColumnFillCount="1"
                EndColumnFillColor="Swatch/None" EndColumnFillCount="1"/>
  </RootTableStyleGroup>"#;
    let bytes = build_table_idml(styles, r#"AppliedTableStyle="TableStyle/Bare""#, "");
    let cmds = build_commands(&bytes);
    assert_eq!(count_fills(&cmds), 0);
}

#[test]
fn cell_diagonal_emits_stroke_paths() {
    // Cell 0:0 carries both diagonals (an X). The renderer emits one
    // StrokePath per drawn diagonal → 2 strokes. No table style, so no
    // fills.
    let styles = r#"  <RootTableStyleGroup>
    <TableStyle Self="TableStyle/$ID/[No table style]" Name="$ID/[No table style]"/>
  </RootTableStyleGroup>"#;
    let cell_attrs = r#"LeftLineDrawn="true" LeftLineStrokeColor="Color/Magenta" LeftLineStrokeWeight="1.5"
        RightLineDrawn="true" RightLineStrokeColor="Color/Magenta" RightLineStrokeWeight="1.5""#;
    let bytes = build_table_idml(styles, "", cell_attrs);
    let cmds = build_commands(&bytes);
    assert_eq!(count_strokes(&cmds), 2, "two diagonals, got {cmds:#?}");
    // Stroke weight carried through to the display command.
    for c in cmds.iter() {
        if let DisplayCommand::StrokePath { stroke, .. } = c {
            assert_eq!(stroke.width, 1.5);
        }
    }
}

#[test]
fn diagonal_in_front_paints_after_cell_fill() {
    // With a cell FillColor + an in-front diagonal, the StrokePath must
    // land AFTER the cell's FillPath in command order so it paints over
    // the background (and, in the real case, the glyphs).
    let styles = r#"  <RootTableStyleGroup>
    <TableStyle Self="TableStyle/$ID/[No table style]" Name="$ID/[No table style]"/>
  </RootTableStyleGroup>"#;
    let cell_attrs = r#"FillColor="Color/Cyan"
        LeftLineDrawn="true" LeftLineStrokeColor="Color/Magenta" LeftLineStrokeWeight="2"
        DiagonalLineInFront="true""#;
    let bytes = build_table_idml(styles, "", cell_attrs);
    let cmds = build_commands(&bytes);
    let first_fill = cmds
        .iter()
        .position(|c| matches!(c, DisplayCommand::FillPath { .. }))
        .expect("cell fill present");
    let stroke = cmds
        .iter()
        .position(|c| matches!(c, DisplayCommand::StrokePath { .. }))
        .expect("diagonal present");
    assert!(
        stroke > first_fill,
        "in-front diagonal must paint after the cell fill (fill at {first_fill}, stroke at {stroke})"
    );
}

// ── W1.11a — cell vertical justification rendering ──────────────────

/// Build a 1×1 table whose single cell is 200pt tall (lots of vertical
/// slack for a single 24pt text line) carrying the supplied
/// `VerticalJustification` inline on the `<Cell>`. The cell text uses
/// the Inter fixture so glyphs actually shape and the y-shift is
/// observable. `cell_paragraphs` lets a JustifyAlign test pass two
/// paragraphs (the distribute path needs ≥ 2).
fn build_vjust_table_idml(vjust: Option<&str>, cell_paragraphs: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let put = |zip: &mut ZipWriter<_>, name: &str, body: &[u8]| {
        zip.start_file(name, deflated).unwrap();
        zip.write_all(body).unwrap();
    };
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    put(
        &mut zip,
        "designmap.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    );
    put(
        &mut zip,
        "Resources/Graphic.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Graphic/></idPkg:Graphic>"#,
    );
    put(
        &mut zip,
        "Spreads/Spread_sp1.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="0 0 400 400" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    );
    let vj_attr = vjust
        .map(|v| format!(" VerticalJustification=\"{v}\""))
        .unwrap_or_default();
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Table Self="t1" BodyRowCount="1" ColumnCount="1">
          <Row Self="r0" Name="0" SingleRowHeight="200"/>
          <Column Self="c0" Name="0" SingleColumnWidth="200"/>
          <Cell Self="c00" Name="0:0" TextTopInset="0" TextBottomInset="0"{vj_attr}>{cell_paragraphs}</Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    put(&mut zip, "Stories/Story_u10.xml", story.as_bytes());
    zip.finish().unwrap().into_inner()
}

const VJUST_PARAGRAPH: &str = r#"<ParagraphStyleRange><CharacterStyleRange><Properties><AppliedFont type="string">Inter</AppliedFont></Properties><Content>Hi</Content></CharacterStyleRange></ParagraphStyleRange>"#;

/// Minimum glyph baseline (`ty`) across all FillPath commands — the top
/// of the rendered text block. Smaller = higher on the page.
fn min_glyph_ty(cmds: &[DisplayCommand]) -> f32 {
    cmds.iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0[5]),
            _ => None,
        })
        .fold(f32::INFINITY, f32::min)
}

fn build_vjust_commands(vjust: Option<&str>, paragraphs: &str) -> Vec<DisplayCommand> {
    let bytes = build_vjust_table_idml(vjust, paragraphs);
    let document = Document::open(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).unwrap();
    built.pages[0].list.commands.clone()
}

#[test]
fn cell_vjust_bottom_pushes_glyphs_below_top_align() {
    // A single 24pt line in a 200pt-tall cell: TopAlign keeps it at the
    // top; BottomAlign shifts it down by the full slack. The minimum
    // glyph baseline must be strictly larger (lower on the page) under
    // BottomAlign.
    let top = build_vjust_commands(Some("TopAlign"), VJUST_PARAGRAPH);
    let bottom = build_vjust_commands(Some("BottomAlign"), VJUST_PARAGRAPH);
    let top_y = min_glyph_ty(&top);
    let bottom_y = min_glyph_ty(&bottom);
    assert!(top_y.is_finite() && bottom_y.is_finite(), "glyphs emitted");
    assert!(
        bottom_y > top_y + 50.0,
        "BottomAlign must drop the text well below TopAlign (top {top_y}, bottom {bottom_y})"
    );
}

#[test]
fn cell_vjust_center_lands_between_top_and_bottom() {
    let top = min_glyph_ty(&build_vjust_commands(Some("TopAlign"), VJUST_PARAGRAPH));
    let center = min_glyph_ty(&build_vjust_commands(Some("CenterAlign"), VJUST_PARAGRAPH));
    let bottom = min_glyph_ty(&build_vjust_commands(Some("BottomAlign"), VJUST_PARAGRAPH));
    assert!(
        center > top + 10.0 && center < bottom - 10.0,
        "CenterAlign sits between Top and Bottom (top {top}, center {center}, bottom {bottom})"
    );
}

#[test]
fn cell_vjust_absent_matches_top_align() {
    // No inline VerticalJustification + no cell style ⇒ default Top.
    let none = min_glyph_ty(&build_vjust_commands(None, VJUST_PARAGRAPH));
    let top = min_glyph_ty(&build_vjust_commands(Some("TopAlign"), VJUST_PARAGRAPH));
    assert!(
        (none - top).abs() < 0.01,
        "absent vjust must equal TopAlign (none {none}, top {top})"
    );
}

#[test]
fn cell_vjust_justify_distributes_between_paragraphs() {
    // Two paragraphs in a tall cell: JustifyAlign keeps the first at the
    // top (matching TopAlign's first line) but pushes the SECOND down to
    // the bottom, so the overall span grows. The max glyph baseline must
    // exceed the TopAlign max by most of the slack.
    let two = format!("{VJUST_PARAGRAPH}{VJUST_PARAGRAPH}");
    let top = build_vjust_commands(Some("TopAlign"), &two);
    let justify = build_vjust_commands(Some("JustifyAlign"), &two);
    let max_ty = |cmds: &[DisplayCommand]| {
        cmds.iter()
            .filter_map(|c| match c {
                DisplayCommand::FillPath { transform, .. } => Some(transform.0[5]),
                _ => None,
            })
            .fold(f32::NEG_INFINITY, f32::max)
    };
    // First-line top must match (Justify doesn't move the first paragraph).
    assert!(
        (min_glyph_ty(&top) - min_glyph_ty(&justify)).abs() < 0.01,
        "JustifyAlign keeps the first paragraph at the top"
    );
    // Last-line bottom must drop well past the TopAlign stacked position.
    assert!(
        max_ty(&justify) > max_ty(&top) + 50.0,
        "JustifyAlign must push the last paragraph toward the bottom (top max {}, justify max {})",
        max_ty(&top),
        max_ty(&justify)
    );
}

// ── W1.12b — renderer honours RowSpan / ColumnSpan ──────────────────

/// Build a 2-column × 2-row table where cell 0:0 carries the supplied
/// span attributes + a Cyan fill (so the spanning cell paints a fill
/// whose rect we can measure). Each column is 100pt wide, each row 30pt
/// tall, so a `ColumnSpan="2"` fill must be ~200pt wide and a
/// `RowSpan="2"` fill ~60pt tall.
fn build_span_table_idml(cell00_span_attrs: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let put = |zip: &mut ZipWriter<_>, name: &str, body: &[u8]| {
        zip.start_file(name, deflated).unwrap();
        zip.write_all(body).unwrap();
    };
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    put(
        &mut zip,
        "designmap.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    );
    put(
        &mut zip,
        "Resources/Graphic.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic><Color Self="Color/Cyan" Name="Cyan" Space="CMYK" ColorValue="100 0 0 0"/></Graphic>
</idPkg:Graphic>"#,
    );
    put(
        &mut zip,
        "Spreads/Spread_sp1.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="0 0 400 400" FillColor="Swatch/None" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    );
    // Cell 0:0 takes the span attrs; the covered slot (the cell the span
    // overlaps) is omitted, IDML's column-major span convention.
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Table Self="t1" BodyRowCount="2" ColumnCount="2">
          <Row Self="r0" Name="0" SingleRowHeight="30"/>
          <Row Self="r1" Name="1" SingleRowHeight="30"/>
          <Column Self="c0" Name="0" SingleColumnWidth="100"/>
          <Column Self="c1" Name="1" SingleColumnWidth="100"/>
          <Cell Self="c00" Name="0:0" FillColor="Color/Cyan" {cell00_span_attrs}><ParagraphStyleRange><CharacterStyleRange><Content>X</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c01" Name="0:1"><ParagraphStyleRange><CharacterStyleRange><Content>Y</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="c11" Name="1:1"><ParagraphStyleRange><CharacterStyleRange><Content>Z</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    put(&mut zip, "Stories/Story_u10.xml", story.as_bytes());
    zip.finish().unwrap().into_inner()
}

/// The (width, height) of the first FillPath rect — for `emit_rect` the
/// transform encodes the rect as `[w, 0, 0, h, x, y]`, so `.0[0]` is the
/// width and `.0[3]` the height in pt.
fn first_fill_dims(cmds: &[DisplayCommand]) -> (f32, f32) {
    cmds.iter()
        .find_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some((transform.0[0], transform.0[3])),
            _ => None,
        })
        .expect("a cell fill present")
}

#[test]
fn cell_column_span_widens_fill_rect() {
    // ColumnSpan="2" → the spanning cell's fill spans both 100pt columns.
    let bytes = build_span_table_idml(r#"ColumnSpan="2""#);
    let cmds = build_commands(&bytes);
    let (w, h) = first_fill_dims(&cmds);
    assert!(
        (w - 200.0).abs() < 0.5,
        "column-span fill must be ~200pt wide, got {w}"
    );
    assert!((h - 30.0).abs() < 0.5, "single-row height ~30pt, got {h}");
}

#[test]
fn cell_row_span_lengthens_fill_rect() {
    // RowSpan="2" → the spanning cell's fill covers both 30pt rows.
    let bytes = build_span_table_idml(r#"RowSpan="2""#);
    let cmds = build_commands(&bytes);
    let (w, h) = first_fill_dims(&cmds);
    assert!(
        (w - 100.0).abs() < 0.5,
        "single-column width ~100pt, got {w}"
    );
    assert!(
        (h - 60.0).abs() < 0.5,
        "row-span fill must be ~60pt tall, got {h}"
    );
}

#[test]
fn cell_no_span_is_single_cell() {
    // Baseline: no span → fill is exactly one 100×30 cell.
    let bytes = build_span_table_idml("");
    let cmds = build_commands(&bytes);
    let (w, h) = first_fill_dims(&cmds);
    assert!(
        (w - 100.0).abs() < 0.5 && (h - 30.0).abs() < 0.5,
        "1×1 cell"
    );
}

// ---- W1.13 — cell text addressing in StoryLayout (renderer) ---------

/// Build a 2×1 table (2 rows, 1 col) whose cells carry Inter-shaped
/// text so cell `LineLayout`s are captured with cluster positions.
fn build_cell_text_table_idml() -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let put = |zip: &mut ZipWriter<_>, name: &str, body: &[u8]| {
        zip.start_file(name, deflated).unwrap();
        zip.write_all(body).unwrap();
    };
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    put(
        &mut zip,
        "designmap.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    );
    put(
        &mut zip,
        "Resources/Graphic.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Graphic/></idPkg:Graphic>"#,
    );
    put(
        &mut zip,
        "Spreads/Spread_sp1.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="0 0 400 400" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    );
    let cell = |name: &str, text: &str| {
        let self_id = name.replace(':', "");
        format!(
            r#"<Cell Self="c{self_id}" Name="{name}" TextTopInset="0" TextBottomInset="0"><ParagraphStyleRange><CharacterStyleRange><Properties><AppliedFont type="string">Inter</AppliedFont></Properties><Content>{text}</Content></CharacterStyleRange></ParagraphStyleRange></Cell>"#,
        )
    };
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Table Self="tbl1" BodyRowCount="2" ColumnCount="1">
          <Row Self="r0" Name="0" SingleRowHeight="40"/>
          <Row Self="r1" Name="1" SingleRowHeight="40"/>
          <Column Self="col0" Name="0" SingleColumnWidth="200"/>
          {top}
          {bot}
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        top = cell("0:0", "top"),
        bot = cell("0:1", "bottom"),
    );
    put(&mut zip, "Stories/Story_u10.xml", story.as_bytes());
    zip.finish().unwrap().into_inner()
}

fn build_cell_doc() -> Document {
    Document::open(&build_cell_text_table_idml()).unwrap()
}

fn build_with_fonts(document: &Document) -> paged_renderer::BuiltDocument {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    pipeline::build_document(document, &opts).unwrap()
}

#[test]
fn cell_lines_carry_disjoint_cell_qualifier() {
    let document = build_cell_doc();
    let built = build_with_fonts(&document);
    let lines = built.story_layout("u10");
    // Every captured line is a cell line (the body holds only the
    // contentless table host), each tagged with its (table,row,col).
    let cell_lines: Vec<_> = lines.iter().filter(|l| l.cell.is_some()).collect();
    assert!(
        cell_lines.len() >= 2,
        "expected ≥2 cell lines (one per row), got {}",
        cell_lines.len()
    );
    let addr = |row, col| paged_renderer::CellAddr {
        table_id: "tbl1".into(),
        row,
        col,
    };
    let top = built.cell_layout("u10", &addr(0, 0));
    let bot = built.cell_layout("u10", &addr(1, 0));
    assert_eq!(top.len(), 1, "top cell is one line");
    assert_eq!(bot.len(), 1, "bottom cell is one line");
    // Cell-local paragraph index is 0 for each single-paragraph cell —
    // the collision the disjoint `cell` axis resolves.
    assert_eq!(top[0].paragraph_idx, 0);
    assert_eq!(bot[0].paragraph_idx, 0);
    // The two cells occupy distinct vertical bands.
    assert!(
        bot[0].baseline_y_pt > top[0].baseline_y_pt,
        "bottom cell sits below top cell"
    );
}

#[test]
fn edited_cell_paragraphs_relayout_into_more_lines() {
    // Baseline: top cell is one line. Inject a second paragraph into the
    // top cell's content (the kind of edit InsertText produces) and
    // rebuild — the cell must re-lay out into two lines.
    let mut document = build_cell_doc();
    let addr = paged_renderer::CellAddr {
        table_id: "tbl1".into(),
        row: 0,
        col: 0,
    };
    let before = build_with_fonts(&document).cell_layout("u10", &addr).len();
    assert_eq!(before, 1, "top cell starts as one line");

    // Mutate the parsed cell content: append a second paragraph.
    {
        let table = document.stories[0]
            .story
            .paragraphs
            .iter_mut()
            .find_map(|p| p.table.as_mut())
            .unwrap();
        let c = table
            .cells
            .iter_mut()
            .find(|c| c.coords() == Some((0, 0)))
            .unwrap();
        c.paragraphs.push(paged_parse::Paragraph {
            runs: vec![paged_parse::CharacterRun {
                text: "second".into(),
                font: Some("Inter".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    let after = build_with_fonts(&document).cell_layout("u10", &addr).len();
    assert!(
        after > before,
        "edited cell must re-lay out into more lines ({before} -> {after})"
    );
}
