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

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

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
