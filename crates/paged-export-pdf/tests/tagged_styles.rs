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

//! ADR 182 — a paragraph style's PDF export tag is its role in a tagged
//! PDF, as InDesign 20.0.1 writes it (`tagged-styles-probe`; the fixture
//! is InDesign's own package): `Heading` (tag H1) and `Sub` (H2) map to
//! their headings, the untagged `Body` and the `Artifact`-tagged `Folio`
//! to `P`, and the containers `Article` and `Story` to `Art` and `Sect`.

use lopdf::{Document, Object};
use paged_export_pdf::{export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn export() -> Vec<u8> {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tagged-styles-indesign.idml"),
    )
    .unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    // The importer reads each style's `<StyleExportTagMap ExportType="PDF">`.
    for (name, tag) in [("Heading", "H1"), ("Sub", "H2"), ("Folio", "Artifact")] {
        let def = document
            .styles
            .paragraph_styles
            .values()
            .find(|d| d.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("style {name}"));
        assert_eq!(def.pdf_export_tag.as_deref(), Some(tag), "{name}");
    }
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

#[test]
fn a_styles_pdf_tag_is_its_role() {
    let pdf = Document::load_mem(&export()).expect("re-parse");
    let root = pdf
        .catalog()
        .unwrap()
        .get(b"StructTreeRoot")
        .and_then(Object::as_reference)
        .expect("structure tree");
    let role_map = pdf
        .get_dictionary(root)
        .unwrap()
        .get(b"RoleMap")
        .unwrap()
        .as_dict()
        .expect("role map");
    let mut roles: Vec<(String, String)> = role_map
        .iter()
        .map(|(k, v)| {
            (
                String::from_utf8_lossy(k).into_owned(),
                String::from_utf8_lossy(v.as_name().unwrap()).into_owned(),
            )
        })
        .collect();
    roles.sort();
    // InDesign's role map for the same package, read by pdf-structure.py.
    let indesign = [
        ("Article", "Art"),
        ("Body", "P"),
        ("Folio", "P"),
        ("Heading", "H1"),
        ("Story", "Sect"),
        ("Sub", "H2"),
    ];
    assert_eq!(
        roles,
        indesign
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect::<Vec<_>>()
    );
}
