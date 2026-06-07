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

//! Tables v2 mutation round-trips (W1.11b / W1.12a / W1.12b).
//!
//! * W1.11b — per-cell edge-stroke `PropertyPath`s (colour / weight /
//!   tint on each of the four edges): apply writes the parse field,
//!   inverse restores the prior value bytewise.
//! * W1.12a — header / footer row inserts: `Insert{Header,Footer}Row`
//!   grows the band count + mints an empty row; the inverse
//!   (`Remove{Header,Footer}Row`) removes it and re-inserts losslessly.
//! * W1.12b — `SetCellSpan` merge / split, inverse restoring prior spans.

use std::io::Write;

use paged_mutate::{apply, NodeId, Operation, PropertyPath, Value};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A one-page IDML whose story `u10` carries a 2-column × 3-row table
/// `t1` (one header row, two body rows). Cells are column-major. Used
/// for every test below.
fn table_idml() -> Vec<u8> {
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
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" Self="d1">
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
    <Color Self="Color/Red" Name="Red" Space="CMYK" ColorValue="0 100 100 0"/>
    <Color Self="Color/Blue" Name="Blue" Space="CMYK" ColorValue="100 100 0 0"/>
  </Graphic>
</idPkg:Graphic>"#,
    );
    put(
        &mut zip,
        "Spreads/Spread_sp1.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1">
    <Page Self="p1" GeometricBounds="0 0 400 612" ItemTransform="1 0 0 1 0 0"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 380 572" ItemTransform="1 0 0 1 0 0"/>
  </Spread>
</idPkg:Spread>"#,
    );
    put(
        &mut zip,
        "Stories/Story_u10.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Table Self="t1" HeaderRowCount="1" FooterRowCount="0" BodyRowCount="2" ColumnCount="2">
          <Row Self="r0" Name="0" SingleRowHeight="30"/>
          <Row Self="r1" Name="1" SingleRowHeight="30"/>
          <Row Self="r2" Name="2" SingleRowHeight="30"/>
          <Column Self="c0" Name="0" SingleColumnWidth="100"/>
          <Column Self="c1" Name="1" SingleColumnWidth="100"/>
          <Cell Self="cell00" Name="0:0"><ParagraphStyleRange><CharacterStyleRange><Content>A</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="cell01" Name="0:1"><ParagraphStyleRange><CharacterStyleRange><Content>B</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="cell02" Name="0:2"><ParagraphStyleRange><CharacterStyleRange><Content>C</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="cell10" Name="1:0"><ParagraphStyleRange><CharacterStyleRange><Content>D</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="cell11" Name="1:1"><ParagraphStyleRange><CharacterStyleRange><Content>E</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          <Cell Self="cell12" Name="1:2"><ParagraphStyleRange><CharacterStyleRange><Content>F</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    );
    zip.finish().unwrap().into_inner()
}

fn open_doc() -> Document {
    Document::open(&table_idml()).expect("fixture must open")
}

/// Borrow the parsed `<Table>` for assertions.
fn table(doc: &Document) -> &paged_parse::Table {
    doc.stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .find_map(|p| p.table.as_ref())
        .expect("table t1 present")
}

/// Borrow the cell originating at `(col, row)`.
fn cell(doc: &Document, col: u32, row: u32) -> &paged_parse::TableCell {
    table(doc)
        .cells
        .iter()
        .find(|c| c.coords() == Some((col, row)))
        .unwrap_or_else(|| panic!("cell at ({col},{row}) present"))
}

fn cell_node(row: u32, col: u32) -> NodeId {
    NodeId::TableCell {
        story_id: "u10".into(),
        table_id: "t1".into(),
        row,
        col,
    }
}

fn set(node: NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty { node, path, value }
}

// ── W1.11b — per-cell edge strokes ──────────────────────────────────

#[test]
fn cell_edge_stroke_color_round_trips() {
    let mut doc = open_doc();
    assert!(cell(&doc, 0, 1).top_edge_stroke_color.is_none());

    let applied = apply(
        &mut doc,
        &set(
            cell_node(1, 0),
            PropertyPath::CellTopEdgeStrokeColor,
            Value::ColorRef(Some("Color/Red".into())),
        ),
    )
    .expect("apply edge color");
    assert_eq!(
        cell(&doc, 0, 1).top_edge_stroke_color.as_deref(),
        Some("Color/Red")
    );
    // A cell edge change reflows the host story.
    assert!(!applied.invalidation.text_reflow.is_empty());

    // Inverse restores the prior absent value (bytewise round-trip).
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert!(cell(&doc, 0, 1).top_edge_stroke_color.is_none());
}

#[test]
fn cell_edge_stroke_weight_round_trips() {
    let mut doc = open_doc();
    assert!(cell(&doc, 1, 1).bottom_edge_stroke_weight.is_none());

    let applied = apply(
        &mut doc,
        &set(
            cell_node(1, 1),
            PropertyPath::CellBottomEdgeStrokeWeight,
            Value::Length(Some(2.5)),
        ),
    )
    .expect("apply edge weight");
    assert_eq!(cell(&doc, 1, 1).bottom_edge_stroke_weight, Some(2.5));

    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert!(cell(&doc, 1, 1).bottom_edge_stroke_weight.is_none());
}

