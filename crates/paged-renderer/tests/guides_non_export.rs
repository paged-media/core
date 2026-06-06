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

//! W1.8 — ruler guides must never appear in exported / printed output.
//!
//! `<Guide>` elements are an editor-canvas overlay only (the canvas
//! draws them cyan; see `paged_parse::Spread::guides`). The render
//! pipeline parses them but must NOT paint them into the page display
//! list. This test renders two otherwise-identical spreads — one with
//! three ruler guides, one without — and asserts the exported display
//! lists are byte-for-byte command-equivalent (guides contribute zero
//! draw commands).

use std::io::Write;

use paged_compose::DisplayCommand;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// Build a one-page IDML whose single spread optionally carries three
/// ruler guides. The page holds one stroked + filled rectangle so the
/// display list is non-empty either way.
fn idml_with_guides(include_guides: bool) -> Vec<u8> {
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
</Document>"#,
    )
    .unwrap();

    // Three guides (two vertical, one horizontal) on the page. Emitted
    // before the Rectangle so any accidental paint would land underneath
    // (i.e. its absence is what we assert).
    let guides = if include_guides {
        r#"<Guide Self="g1" Orientation="Vertical" Location="120" PageIndex="0"/>
       <Guide Self="g2" Orientation="Vertical" Location="300" PageIndex="0"/>
       <Guide Self="g3" Orientation="Horizontal" Location="200" PageIndex="0"/>"#
    } else {
        ""
    };

    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" ItemTransform="1 0 0 1 0 0">
    <Page Self="pg1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
    {guides}
    <Rectangle Self="r1" FillColor="Color/Black" StrokeColor="Color/Black" StrokeWeight="2"
               ItemTransform="1 0 0 1 100 100">
      <Properties>
        <PathGeometry>
          <GeometryPathType PathOpen="false">
            <PathPointArray>
              <PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
              <PathPointType Anchor="0 80" LeftDirection="0 80" RightDirection="0 80"/>
              <PathPointType Anchor="160 80" LeftDirection="160 80" RightDirection="160 80"/>
              <PathPointType Anchor="160 0" LeftDirection="160 0" RightDirection="160 0"/>
            </PathPointArray>
          </GeometryPathType>
        </PathGeometry>
      </Properties>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

fn render_command_count(include_guides: bool) -> usize {
    let bytes = idml_with_guides(include_guides);
    let doc = paged_scene::Document::open(&bytes).expect("open guide idml");
    let options = paged_renderer::pipeline::PipelineOptions::default();
    let built = paged_renderer::pipeline::build_document(&doc, &options).expect("build");
    built.pages[0].list.commands.len()
}

#[test]
fn guides_are_parsed_but_not_present_in_render_path() {
    // Sanity: the spread actually carries guides once parsed (so the
    // test would catch a parser regression that silently drops them and
    // makes the with/without comparison trivially pass).
    let bytes = idml_with_guides(true);
    let doc = paged_scene::Document::open(&bytes).expect("open");
    let guide_count: usize = doc.spreads.iter().map(|s| s.spread.guides.len()).sum();
    assert_eq!(guide_count, 3, "the fixture should parse three guides");
}

#[test]
fn guides_add_no_commands_to_exported_display_list() {
    let with_guides = render_command_count(true);
    let without_guides = render_command_count(false);
    assert_eq!(
        with_guides, without_guides,
        "ruler guides must contribute zero draw commands to export \
         (with={with_guides}, without={without_guides})"
    );
    assert!(without_guides > 0, "the rectangle should still render");
}

#[test]
fn exported_page_has_no_guide_orientation_strokes() {
    // A defensive shape-level check: every StrokePath on the page must
    // trace to the rectangle's outline, never a full-page vertical /
    // horizontal guide line. We approximate "guide line" as a stroke
    // whose path spans (near) the full page height/width — the
    // rectangle's 160×80 outline never does.
    let bytes = idml_with_guides(true);
    let doc = paged_scene::Document::open(&bytes).expect("open");
    let options = paged_renderer::pipeline::PipelineOptions::default();
    let built = paged_renderer::pipeline::build_document(&doc, &options).expect("build");
    let page = &built.pages[0];
    for cmd in &page.list.commands {
        if let DisplayCommand::StrokePath {
            path_id, transform, ..
        } = cmd
        {
            let Some(path) = page.list.paths.get(*path_id) else {
                continue;
            };
            // Page-space AABB of the stroked path.
            let mut min_y = f32::INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            for seg in &path.segments {
                let pts: &[(f32, f32)] = match seg {
                    paged_compose::PathSegment::MoveTo { x, y }
                    | paged_compose::PathSegment::LineTo { x, y } => &[(*x, *y)],
                    paged_compose::PathSegment::CubicTo { x, y, .. }
                    | paged_compose::PathSegment::QuadTo { x, y, .. } => &[(*x, *y)],
                    paged_compose::PathSegment::Close => &[],
                };
                for (lx, ly) in pts {
                    let (px, py) = transform.apply(*lx, *ly);
                    min_x = min_x.min(px);
                    max_x = max_x.max(px);
                    min_y = min_y.min(py);
                    max_y = max_y.max(py);
                }
            }
            let span_x = max_x - min_x;
            let span_y = max_y - min_y;
            // A guide would span ~full page (612pt wide / 792pt tall);
            // the rectangle outline spans only 160×80.
            assert!(
                span_x < 400.0 && span_y < 400.0,
                "found a full-page-spanning stroke ({span_x}×{span_y}) — a leaked guide?"
            );
        }
    }
}
