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

//! ADR 171 — PDF bookmarks. A document's bookmarks export as `/Outlines`,
//! as InDesign 20.0.1 exports them (`pdf-outlines.py` on its PDF of the
//! updated `toc` sample): each a titled item nested under the nearest
//! earlier item one level up, going to its page fitted (`/Fit`).

use lopdf::{Document, Object, ObjectId};
use paged_export_pdf::{
    export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles, PdfBookmark,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn export(bookmarks: Vec<PdfBookmark>, page_range: Option<(usize, usize)>) -> Vec<u8> {
    let sample = paged_gen::samples::toc::build();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let font = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .unwrap();
    let mut opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    opts.font = Some(&font);
    let fonts = FontTable::build(&document, &opts);
    opts.pre_built_font_table = Some(&fonts);
    let doc = pipeline::build_document(&document, &opts).unwrap();
    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    export_pdf(ExportInput {
        doc: &doc,
        palette: &document.palette,
        fonts: Some(&fonts),
        cmm: &cmm,
        profiles: ExportProfiles::default(),
        inks: ExportInkSettings::default(),
        options: ExportOptions {
            bookmarks,
            page_range,
            ..Default::default()
        },
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    })
    .expect("export")
    .bytes
}

fn mark(title: &str, depth: u32, page_index: usize) -> PdfBookmark {
    PdfBookmark {
        title: title.into(),
        depth,
        page_index,
    }
}

/// The outline as `(depth, title, page index, view)`, walked depth-first.
fn outline(pdf: &[u8]) -> Vec<(u32, String, usize, String)> {
    let doc = Document::load_mem(pdf).expect("re-parse");
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let catalog = doc.catalog().unwrap();
    let Ok(root) = catalog.get(b"Outlines").and_then(Object::as_reference) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(
        doc: &Document,
        pages: &[ObjectId],
        first: Option<ObjectId>,
        depth: u32,
        out: &mut Vec<(u32, String, usize, String)>,
    ) {
        let mut at = first;
        while let Some(id) = at {
            let item = doc.get_dictionary(id).unwrap();
            let title = match item.get(b"Title").unwrap() {
                Object::String(bytes, _) => {
                    if bytes.starts_with(&[0xFE, 0xFF]) {
                        let units: Vec<u16> = bytes[2..]
                            .chunks(2)
                            .map(|c| u16::from_be_bytes([c[0], c[1]]))
                            .collect();
                        String::from_utf16(&units).unwrap()
                    } else {
                        String::from_utf8_lossy(bytes).into_owned()
                    }
                }
                other => panic!("title {other:?}"),
            };
            let dest = item.get(b"Dest").unwrap().as_array().unwrap();
            let page = dest[0].as_reference().unwrap();
            let view = String::from_utf8_lossy(dest[1].as_name().unwrap()).into_owned();
            out.push((
                depth,
                title,
                pages.iter().position(|p| *p == page).unwrap(),
                view,
            ));
            let first = item.get(b"First").and_then(Object::as_reference).ok();
            walk(doc, pages, first, depth + 1, out);
            at = item.get(b"Next").and_then(Object::as_reference).ok();
        }
    }
    let first = doc
        .get_dictionary(root)
        .unwrap()
        .get(b"First")
        .and_then(Object::as_reference)
        .ok();
    walk(&doc, &pages, first, 0, &mut out);
    out
}

#[test]
fn bookmarks_export_as_a_nested_outline() {
    let pdf = export(
        vec![
            mark("Getting Started", 0, 0),
            mark("First Steps", 1, 0),
            mark("Going Further", 0, 1),
            mark("Aside", 1, 1),
            mark("Last Words", 1, 2),
        ],
        None,
    );
    let fit = "Fit".to_string();
    assert_eq!(
        outline(&pdf),
        [
            (0, "Getting Started".into(), 0, fit.clone()),
            (1, "First Steps".into(), 0, fit.clone()),
            (0, "Going Further".into(), 1, fit.clone()),
            (1, "Aside".into(), 1, fit.clone()),
            (1, "Last Words".into(), 2, fit),
        ]
    );
    // Open items: the parent's /Count is its descendants.
    let doc = Document::load_mem(&pdf).unwrap();
    let root = doc
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    let root = doc.get_dictionary(root).unwrap();
    assert_eq!(root.get(b"Count").unwrap().as_i64().unwrap(), 5);
}

#[test]
fn no_bookmarks_write_no_outline() {
    let pdf = export(Vec::new(), None);
    let doc = Document::load_mem(&pdf).unwrap();
    assert!(doc.catalog().unwrap().get(b"Outlines").is_err());
}

/// A bookmark whose page is not exported goes, with the ones under it.
#[test]
fn a_page_range_keeps_its_own_bookmarks() {
    let pdf = export(
        vec![
            mark("Getting Started", 0, 0),
            mark("First Steps", 1, 0),
            mark("Going Further", 0, 1),
            mark("Aside", 1, 1),
        ],
        Some((1, 2)),
    );
    let fit = "Fit".to_string();
    assert_eq!(
        outline(&pdf),
        [
            (0, "Going Further".into(), 0, fit.clone()),
            (1, "Aside".into(), 0, fit),
        ]
    );
}