#[test]
fn cell_edge_stroke_tint_round_trips() {
    let mut doc = open_doc();
    let applied = apply(
        &mut doc,
        &set(
            cell_node(2, 1),
            PropertyPath::CellLeftEdgeStrokeTint,
            Value::Length(Some(40.0)),
        ),
    )
    .expect("apply edge tint");
    assert_eq!(cell(&doc, 1, 2).left_edge_stroke_tint, Some(40.0));

    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert!(cell(&doc, 1, 2).left_edge_stroke_tint.is_none());
}

#[test]
fn cell_all_four_edges_independent() {
    // Writing each edge's colour touches only that edge.
    let mut doc = open_doc();
    let edges = [
        PropertyPath::CellTopEdgeStrokeColor,
        PropertyPath::CellBottomEdgeStrokeColor,
        PropertyPath::CellLeftEdgeStrokeColor,
        PropertyPath::CellRightEdgeStrokeColor,
    ];
    for path in edges {
        apply(
            &mut doc,
            &set(
                cell_node(0, 0),
                path,
                Value::ColorRef(Some("Color/Blue".into())),
            ),
        )
        .expect("apply edge");
    }
    let c = cell(&doc, 0, 0);
    assert_eq!(c.top_edge_stroke_color.as_deref(), Some("Color/Blue"));
    assert_eq!(c.bottom_edge_stroke_color.as_deref(), Some("Color/Blue"));
    assert_eq!(c.left_edge_stroke_color.as_deref(), Some("Color/Blue"));
    assert_eq!(c.right_edge_stroke_color.as_deref(), Some("Color/Blue"));
}

#[test]
fn cell_edge_color_clear_then_restore() {
    // Pre-set an edge, then clear it (ColorRef(None)) and undo.
    let mut doc = open_doc();
    apply(
        &mut doc,
        &set(
            cell_node(0, 0),
            PropertyPath::CellRightEdgeStrokeColor,
            Value::ColorRef(Some("Color/Red".into())),
        ),
    )
    .expect("seed edge");
    let applied = apply(
        &mut doc,
        &set(
            cell_node(0, 0),
            PropertyPath::CellRightEdgeStrokeColor,
            Value::ColorRef(None),
        ),
    )
    .expect("clear edge");
    assert!(cell(&doc, 0, 0).right_edge_stroke_color.is_none());
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(
        cell(&doc, 0, 0).right_edge_stroke_color.as_deref(),
        Some("Color/Red")
    );
}

// ── W1.12a — header / footer row inserts ────────────────────────────

#[test]
fn insert_header_row_grows_band_and_inverts() {
    let mut doc = open_doc();
    assert_eq!(table(&doc).header_row_count, 1);
    assert_eq!(table(&doc).rows.len(), 3);
    // Original header cell content.
    assert_eq!(cell(&doc, 0, 0).paragraphs[0].runs[0].text, "A");

    let applied = apply(
        &mut doc,
        &Operation::InsertHeaderRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
            restore: None,
        },
    )
    .expect("insert header row");
    // Band grew, total rows grew, everything shifted down by one.
    assert_eq!(table(&doc).header_row_count, 2);
    assert_eq!(table(&doc).rows.len(), 4);
    // The original header "A" is now at row 1; row 0 is the fresh empty.
    assert_eq!(cell(&doc, 0, 1).paragraphs[0].runs[0].text, "A");
    assert!(cell(&doc, 0, 0).paragraphs.is_empty());
    assert!(matches!(applied.inverse, Operation::RemoveHeaderRow { .. }));

    // Undo restores the original shape.
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(table(&doc).header_row_count, 1);
    assert_eq!(table(&doc).rows.len(), 3);
    assert_eq!(cell(&doc, 0, 0).paragraphs[0].runs[0].text, "A");
}

#[test]
fn remove_header_row_shrinks_band_and_restores_lossless() {
    let mut doc = open_doc();
    let applied = apply(
        &mut doc,
        &Operation::RemoveHeaderRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        },
    )
    .expect("remove header row");
    assert_eq!(table(&doc).header_row_count, 0);
    assert_eq!(table(&doc).rows.len(), 2);
    // Former body row "B" is now the top row.
    assert_eq!(cell(&doc, 0, 0).paragraphs[0].runs[0].text, "B");
    // Inverse re-inserts WITH the captured row (restore blob present).
    assert!(matches!(
        applied.inverse,
        Operation::InsertHeaderRow {
            restore: Some(_),
            ..
        }
    ));
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(table(&doc).header_row_count, 1);
    assert_eq!(table(&doc).rows.len(), 3);
}

