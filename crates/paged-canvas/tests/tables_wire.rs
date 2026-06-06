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

//! W3.A1 — table NodeId surface wire tests.
//!
//!   1. `hit_test` into a table cell returns the `(tableId, row, col)`
//!      table context alongside the frame hit.
//!   2. `element_properties` on a `TableCell` address returns the
//!      cell's fill / inset / vertical-justify / applied-style entries;
//!      on a `Table` address returns the applied-table-style entry.
//!   3. a cell-fill `SetElementProperty` mutation applies + the read
//!      reflects it (the full wire round-trip).

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::channel::Mutation;
use paged_canvas::element_selection::ElementId;
use paged_canvas::{CanvasModel, CanvasOptions, PageId};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name))
        .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// A single-page IDML whose `Story/u10` carries a 2×2 table `Table/t1`
/// in a `TextFrame/frameA`. Column widths are 100 / 60 pt, row heights
/// 30 / 40 pt, so cell rects are predictable for the hit-test. The
/// frame sits at page-local `(40, 40)` (top-left), and the table
/// starts at the frame's inner top-left.
fn build_tables_idml() -> Vec<u8> {
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();

    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 380 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
        <Table Self="t1" HeaderRowCount="0" BodyRowCount="2" ColumnCount="2">
          <Row Self="r0" Name="0" SingleRowHeight="30"/>
          <Row Self="r1" Name="1" SingleRowHeight="40"/>
          <Column Self="c0" Name="0" SingleColumnWidth="100"/>
          <Column Self="c1" Name="1" SingleColumnWidth="60"/>
          <Cell Self="cell00" Name="0:0" FillColor="Color/A">
            <ParagraphStyleRange><CharacterStyleRange><Content>A</Content></CharacterStyleRange></ParagraphStyleRange>
          </Cell>
          <Cell Self="cell10" Name="1:0" FillColor="Color/B">
            <ParagraphStyleRange><CharacterStyleRange><Content>B</Content></CharacterStyleRange></ParagraphStyleRange>
          </Cell>
          <Cell Self="cell01" Name="0:1" FillColor="Color/C">
            <ParagraphStyleRange><CharacterStyleRange><Content>C</Content></CharacterStyleRange></ParagraphStyleRange>
          </Cell>
          <Cell Self="cell11" Name="1:1" FillColor="Color/D">
            <ParagraphStyleRange><CharacterStyleRange><Content>D</Content></CharacterStyleRange></ParagraphStyleRange>
          </Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

fn load_model() -> CanvasModel {
    let bytes = build_tables_idml();
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("tables", &bytes, opts).expect("load + build")
}

#[test]
fn hit_test_into_a_cell_returns_table_context() {
    let model = load_model();
    let page_id = PageId("p1".into());

    // Table origin = frame inner top-left = page-local (40, 40).
    // Column 0 spans x in [40, 140), column 1 in [140, 200).
    // Row 0 spans y in [40, 70), row 1 in [70, 110).

    // Click well inside cell (row 0, col 0).
    let hit = model.hit_test(&page_id, (60.0, 50.0));
    assert_eq!(hit.frame_id.as_deref(), Some("frameA"));
    let tc = hit.table_context.expect("cell (0,0) context");
    assert_eq!(tc.table_id, "t1");
    assert_eq!((tc.row, tc.col), (0, 0));

    // Click inside cell (row 1, col 1).
    let hit = model.hit_test(&page_id, (170.0, 90.0));
    let tc = hit.table_context.expect("cell (1,1) context");
    assert_eq!((tc.row, tc.col), (1, 1));

    // Click inside cell (row 1, col 0).
    let hit = model.hit_test(&page_id, (60.0, 90.0));
    let tc = hit.table_context.expect("cell (1,0) context");
    assert_eq!((tc.row, tc.col), (1, 0));
}

#[test]
fn hit_test_in_frame_below_table_has_no_cell_context() {
    let model = load_model();
    let page_id = PageId("p1".into());
    // Below the table (y past row 1's bottom at 110) but still in the
    // frame — the hit resolves the frame but no cell.
    let hit = model.hit_test(&page_id, (60.0, 300.0));
    assert_eq!(hit.frame_id.as_deref(), Some("frameA"));
    assert!(hit.table_context.is_none());
}

#[test]
fn cell_property_read_entries() {
    let model = load_model();
    let id = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 0,
        col: 1,
    };
    let props = model.element_properties(&id).expect("cell props");
    assert_eq!(props.kind, "TableCell");
    // The inline FillColor on cell 1:0 is Color/B.
    let fill = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::CellFillColor)
        .and_then(|e| e.value.clone());
    assert_eq!(fill, Some(paged_mutate::Value::ColorRef(Some("Color/B".into()))));
    // The inset + vjust + applied-style entries are present.
    for p in [
        paged_mutate::PropertyPath::CellInsetTop,
        paged_mutate::PropertyPath::CellVerticalJustification,
        paged_mutate::PropertyPath::AppliedCellStyle,
    ] {
        assert!(
            props.entries.iter().any(|e| e.path == p),
            "missing entry {p:?}"
        );
    }
}

#[test]
fn table_property_read_entry() {
    let model = load_model();
    let id = ElementId::Table {
        story_id: "u10".into(),
        table_id: "t1".into(),
    };
    let props = model.element_properties(&id).expect("table props");
    assert_eq!(props.kind, "Table");
    assert!(props
        .entries
        .iter()
        .any(|e| e.path == paged_mutate::PropertyPath::AppliedTableStyle));
}

#[test]
fn cell_fill_mutation_round_trips_through_the_wire() {
    let mut model = load_model();
    let id = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 0,
        col: 0,
    };
    model
        .apply_mutation(&Mutation::SetElementProperty {
            element_id: id.clone(),
            path: paged_mutate::PropertyPath::CellFillColor,
            value: paged_mutate::Value::ColorRef(Some("Color/Z".into())),
        })
        .expect("cell fill mutation");
    let props = model.element_properties(&id).expect("re-read cell props");
    let fill = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::CellFillColor)
        .and_then(|e| e.value.clone());
    assert_eq!(fill, Some(paged_mutate::Value::ColorRef(Some("Color/Z".into()))));
}

#[test]
fn insert_table_row_mutation_applies() {
    let mut model = load_model();
    model
        .apply_mutation(&Mutation::InsertTableRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
            at: 1,
        })
        .expect("insert row mutation");
    // After insert, the new empty cell (row 1, col 0) reads with no
    // fill; the old row-1 cell content shifted to row 2.
    let props = model
        .element_properties(&ElementId::TableCell {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 2,
            col: 0,
        })
        .expect("shifted cell props");
    let fill = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::CellFillColor)
        .and_then(|e| e.value.clone());
    // Original (0,1) cell carried Color/C; it is now at row 2.
    assert_eq!(fill, Some(paged_mutate::Value::ColorRef(Some("Color/C".into()))));
}
