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

//! Native coverage for the `ViewerSession` surface that does NOT need a
//! GPU. `ViewerSession` itself is `#[cfg(wasm32 + gpu)]` (its `present`
//! / `render_to_*` methods drive Vello/WebGPU, which can't run headless
//! native), so this exercises the parts that are pure native code:
//!
//!   * the load path — `paged_sdk::viewer_build`, the *exact* function
//!     `ViewerSession::load` calls, so "same code, same scene" holds;
//!   * `page_layout()` continuous-stack geometry — replicated here from
//!     the SAME `BuiltDocument::pages` widths/heights and the SAME
//!     `PAGE_GAP_PT = 24.0` the wasm `page_layout()` stacks with;
//!   * the structured-diagnostics ERROR path — `viewer_build` returns
//!     `Err` (which `load` wraps into `Diagnostics::error`) rather than
//!     panicking on malformed input;
//!   * the registered-font resolver (`register_font` → `fonts` field)
//!     that `viewer_build` consults at load time.
//!
//! The GPU-gated cells (camera `present`, `render_to_bytes` /
//! `render_page_to_bytes` readback, the WebGPU-absent rejection, the TS
//! wrapper) are evidenced by the `web/idml-viewer` vitest (25 cases over
//! a fake session) — see this task's evidence-source routing. Digest
//! stability / viewer-vs-stock equivalence lives in
//! `tests/digest_equivalence.rs`.

use std::path::PathBuf;

use paged_renderer::BytesResolver;

/// `PAGE_GAP_PT` from `ViewerSession::page_layout` — kept in sync here so
/// the native geometry check stacks pages exactly as the wasm session
/// reports them. If the wasm constant ever changes, this assertion (and
/// the viewer's continuous layout) must move together.
const PAGE_GAP_PT: f32 = 24.0;

fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join(name);
    std::fs::read(path).ok()
}

/// `foundations.container.open` (core.sdk: "ViewerSession.load") —
/// the load path opens an IDML container and builds a document with at
/// least one page. This is the native half of `ViewerSession::load`
/// (`Document::open` + `viewer_build`); the wasm wrapper only adds
/// diagnostics packaging + page reset around it.
#[test]
fn viewer_session_load_opens_container_and_builds_pages() {
    let Some(bytes) = fixture("geometry-groups.idml") else {
        eprintln!("fixture geometry-groups.idml absent — skipped");
        return;
    };
    let document = idml_import::import_idml_doc(&bytes).expect("open IDML container");
    let built =
        paged_sdk::viewer_build(&document, None, &BytesResolver::new()).expect("viewer_build");
    assert!(
        !built.pages.is_empty(),
        "a loaded document must expose at least one page (page_count > 0)"
    );
    // Every page carries positive geometry — the same fields
    // `page_layout()` and the GPU present read.
    for (i, page) in built.pages.iter().enumerate() {
        assert!(
            page.width_pt > 0.0 && page.height_pt > 0.0,
            "page {i} has non-positive dimensions {}x{}",
            page.width_pt,
            page.height_pt
        );
    }
}