#[test]
fn remove_header_row_rejected_when_no_header() {
    let mut doc = open_doc();
    // Strip the only header row first.
    apply(
        &mut doc,
        &Operation::RemoveHeaderRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        },
    )
    .expect("first remove ok");
    // A second remove has no header band left → error.
    let err = apply(
        &mut doc,
        &Operation::RemoveHeaderRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        },
    );
    assert!(err.is_err(), "removing a non-existent header must reject");
}

#[test]
fn insert_footer_row_appends_at_bottom_and_inverts() {
    let mut doc = open_doc();
    assert_eq!(table(&doc).footer_row_count, 0);
    assert_eq!(table(&doc).rows.len(), 3);

    let applied = apply(
        &mut doc,
        &Operation::InsertFooterRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
            restore: None,
        },
    )
    .expect("insert footer row");
    assert_eq!(table(&doc).footer_row_count, 1);
    assert_eq!(table(&doc).rows.len(), 4);
    // The new footer row is the last row (index 3) and is empty.
    assert!(cell(&doc, 0, 3).paragraphs.is_empty());
    // Existing rows are untouched (footer appends, no shift).
    assert_eq!(cell(&doc, 0, 0).paragraphs[0].runs[0].text, "A");
    assert_eq!(cell(&doc, 0, 2).paragraphs[0].runs[0].text, "C");

    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(table(&doc).footer_row_count, 0);
    assert_eq!(table(&doc).rows.len(), 3);
}

#[test]
fn remove_footer_row_rejected_when_no_footer() {
    let mut doc = open_doc();
    let err = apply(
        &mut doc,
        &Operation::RemoveFooterRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        },
    );
    assert!(err.is_err(), "no footer band to remove");
}

#[test]
fn insert_then_remove_footer_round_trips_content() {
    // Insert a footer, then a Remove must undo back to the original.
    let mut doc = open_doc();
    apply(
        &mut doc,
        &Operation::InsertFooterRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
            restore: None,
        },
    )
    .expect("insert footer");
    let applied = apply(
        &mut doc,
        &Operation::RemoveFooterRow {
            story_id: "u10".into(),
            table_id: "t1".into(),
        },
    )
    .expect("remove footer");
    assert_eq!(table(&doc).footer_row_count, 0);
    assert_eq!(table(&doc).rows.len(), 3);
    apply(&mut doc, &applied.inverse).expect("undo the remove");
    assert_eq!(table(&doc).footer_row_count, 1);
    assert_eq!(table(&doc).rows.len(), 4);
}

// ── W1.12b — merge / split spans ────────────────────────────────────

#[test]
fn set_cell_span_merge_then_invert() {
    let mut doc = open_doc();
    assert_eq!(cell(&doc, 0, 0).row_span, 1);
    assert_eq!(cell(&doc, 0, 0).column_span, 1);

    let applied = apply(
        &mut doc,
        &Operation::SetCellSpan {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 0,
            col: 0,
            row_span: 2,
            column_span: 2,
        },
    )
    .expect("merge cell");
    assert_eq!(cell(&doc, 0, 0).row_span, 2);
    assert_eq!(cell(&doc, 0, 0).column_span, 2);
    assert!(!applied.invalidation.text_reflow.is_empty());

    // Inverse restores the prior (1, 1) spans.
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(cell(&doc, 0, 0).row_span, 1);
    assert_eq!(cell(&doc, 0, 0).column_span, 1);
}

#[test]
fn set_cell_span_split_restores_prior_span() {
    // Merge first, then split back to 1×1; the split's inverse must
    // restore the merged spans.
    let mut doc = open_doc();
    apply(
        &mut doc,
        &Operation::SetCellSpan {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 0,
            col: 1,
            row_span: 3,
            column_span: 1,
        },
    )
    .expect("merge");
    let applied = apply(
        &mut doc,
        &Operation::SetCellSpan {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 0,
            col: 1,
            row_span: 1,
            column_span: 1,
        },
    )
    .expect("split");
    assert_eq!(cell(&doc, 1, 0).row_span, 1);
    // Undo the split → restore the 3-row span.
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(cell(&doc, 1, 0).row_span, 3);
}

#[test]
fn set_cell_span_clamps_zero_to_one() {
    let mut doc = open_doc();
    apply(
        &mut doc,
        &Operation::SetCellSpan {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 1,
            col: 0,
            row_span: 0,
            column_span: 0,
        },
    )
    .expect("apply with zero spans");
    // A 0 span is meaningless — clamps to IDML's minimum of 1.
    assert_eq!(cell(&doc, 0, 1).row_span, 1);
    assert_eq!(cell(&doc, 0, 1).column_span, 1);
}

#[test]
fn set_cell_span_unknown_cell_rejected() {
    let mut doc = open_doc();
    let err = apply(
        &mut doc,
        &Operation::SetCellSpan {
            story_id: "u10".into(),
            table_id: "t1".into(),
            row: 9,
            col: 9,
            row_span: 2,
            column_span: 2,
        },
    );
    assert!(err.is_err(), "no cell originates at (9,9)");
}
