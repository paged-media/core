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

//! W1.13 — table-cell TEXT editing over the wire.
//!
//! Covers the full loop: hit-test into a cell → caret-in-cell offset →
//! `InsertText`/`DeleteRange` with a cell qualifier → read-back; caret /
//! word / paragraph bounds in cells; and the paragraph_idx collision
//! regression (body paragraph N and cell paragraph N are distinct
//! addresses).
//!
//! The fixture is a one-page story whose body holds a leading paragraph
//! ("Body line one.") and then a 2×2 table. Each cell carries a single
//! text paragraph. Critically, the BODY's paragraph 0 and each cell's
//! paragraph 0 all sit at `paragraph_idx == 0` in their own streams —
//! the exact collision the disjoint `cell` qualifier resolves.

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::channel::Mutation;
use paged_canvas::{CanvasModel, CanvasOptions, PageId, TextCellAddr};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// A story = one body paragraph + a 2×2 table. Each cell `(col,row)`
/// carries the text `c{col}{row}` ("c00", "c10", "c01", "c11") so a
/// read-back can tell cells apart unambiguously.
fn build_table_idml() -> Vec<u8> {
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

    // Body paragraph, then a Table. Cell content paragraphs are parsed
    // exactly like story paragraphs (ParagraphStyleRange > CSR >
    // Content). Each Cell Name="col:row".
    let cell = |col: u32, row: u32| {
        format!(
            r#"<Cell Name="{col}:{row}" RowSpan="1" ColumnSpan="1" TextTopInset="2" TextLeftInset="2" TextBottomInset="2" TextRightInset="2">
              <ParagraphStyleRange>
                <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                  <Content>c{col}{row}</Content>
                </CharacterStyleRange>
              </ParagraphStyleRange>
            </Cell>"#
        )
    };
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>Body line one.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Table Self="tbl1" HeaderRowCount="0" FooterRowCount="0" BodyRowCount="2" ColumnCount="2">
          <Row Self="r0" Name="0" SingleRowHeight="40"/>
          <Row Self="r1" Name="1" SingleRowHeight="40"/>
          <Column Self="col0" Name="0" SingleColumnWidth="120"/>
          <Column Self="col1" Name="1" SingleColumnWidth="120"/>
          {c00}
          {c10}
          {c01}
          {c11}
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        c00 = cell(0, 0),
        c10 = cell(1, 0),
        c01 = cell(0, 1),
        c11 = cell(1, 1),
    );
    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

fn load_model() -> CanvasModel {
    let bytes = build_table_idml();
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("doc", &bytes, opts).expect("load + build")
}

fn addr(col: u32, row: u32) -> TextCellAddr {
    TextCellAddr {
        table_id: "tbl1".into(),
        row,
        col,
    }
}

/// Read back a cell's concatenated paragraph text from the live scene.
fn cell_text(model: &CanvasModel, col: u32, row: u32) -> String {
    let story = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == "u10")
        .expect("story u10");
    let table = story
        .story
        .paragraphs
        .iter()
        .filter_map(|p| p.table.as_ref())
        .find(|t| t.self_id.as_deref() == Some("tbl1"))
        .expect("table tbl1");
    let c = table
        .cells
        .iter()
        .find(|c| c.coords() == Some((col, row)))
        .expect("cell");
    let mut out = String::new();
    for (i, p) in c.paragraphs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for r in &p.runs {
            out.push_str(&r.text);
        }
    }
    out
}

fn body_text(model: &CanvasModel) -> String {
    let story = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == "u10")
        .unwrap();
    let mut out = String::new();
    for (i, p) in story.story.paragraphs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for r in &p.runs {
            out.push_str(&r.text);
        }
    }
    out
}

#[test]
fn fixture_parses_with_four_cells_carrying_text() {
    let model = load_model();
    assert_eq!(cell_text(&model, 0, 0), "c00");
    assert_eq!(cell_text(&model, 1, 0), "c10");
    assert_eq!(cell_text(&model, 0, 1), "c01");
    assert_eq!(cell_text(&model, 1, 1), "c11");
    // Body flow holds the leading paragraph + a contentless table host.
    assert!(body_text(&model).starts_with("Body line one."));
}

#[test]
fn insert_text_into_cell_then_read_back() {
    let mut model = load_model();
    // Insert "X" at cell-local offset 1 of cell (0,0): "c00" → "cX00".
    let out = model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 1,
            text: "X".into(),
            cell: Some(addr(0, 0)),
        })
        .expect("insert into cell");
    assert_eq!(cell_text(&model, 0, 0), "cX00");
    // Other cells untouched.
    assert_eq!(cell_text(&model, 1, 0), "c10");
    assert_eq!(cell_text(&model, 0, 1), "c01");
    // The body paragraph must NOT have been touched by a cell edit.
    assert!(body_text(&model).starts_with("Body line one."));
    // Inverse is a cell-qualified DeleteRange over [1,2).
    match out.inverse {
        paged_canvas::TextOp::DeleteRange {
            start, end, cell, ..
        } => {
            assert_eq!((start, end), (1, 2));
            assert_eq!(cell, Some(addr(0, 0)));
        }
        other => panic!("inverse must be cell DeleteRange, got {other:?}"),
    }
}

