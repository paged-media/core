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
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
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
    assert_eq!(
        fill,
        Some(paged_mutate::Value::ColorRef(Some("Color/B".into())))
    );
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
    assert_eq!(
        fill,
        Some(paged_mutate::Value::ColorRef(Some("Color/Z".into())))
    );
}

#[test]
fn table_dimension_read_entries() {
    // Aftercare-A — `element_properties` on a `Table` address now also
    // reports the read-only `tableRowCount` / `tableColumnCount`
    // (integer-as-Length). The fixture is a 2-row × 2-column table.
    let model = load_model();
    let id = ElementId::Table {
        story_id: "u10".into(),
        table_id: "t1".into(),
    };
    let props = model.element_properties(&id).expect("table props");
    let row_count = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::TableRowCount)
        .and_then(|e| e.value.clone());
    assert_eq!(row_count, Some(paged_mutate::Value::Length(Some(2.0))));
    let col_count = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::TableColumnCount)
        .and_then(|e| e.value.clone());
    assert_eq!(col_count, Some(paged_mutate::Value::Length(Some(2.0))));
}

#[test]
fn table_dimension_write_is_rejected() {
    // Aftercare-A — the dimension paths are read-only: a write routes
    // to `apply_table_property` and is rejected (the standard
    // read-only contract). The catch-all guard means the model errors.
    let mut model = load_model();
    let id = ElementId::Table {
        story_id: "u10".into(),
        table_id: "t1".into(),
    };
    let err = model.apply_mutation(&Mutation::SetElementProperty {
        element_id: id,
        path: paged_mutate::PropertyPath::TableRowCount,
        value: paged_mutate::Value::Length(Some(5.0)),
    });
    assert!(err.is_err(), "writing tableRowCount must be rejected");
}

#[test]
fn cell_geometry_returns_the_hit_test_rect() {
    // Aftercare-A — `element_geometry` on a `TableCell` resolves the
    // BuiltPage `cell_rects` entry the hit-test path uses. Cell (0,0)
    // sits at page-local (40, 40), 100pt wide × 30pt tall, so the
    // ElementGeometryItem bounds `[top, left, bottom, right]` are
    // `[40, 40, 70, 140]` and carry no item_transform (already
    // page-local).
    let model = load_model();
    let id = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 0,
        col: 0,
    };
    let items = model.element_geometry(std::slice::from_ref(&id));
    assert_eq!(items.len(), 1, "cell geometry resolves to one item");
    let item = &items[0];
    assert_eq!(item.page_id.as_str(), "p1");
    assert!(item.item_transform.is_none(), "cell rect is page-local");
    let [top, left, bottom, right] = item.bounds;
    assert!((top - 40.0).abs() < 0.5, "top {top}");
    assert!((left - 40.0).abs() < 0.5, "left {left}");
    assert!((bottom - 70.0).abs() < 0.5, "bottom {bottom}");
    assert!((right - 140.0).abs() < 0.5, "right {right}");

    // Cell (1, 1): x in [140, 200), y in [70, 110).
    let id11 = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 1,
        col: 1,
    };
    let items = model.element_geometry(std::slice::from_ref(&id11));
    assert_eq!(items.len(), 1);
    let [top, left, bottom, right] = items[0].bounds;
    assert!((top - 70.0).abs() < 0.5, "top {top}");
    assert!((left - 140.0).abs() < 0.5, "left {left}");
    assert!((bottom - 110.0).abs() < 0.5, "bottom {bottom}");
    assert!((right - 200.0).abs() < 0.5, "right {right}");
}

#[test]
fn cell_geometry_unknown_cell_is_dropped() {
    // An out-of-range cell resolves nothing (dropped silently, like
    // every other unresolved geometry id).
    let model = load_model();
    let id = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 9,
        col: 9,
    };
    assert!(model.element_geometry(std::slice::from_ref(&id)).is_empty());
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
    assert_eq!(
        fill,
        Some(paged_mutate::Value::ColorRef(Some("Color/C".into())))
    );
}

// ── W1.11b — per-cell edge strokes over the wire ────────────────────

