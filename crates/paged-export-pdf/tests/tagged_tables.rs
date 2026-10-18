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

//! ADR 182 amended — a table in a tagged PDF, as InDesign 20.0.1 sets it
//! (`tagged-tables-probe`; the fixture is InDesign's own package): the
//! paragraph holding the table holds `Table` > one `TR` per row > `TH` for
//! a header row's cells, `TD` for a body row's > one `P` per cell
//! paragraph, which holds the cell's text.

use lopdf::{Document, Object, ObjectId};
use paged_export_pdf::{export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn export() -> Vec<u8> {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tagged-tables-indesign.idml"),
    )
    .unwrap();
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
            tagged: true,
            ..Default::default()
        },
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    })
    .expect("export")
    .bytes
}

/// The structure tree depth-first as `(depth, type)`.
fn tree(pdf: &Document) -> Vec<(usize, String)> {
    fn walk(pdf: &Document, id: ObjectId, depth: usize, out: &mut Vec<(usize, String)>) {
        let Ok(dict) = pdf.get_dictionary(id) else {
            return;
        };
        if let Ok(s) = dict.get(b"S").and_then(Object::as_name) {
            out.push((depth, String::from_utf8_lossy(s).into_owned()));
        }
        let kids = match dict.get(b"K") {
            Ok(Object::Array(a)) => a.clone(),
            Ok(o) => vec![o.clone()],
            Err(_) => vec![],
        };
        for k in kids {
            if let Ok(r) = k.as_reference() {
                walk(pdf, r, depth + 1, out);
            }
        }
    }
    let root = pdf
        .catalog()
        .unwrap()
        .get(b"StructTreeRoot")
        .and_then(Object::as_reference)
        .unwrap();
    let mut out = Vec::new();
    walk(pdf, root, 0, &mut out);
    out
}

#[test]
fn a_table_is_tagged_as_indesign_tags_it() {
    let pdf = Document::load_mem(&export()).expect("re-parse");
    let types: Vec<String> = tree(&pdf)
        .into_iter()
        .map(|(d, t)| format!("{d}:{t}"))
        .collect();
    // InDesign's tree for the same package (pdf-structure.py on its PDF),
    // the story's paragraphs named after their style.
    let indesign = [
        "1:Document",
        "2:Article",
        "3:Story",
        "4:NormalParagraphStyle",
        "4:NormalParagraphStyle",
        "5:Table",
        "6:TR",
        "7:TH",
        "8:P",
        "7:TH",
        "8:P",
        "6:TR",
        "7:TD",
        "8:P",
        "7:TD",
        "8:P",
        "6:TR",
        "7:TD",
        "8:P",
        "7:TD",
        "8:P",
        "4:NormalParagraphStyle",
    ];
    assert_eq!(types, indesign, "{types:#?}");
}
