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

//! Create Tagged PDF (ADR 182): the logical structure InDesign 20.0.1
//! writes (`tagged-pdf-probe` + `pdf-structure.py`) — a Document holding
//! one Article, a Story (`Sect`) per text story and a paragraph (`P`) per
//! paragraph, each referencing its text's marked content — with
//! `/MarkInfo <</Marked true>>`, `/Lang` and the document title shown.

use paged_canvas::channel::ExportPdfWireOptions;
use paged_canvas::export::CanvasExportSession;
use paged_canvas::{CanvasModel, CanvasOptions};

fn export(tagged: bool) -> String {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&paged_gen::samples::navigation::build()).expect("idml");
    let m = CanvasModel::load(
        "nav",
        &idml,
        CanvasOptions {
            fonts: vec![font],
            ..CanvasOptions::default()
        },
    )
    .expect("load");
    let wire = ExportPdfWireOptions {
        tagged,
        ..ExportPdfWireOptions::default()
    };
    let (mut session, pages) = CanvasExportSession::begin(&m, &wire).expect("begin");
    for _ in 0..pages {
        session.export_next_page().expect("page");
    }
    String::from_utf8_lossy(&session.finish().expect("finish").pdf_bytes).into_owned()
}

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

#[test]
fn a_tagged_pdf_carries_the_structure_indesign_writes() {
    let pdf = export(true);
    assert_eq!(
        count(&pdf, "/Type /StructTreeRoot"),
        1,
        "one structure tree"
    );
    assert!(pdf.contains("/Marked true"), "MarkInfo");
    assert!(pdf.contains("/DisplayDocTitle true"));
    assert!(pdf.contains("/ParentTree"));
    assert_eq!(count(&pdf, "/S /Document"), 1);
    assert_eq!(count(&pdf, "/S /Art"), 1);
    assert!(count(&pdf, "/S /Sect") >= 1, "a Story per text story");
    let paragraphs = count(&pdf, "/S /P\n") + count(&pdf, "/S /P ") + count(&pdf, "/S /P/");
    assert!(
        paragraphs >= count(&pdf, "/S /Sect"),
        "every story holds a paragraph"
    );
    assert!(count(&pdf, "/StructParents") >= 1, "pages map their marks");
}

#[test]
fn an_untagged_pdf_has_no_structure() {
    let pdf = export(false);
    assert!(!pdf.contains("/StructTreeRoot"));
    assert!(!pdf.contains("/MarkInfo"));
    assert!(!pdf.contains("/StructParents"));
}

#[test]
fn language_names_map_to_bcp47() {
    use paged_canvas::export::bcp47_of;
    assert_eq!(bcp47_of("$ID/English: USA").as_deref(), Some("en-US"));
    assert_eq!(
        bcp47_of("Language/$ID/English%3a UK").as_deref(),
        Some("en-GB")
    );
    assert_eq!(bcp47_of("$ID/de_DE_2006").as_deref(), Some("de-DE"));
    assert_eq!(bcp47_of("$ID/German: Swiss").as_deref(), Some("de-CH"));
    assert_eq!(bcp47_of("$ID/[No Language]"), None);
}

/// The tagged export of `navigation`, written for `pdf-structure.py` to
/// read beside InDesign's (`tagged-pdf-probe`):
/// `PAGED_TAGGED_PDF=<path> cargo test -p paged-canvas --test tagged_pdf
/// -- --ignored`.
#[test]
#[ignore]
fn writes_the_tagged_pdf() {
    let path = std::env::var("PAGED_TAGGED_PDF").expect("PAGED_TAGGED_PDF");
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&paged_gen::samples::navigation::build()).expect("idml");
    let m = CanvasModel::load(
        "nav",
        &idml,
        CanvasOptions {
            fonts: vec![font],
            ..CanvasOptions::default()
        },
    )
    .expect("load");
    let wire = ExportPdfWireOptions {
        tagged: true,
        ..ExportPdfWireOptions::default()
    };
    let (mut session, pages) = CanvasExportSession::begin(&m, &wire).expect("begin");
    for _ in 0..pages {
        session.export_next_page().expect("page");
    }
    std::fs::write(path, session.finish().expect("finish").pdf_bytes).expect("write");
}
