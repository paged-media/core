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

//! thoughts ADR 029 — the DOCX standalone-open path, engine side. A plugin
//! produces a native SKELETON (a page per Word section, its margin-box frame,
//! a story with a grow rule) packaged by `paged_store::package::wrap_document`
//! and opened like any `.paged`. The grow rule rides inside the native model,
//! so the opened document grows by itself: Word's 5 pages for the
//! `docx-pagination` shape (paged-renderer's docx_pagination_pipeline.rs
//! pins the per-page content against Word).

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_gen::samples::docx_pagination::{section_story_id, sections};

fn inter() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).expect("read Inter.ttf")
}

#[test]
fn a_native_skeleton_with_grow_rules_opens_and_grows() {
    let idml = paged_gen::write_idml(&paged_gen::samples::docx_pagination::build()).expect("idml");
    let mut doc = idml_import::import_idml_doc(&idml).expect("import");
    let ids: Vec<String> = (0..sections().len() as u32).map(section_story_id).collect();
    for s in doc.stories.iter_mut() {
        if ids.contains(&s.self_id) {
            s.story.grow = Some(paged_model::FlowGrowRule {
                copy_frame_options: true,
                ..Default::default()
            });
        }
    }
    let paged =
        paged_store::package::wrap_document(&doc, "Imported.docx", 612.0, 792.0).expect("wrap");

    let opts = CanvasOptions {
        fonts: vec![inter()],
        ..CanvasOptions::default()
    };
    let model = CanvasModel::load("doc1", &paged, opts).expect("load .paged");
    assert_eq!(
        model.scene().growing_stories().len(),
        2,
        "the grow rules survive the native round trip"
    );
    assert_eq!(
        model.built().pages.len(),
        5,
        "Word paginates this document to 5 pages"
    );
}