#[test]
fn pour_text_into_a_freshly_inserted_empty_table_cell_does_not_panic() {
    // Regression (the sheet cell-pour panic): a cell created by `insertTable`
    // carries ZERO paragraphs. The first text pour into it located the end of
    // an empty stream and indexed `paragraphs[0]` on an empty slice —
    // `index out of bounds: the len is 0 but the index is 0` in
    // `insert_one_segment`. `apply_insert_text` now seeds one empty paragraph
    // when the cell stream is empty, so the first write lands cleanly.
    let mut model = load_model();
    let out = model
        .apply_mutation(&Mutation::InsertTable {
            story_id: "u10".into(),
            rows: 2,
            cols: 2,
            header_rows: 0,
            footer_rows: 0,
            column_widths: vec![],
            row_heights: vec![],
        })
        .expect("insert a 2x2 table into the story");
    let table_id = match out.created_id {
        Some(paged_canvas::ElementId::Table { table_id, .. }) => table_id,
        other => panic!("insertTable must report a Table createdId, got {other:?}"),
    };
    // The brand-new cell (0,0) has no paragraphs; pouring text MUST NOT panic.
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "Hi".into(),
            cell: Some(TextCellAddr {
                table_id,
                row: 0,
                col: 0,
            }),
        })
        .expect("pour text into the fresh empty cell");
}

#[test]
fn insert_into_cell_undo_restores_exact_prior_content() {
    let mut model = load_model();
    let before = cell_text(&model, 1, 1);
    let hash_before = model.current_state_hash();
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "ZZ".into(),
            cell: Some(addr(1, 1)),
        })
        .unwrap();
    assert_eq!(cell_text(&model, 1, 1), "ZZc11");
    assert_ne!(model.current_state_hash(), hash_before);
    // Undo via the model's log → exact prior cell content + state hash.
    model.undo().expect("undo");
    assert_eq!(cell_text(&model, 1, 1), before);
    assert_eq!(
        model.current_state_hash(),
        hash_before,
        "undo must restore the exact prior state"
    );
}

#[test]
fn delete_range_in_cell_recovers_on_undo() {
    let mut model = load_model();
    // Delete "00" from "c00" (cell (0,0)) → "c"; undo restores "c00".
    model
        .apply_mutation(&Mutation::DeleteRange {
            story_id: "u10".into(),
            start: 1,
            end: 3,
            cell: Some(addr(0, 0)),
        })
        .unwrap();
    assert_eq!(cell_text(&model, 0, 0), "c");
    model.undo().unwrap();
    assert_eq!(cell_text(&model, 0, 0), "c00");
}

#[test]
fn multi_paragraph_cell_insert_newline_splits_within_cell() {
    let mut model = load_model();
    // Insert "\n" at cell-local offset 1 of "c00" → two paragraphs
    // "c" / "00" inside the SAME cell. Undo merges them back.
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 1,
            text: "\n".into(),
            cell: Some(addr(0, 0)),
        })
        .unwrap();
    // Cell now has two paragraphs: "c" and "00" (joined by \n in read-back).
    assert_eq!(cell_text(&model, 0, 0), "c\n00");
    let story = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == "u10")
        .unwrap();
    let table = story
        .story
        .paragraphs
        .iter()
        .filter_map(|p| p.table.as_ref())
        .find(|t| t.self_id.as_deref() == Some("tbl1"))
        .unwrap();
    let c = table
        .cells
        .iter()
        .find(|c| c.coords() == Some((0, 0)))
        .unwrap();
    assert_eq!(c.paragraphs.len(), 2, "cell split into two paragraphs");
    model.undo().unwrap();
    assert_eq!(cell_text(&model, 0, 0), "c00");
}

#[test]
fn edit_in_one_cell_does_not_disturb_sibling_cells() {
    let mut model = load_model();
    for (col, row) in [(1u32, 0u32), (0, 1), (1, 1)] {
        model
            .apply_mutation(&Mutation::InsertText {
                story_id: "u10".into(),
                offset: 0,
                text: "*".into(),
                cell: Some(addr(0, 0)),
            })
            .unwrap();
        assert_eq!(
            cell_text(&model, col, row),
            format!("c{col}{row}"),
            "sibling cell ({col},{row}) must be untouched by an edit in (0,0)"
        );
        model.undo().unwrap();
    }
}

