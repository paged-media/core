//! Plan-2 §8.3 — load a real InDesign export that ships `<Guide>`
//! elements and confirm `CanvasModel::handle()` surfaces them on
//! `DocumentHandle.ruler_guides`. Resume-template-teacher carries 14
//! guides across its body pages.

use std::path::PathBuf;

use idml_canvas::{CanvasModel, CanvasOptions};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("envato")
        .join("packs")
        .join("resume-template-teacher")
        .join("template.idml")
}

#[test]
fn document_handle_exposes_ruler_guides_from_real_idml() {
    let path = fixture_path();
    if !path.exists() {
        // Envato pack not checked out — skip rather than fail. The
        // packs are gitignored; CI hosts may not have them.
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let bytes = std::fs::read(&path).expect("read fixture");
    let opts = CanvasOptions::default();
    let model = CanvasModel::load("doc-rg", &bytes, opts).expect("load + build");
    let handle = model.handle();
    assert!(
        handle.ruler_guides.len() >= 14,
        "expected >= 14 guides in resume-template-teacher; got {}",
        handle.ruler_guides.len(),
    );
    // Each guide should reference a real page id from the handle.
    let page_ids: std::collections::HashSet<&str> =
        handle.page_ids.iter().map(|p| p.as_str()).collect();
    for g in &handle.ruler_guides {
        assert!(
            page_ids.contains(g.page_id.as_str()),
            "guide page_id {:?} not in document's pages",
            g.page_id,
        );
    }
}