#[test]
fn cell_edge_stroke_read_entries_present() {
    // `element_properties` on a cell now also reports the twelve
    // per-edge stroke paths (colour / weight / tint × 4 edges).
    let model = load_model();
    let id = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 0,
        col: 0,
    };
    let props = model.element_properties(&id).expect("cell props");
    for p in [
        paged_mutate::PropertyPath::CellTopEdgeStrokeColor,
        paged_mutate::PropertyPath::CellTopEdgeStrokeWeight,
        paged_mutate::PropertyPath::CellTopEdgeStrokeTint,
        paged_mutate::PropertyPath::CellBottomEdgeStrokeColor,
        paged_mutate::PropertyPath::CellLeftEdgeStrokeColor,
        paged_mutate::PropertyPath::CellRightEdgeStrokeColor,
    ] {
        assert!(
            props.entries.iter().any(|e| e.path == p),
            "missing edge-stroke entry {p:?}"
        );
    }
}

#[test]
fn cell_edge_stroke_mutation_round_trips_through_the_wire() {
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
            path: paged_mutate::PropertyPath::CellTopEdgeStrokeColor,
            value: paged_mutate::Value::ColorRef(Some("Color/A".into())),
        })
        .expect("edge color mutation");
    let props = model.element_properties(&id).expect("re-read");
    let color = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::CellTopEdgeStrokeColor)
        .and_then(|e| e.value.clone());
    assert_eq!(
        color,
        Some(paged_mutate::Value::ColorRef(Some("Color/A".into())))
    );
}

// ── W1.12a / W1.12b — structural mutations over the wire ────────────

#[test]
fn insert_header_row_mutation_applies() {
    let mut model = load_model();
    // The fixture has HeaderRowCount=0; an insert makes the FIRST row a
    // fresh empty header and shifts the original top row (Color/A) down.
    model
        .apply_mutation(&Mutation::InsertHeaderRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        })
        .expect("insert header row");
    // The original (0,0)=Color/A cell is now at row 1.
    let props = model
        .element_properties(&ElementId::TableCell {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 1,
            col: 0,
        })
        .expect("shifted cell");
    let fill = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::CellFillColor)
        .and_then(|e| e.value.clone());
    assert_eq!(
        fill,
        Some(paged_mutate::Value::ColorRef(Some("Color/A".into())))
    );
}

#[test]
fn insert_footer_row_mutation_applies() {
    let mut model = load_model();
    model
        .apply_mutation(&Mutation::InsertFooterRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        })
        .expect("insert footer row");
    // The table read-side now reports 3 rows (was 2).
    let props = model
        .element_properties(&ElementId::Table {
            story_id: "u10".into(),
            table_id: "t1".into(),
        })
        .expect("table props");
    let rows = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::TableRowCount)
        .and_then(|e| e.value.clone());
    assert_eq!(rows, Some(paged_mutate::Value::Length(Some(3.0))));
}

#[test]
fn set_cell_span_mutation_applies() {
    let mut model = load_model();
    model
        .apply_mutation(&Mutation::SetCellSpan {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 0,
            col: 0,
            row_span: 1,
            column_span: 2,
        })
        .expect("merge cell mutation");
    // The merge applied (a ColumnSpan=2 over the two-column table). The
    // spanning origin cell (0,0) keeps its hit context, and its cell
    // geometry now covers both columns (100 + 60 = 160pt wide vs the
    // original 100pt). `element_geometry` reports the BuiltPage cell
    // rect the render pass widened.
    let page_id = PageId("p1".into());
    let hit = model.hit_test(&page_id, (60.0, 50.0));
    let tc = hit.table_context.expect("origin cell context");
    assert_eq!((tc.row, tc.col), (0, 0));
    let id = ElementId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row: 0,
        col: 0,
    };
    let items = model.element_geometry(std::slice::from_ref(&id));
    assert_eq!(items.len(), 1, "origin cell geometry resolves");
    // Bounds are [top, left, bottom, right]; width = right - left.
    let [_, left, _, right] = items[0].bounds;
    let width = right - left;
    assert!(
        (width - 160.0).abs() < 0.5,
        "merged cell spans both columns (100 + 60 = 160pt), got {width}"
    );
}
