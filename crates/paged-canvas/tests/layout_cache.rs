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

//! Phase 4 Step 1 — per-paragraph layout cache.
//!
//! End-to-end check that the persistent `LayoutCache` on `CanvasModel`
//! actually wins on mutation rebuilds: type a character into one
//! paragraph of a multi-paragraph story and assert the rebuild's cache
//! hit count exceeds zero (i.e. some other paragraph reused its
//! cached layout instead of re-running Knuth-Plass).

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_canvas::channel::Mutation;

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name))
        .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Build an IDML with a single story containing `n` short paragraphs.
/// Each paragraph carries unique text so the cache can't share entries
/// across paragraphs by accident — every paragraph hits the K-P engine
/// once during the cold build.
fn build_multipara_idml(n: usize) -> Vec<u8> {
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
    <Page Self="p1" GeometricBounds="0 0 800 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 760 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    let mut story = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
"#,
    );
    for i in 0..n {
        story.push_str(&format!(
            r#"    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Paragraph number {i} carries unique text so the cache cannot share entries by accident across rows.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
"#,
        ));
    }
    story.push_str("  </Story>\n</idPkg:Story>");
    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

fn load_model(n_paragraphs: usize) -> CanvasModel {
    let bytes = build_multipara_idml(n_paragraphs);
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("doc", &bytes, opts).expect("load + build")
}

#[test]
fn cold_build_records_misses() {
    // Initial build: every paragraph is fresh; the cache should
    // record N misses and 0 hits.
    let model = load_model(8);
    let stats = model.layout_cache_stats();
    assert!(
        stats.misses >= 8,
        "cold build should record ≥ N=8 misses, got {stats:?}"
    );
    assert_eq!(
        stats.hits, 0,
        "cold build cannot record any hits, got {stats:?}"
    );
}

#[test]
fn mutation_rebuild_hits_unchanged_paragraphs() {
    // Build with 8 distinct paragraphs. Mutate the first paragraph.
    // The rebuild must hit at least the OTHER 7 paragraphs that
    // didn't change (≥ 7 hits). Exact numbers depend on the
    // pipeline's per-emit re-walks but the floor is "≥ N-1 hits".
    let mut model = load_model(8);
    let m = Mutation::InsertText {
        story_id: "u10".into(),
        offset: 0,
        text: "X".into(),
    };
    model.apply_mutation(&m).expect("apply mutation");
    let stats = model.layout_cache_stats();
    assert!(
        stats.hits >= 7,
        "rebuild after 1-para edit should hit ≥ N-1 = 7 paragraphs, got {stats:?}"
    );
    // The edited paragraph itself must produce a miss — it's a fresh
    // text input the cache has never seen.
    assert!(
        stats.misses >= 1,
        "rebuild must miss the edited paragraph, got {stats:?}"
    );
}

#[test]
fn pages_for_story_returns_chain_pages() {
    // The fixture's single story `u10` lives on page `p1` (only one
    // frame, no chaining). `pages_for_story` should report exactly
    // that page.
    let model = load_model(3);
    let pages = model.pages_for_story("u10");
    assert_eq!(pages.len(), 1, "story u10 has one frame on one page");
    assert_eq!(pages[0].0, "p1");

    // Unknown story → empty.
    assert!(model.pages_for_story("does-not-exist").is_empty());

    // Page-index variant agrees.
    let indices = model.page_indices_for_story("u10");
    assert_eq!(indices, vec![0]);
}

#[test]
fn pages_for_story_updates_after_mutation() {
    // The InsertText mutation doesn't add new frames or pages, so
    // the story's page set is identical before and after.
    let mut model = load_model(3);
    let before = model.pages_for_story("u10").to_vec();
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "Z".into(),
        })
        .unwrap();
    let after = model.pages_for_story("u10").to_vec();
    assert_eq!(before, after, "story-pages map should survive a same-page edit");
}

#[test]
fn second_identical_rebuild_is_all_hits() {
    // Apply a mutation, then apply its inverse: scene returns to its
    // initial state. The cache should serve every paragraph (including
    // the once-edited one — both its pre- and post-edit text are now
    // in the cache).
    let mut model = load_model(4);
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "u10".into(),
            offset: 0,
            text: "X".into(),
        })
        .unwrap();
    model
        .apply_mutation(&Mutation::DeleteRange {
            story_id: "u10".into(),
            start: 0,
            end: 1,
        })
        .unwrap();
    let stats = model.layout_cache_stats();
    assert_eq!(
        stats.misses, 0,
        "after round-trip mutation every paragraph should be cached, got {stats:?}"
    );
    assert!(stats.hits >= 4, "expected ≥ 4 hits, got {stats:?}");
}