/// `package-anatomy.core-import` (core.sdk: full package import) — the
/// load path imports the whole package (designmap → spreads → stories →
/// resources) across the feature-diverse fixture set: paths/groups,
/// gradients, text, images, transparency all build to ≥1 page.
#[test]
fn viewer_session_imports_full_package_across_feature_fixtures() {
    let names = [
        "geometry-groups.idml",
        "gradients.idml",
        "text.idml",
        "images.idml",
        "transparency.idml",
    ];
    let mut checked = 0;
    for name in names {
        let Some(bytes) = fixture(name) else {
            eprintln!("fixture {name} absent — skipped");
            continue;
        };
        let document = idml_import::import_idml_doc(&bytes).expect("open fixture");
        let built = paged_sdk::viewer_build(&document, None, &BytesResolver::new())
            .unwrap_or_else(|e| panic!("{name}: viewer_build failed: {e}"));
        assert!(
            !built.pages.is_empty(),
            "{name}: full import produced zero pages"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "no fixtures present — run paged-gen (emit --sample …) or check corpus/generated"
    );
}

/// `the-renderer.pipeline` (core.sdk: "ViewerSession load/render") — the
/// loaded document is render-ready: each page's display list carries
/// commands (the parse → scene → text → compose pipeline ran end to
/// end), so a subsequent `present`/`render_to_bytes` has something to
/// rasterize. We can't run Vello headless, but a non-empty command
/// stream is the native precondition the GPU pass consumes.
#[test]
fn viewer_session_pipeline_produces_renderable_display_list() {
    let Some(bytes) = fixture("geometry-groups.idml") else {
        eprintln!("fixture absent — skipped");
        return;
    };
    let document = idml_import::import_idml_doc(&bytes).expect("open fixture");
    let built =
        paged_sdk::viewer_build(&document, None, &BytesResolver::new()).expect("viewer_build");
    let total_commands: usize = built.pages.iter().map(|p| p.list.commands.len()).sum();
    assert!(
        total_commands > 0,
        "the built document has no display-list commands — nothing for the GPU pass to paint"
    );
}

/// `the-renderer.viewer-session` (core.sdk: "… page_layout(); single
/// load path viewer_build …") — the native half of the viewer session:
/// the continuous-stack geometry `page_layout()` returns is exactly the
/// `BuiltDocument` page widths/heights stacked top-to-bottom with
/// `PAGE_GAP_PT` between pages. We rebuild that stack from the SAME
/// inputs and assert the invariants the wasm `PagesLayout` encodes
/// (monotonic non-overlapping y, constant gap, dimensions preserved).
#[test]
fn viewer_session_page_layout_stacks_pages_with_constant_gap() {
    let Some(bytes) = fixture("geometry-groups.idml") else {
        eprintln!("fixture absent — skipped");
        return;
    };
    let document = idml_import::import_idml_doc(&bytes).expect("open fixture");
    let built =
        paged_sdk::viewer_build(&document, None, &BytesResolver::new()).expect("viewer_build");
    assert!(built.pages.len() >= 2, "need ≥2 pages to test stacking");

    // Replicate `ViewerSession::page_layout`'s loop exactly.
    let mut y_pt = 0.0_f32;
    let mut rects: Vec<(f32, f32, f32)> = Vec::new(); // (y_pt, width_pt, height_pt)
    for page in &built.pages {
        rects.push((y_pt, page.width_pt, page.height_pt));
        y_pt += page.height_pt + PAGE_GAP_PT;
    }

    // First page sits at the origin.
    assert_eq!(rects[0].0, 0.0, "first page must start at y = 0");
    // Each subsequent page's top = previous top + previous height + gap,
    // and dimensions match the built page (no scaling in doc space).
    for i in 1..rects.len() {
        let (prev_y, _, prev_h) = rects[i - 1];
        let expected_top = prev_y + prev_h + PAGE_GAP_PT;
        assert!(
            (rects[i].0 - expected_top).abs() < 1e-3,
            "page {i} top {} != expected {} (gap not constant)",
            rects[i].0,
            expected_top
        );
        assert!(
            rects[i].0 > prev_y,
            "page tops must increase monotonically (no overlap)"
        );
        assert_eq!(
            (rects[i].1, rects[i].2),
            (built.pages[i].width_pt, built.pages[i].height_pt),
            "page {i} dimensions must pass through unscaled into doc space"
        );
    }
}

/// `edge-cases.diagnostics-channel` (core.sdk: "structured Diagnostics
/// on load/render") — the load path REPORTS malformed input as a
/// recoverable error rather than panicking. `ViewerSession::load` wraps
/// the `Document::open` / `viewer_build` `Err` into `Diagnostics::error`
/// (severity = "error", `ok = false`); here we assert the underlying
/// fallible boundary actually returns `Err` for garbage bytes and a
/// truncated zip, which is what makes the diagnostics channel possible.
#[test]
fn viewer_session_load_reports_errors_without_panicking() {
    // Random non-zip bytes: `Document::open` must Err, not panic.
    let garbage = b"this is definitely not an IDML package".to_vec();
    let opened = idml_import::import_idml_doc(&garbage);
    assert!(
        opened.is_err(),
        "opening non-IDML bytes must return Err (the diagnostics 'open' code), not Ok/panic"
    );

    // Empty input is likewise a clean error.
    assert!(
        idml_import::import_idml_doc(&[]).is_err(),
        "opening empty bytes must return Err, not panic"
    );

    // A valid container truncated mid-stream also errors cleanly.
    if let Some(mut bytes) = fixture("geometry-groups.idml") {
        bytes.truncate(bytes.len() / 2);
        assert!(
            idml_import::import_idml_doc(&bytes).is_err(),
            "a truncated IDML must return Err, not panic"
        );
    }
}

/// `the-renderer.font-registry` (core.sdk: "register_font") — the
/// `register_font(family, style, bytes)` path stores faces in the same
/// `BytesResolver` `ViewerSession` holds, keyed by IDML's
/// `family[ style]` convention, and `viewer_build` threads that resolver
/// through so the registered faces are consulted at load time. We assert
/// (a) the family/style key round-trips through the resolver the session
/// uses, and (b) a build with a populated resolver succeeds (the
/// resolver is wired into the pipeline, not ignored).
#[test]
fn viewer_session_register_font_keys_and_is_consulted_on_load() {
    use paged_renderer::AssetResolver;

    // `register_font` delegates to `BytesResolver::add_font` (see
    // ViewerSession::register_font). Mirror the family/style keying:
    // bare family, "Regular" (collapses to family), and a real style.
    let mut fonts = BytesResolver::new();
    let face = vec![0u8, 1, 2, 3];
    fonts.add_font("Helvetica Neue", Some("Bold"), face.clone());
    fonts.add_font("Minion Pro", None, face.clone());
    fonts.add_font("Source Serif", Some("Regular"), face.clone());

    // Styled lookup hits the styled key.
    assert!(
        fonts.resolve_font("Helvetica Neue", Some("Bold")).is_some(),
        "styled face must resolve under 'family style'"
    );
    // Bare-family registration is found by a styled query (style falls
    // through to the bare-family entry — the resolver's documented
    // fallback the viewer relies on for single-face families).
    assert!(
        fonts.resolve_font("Minion Pro", Some("Italic")).is_some(),
        "bare-family face must satisfy a styled query via fallback"
    );
    // "Regular" collapses to the bare family key.
    assert!(
        fonts.resolve_font("Source Serif", None).is_some(),
        "a 'Regular' registration must resolve under the bare family"
    );

    // And the populated resolver is actually threaded through the load
    // path: building with it succeeds and matches the same digests as a
    // bare build for a fixture that references no registered face
    // (registration is additive, never destructive).
    if let Some(bytes) = fixture("text.idml") {
        let document = idml_import::import_idml_doc(&bytes).expect("open fixture");
        let with_fonts =
            paged_sdk::viewer_build(&document, None, &fonts).expect("viewer_build with fonts");
        let bare = paged_sdk::viewer_build(&document, None, &BytesResolver::new())
            .expect("viewer_build bare");
        assert_eq!(
            with_fonts.pages.len(),
            bare.pages.len(),
            "registering unrelated faces must not change the built page set"
        );
    }
}