#[test]
fn hit_test_in_cell_returns_cell_context_and_cell_local_offset() {
    let model = load_model();
    let page_id = PageId("p1".into());
    // The table sits below the body line. Frame top-left is page-local
    // (40,40); body line ~14pt; table starts a bit under it. Cell (0,0)
    // is the top-left cell of the table. Click near its left edge so the
    // offset snaps toward the start of the cell's text.
    // Sweep a vertical band to find a y that lands in a cell of row 0.
    let mut found = None;
    for y in 55..200 {
        let hit = model.hit_test(&page_id, (50.0, y as f32));
        if let Some(tc) = hit.table_context.as_ref() {
            found = Some((y, tc.clone(), hit.offset_within_story));
            break;
        }
    }
    let (_, tc, off) = found.expect("a click should land in a table cell");
    assert_eq!(tc.table_id, "tbl1");
    // Column 0 (we clicked near the left edge).
    assert_eq!(tc.col, 0);
    // Offset is CELL-LOCAL: within the 3-byte cell text "cNN" → 0..=3.
    let off = off.expect("cell hit yields an offset");
    assert!(
        off <= 3,
        "cell-local offset {off} should be within cell text length 3"
    );
}

#[test]
fn caret_word_paragraph_bounds_answer_for_cell_addresses() {
    let model = load_model();
    let sel = paged_canvas::ContentSelection::cell_caret("u10", addr(1, 1), 1);
    // Caret geometry resolves against the cell's own line layout.
    let caret = paged_canvas::caret_geometry(model.built(), &sel)
        .expect("caret geometry for a cell address");
    // The cell sits in column 1 (x ≳ 40 + 120 inset region) and below
    // the body line — both strictly inside the page.
    assert!(
        caret.x_pt > 40.0 && caret.x_pt < 572.0,
        "caret x {} in frame",
        caret.x_pt
    );
    assert!(
        caret.top_pt > 40.0,
        "caret y {} below page top",
        caret.top_pt
    );

    // Word bounds over the cell's text: offset 1 of "c11" → the whole
    // word [0,3).
    let wb = model
        .word_bounds("u10", Some(&addr(1, 1)), 1)
        .expect("word bounds in cell");
    assert_eq!((wb.start, wb.end), (0, 3));

    // Paragraph bounds: the single cell paragraph spans [0,3).
    let pb = model
        .paragraph_bounds("u10", Some(&addr(1, 1)), 2)
        .expect("paragraph bounds in cell");
    assert_eq!((pb.start, pb.end), (0, 3));
}

/// The keystone collision regression: body paragraph 0 and a cell
/// paragraph 0 share `paragraph_idx == 0`, yet must be DISTINCT
/// addresses. Editing the cell at offset 0 must not touch the body, and
/// vice versa; the StoryLayout must expose both a body line and a cell
/// line at paragraph_idx 0 distinguished only by the `cell` qualifier.
#[test]
fn body_para_zero_and_cell_para_zero_are_distinct_addresses() {
    let mut model = load_model();

    // 1. The renderer's StoryLayout has body lines (cell == None) AND
    //    cell lines (cell == Some) at paragraph_idx 0.
    let built = model.built();
    let lines = built.story_layout("u10");
    let body_p0 = lines
        .iter()
        .any(|l| l.cell.is_none() && l.paragraph_idx == 0);
    let cell_p0 = lines
        .iter()
        .any(|l| l.cell.as_ref().map(|c| (c.row, c.col)) == Some((0, 0)) && l.paragraph_idx == 0);
    assert!(body_p0, "expected a BODY line at paragraph_idx 0");
    assert!(cell_p0, "expected a CELL (0,0) line at paragraph_idx 0");

    // 2. Editing cell (0,0) at offset 0 changes only the cell.
    let body_before = body_text(&model);
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "@".into(),
            cell: Some(addr(0, 0)),
        })
        .unwrap();
    assert_eq!(cell_text(&model, 0, 0), "@c00");
    assert_eq!(body_text(&model), body_before, "body must be untouched");
    model.undo().unwrap();

    // 3. Editing the BODY at offset 0 changes only the body.
    let cell_before = cell_text(&model, 0, 0);
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "@".into(),
            cell: None,
        })
        .unwrap();
    assert!(body_text(&model).starts_with("@Body line one."));
    assert_eq!(
        cell_text(&model, 0, 0),
        cell_before,
        "cell must be untouched by a body edit"
    );
}

#[test]
fn edited_cell_text_relayouts_more_lines() {
    let mut model = load_model();
    let addr00 = addr(0, 0);
    let line_count = |m: &CanvasModel| {
        m.built()
            .cell_layout(
                "u10",
                &paged_canvas::CellAddr {
                    table_id: "tbl1".into(),
                    row: 0,
                    col: 0,
                },
            )
            .len()
    };
    let before = line_count(&model);
    assert!(before >= 1, "cell starts with at least one laid-out line");
    // Insert a hard paragraph break → the cell re-lays out into more
    // lines (the renderer must re-run the cell-paragraph emit on the
    // edited content).
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 1,
            text: "\nmore text here".into(),
            cell: Some(addr00),
        })
        .unwrap();
    let after = line_count(&model);
    assert!(
        after > before,
        "edited cell must re-lay out into more lines ({before} -> {after})"
    );
}
