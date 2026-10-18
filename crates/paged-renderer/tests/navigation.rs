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

//! W4.8 — end-to-end document-navigation resolution over the
//! `navigation` generated fixture.
//!
//! The distinctive effects this pack asserts:
//!   * the resolved INDEX story lists both topic markers (the two
//!     `<PageReference>` markers, grouped + alphabetised by topic, each
//!     with the page it landed on), and
//!   * the resolved TOC lists both `Heading`-styled paragraphs in
//!     document order, each carrying its page number.
//!
//! These go through `Document::resolve_index_tree_on` / `index_paragraphs`
//! and `Document::resolve_toc` — the passes behind Generate Index and
//! Update Table of Contents (ADRs 165, 166).

use paged_gen::samples::navigation;
use paged_renderer::pipeline::index_paragraphs;
use paged_renderer::Document;

fn open() -> Document {
    let bytes = paged_gen::write_idml(&navigation::build()).unwrap();
    idml_import::import_idml_doc(&bytes).unwrap()
}

#[test]
fn resolved_index_story_lists_both_topic_markers() {
    let doc = open();
    // The fixture is a single body page → flat page index 0, labelled "1".
    let page_labels = vec!["1".to_string()];
    let entries = doc.resolve_index_tree_on(&Default::default());
    let paragraphs = index_paragraphs(doc.designmap.index_options.as_ref(), entries, &page_labels);

    // The title, then per letter its heading and its topics, alphabetised:
    // "Apples" before "Pears", each behind two spaces on page "1".
    let texts: Vec<String> = paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect())
        .collect();
    assert_eq!(texts, ["Index", "A", "Apples  1", "P", "Pears  1"]);
}

#[test]
fn resolved_toc_lists_both_headings_in_order() {
    let doc = open();
    let toc = doc
        .styles
        .toc_styles
        .get(navigation::TOC_STYLE)
        .expect("TOC style");
    // `resolve_toc` is the scene pass `build_toc_paragraphs` feeds from;
    // walk it directly so the test stays decoupled from the renderer's
    // private synthesis helper.
    let entries = doc.resolve_toc(toc);

    // Both headings feed the TOC, in document order.
    assert_eq!(
        entries.len(),
        2,
        "the TOC must list both headings, got {:?}",
        entries.iter().map(|e| e.text.clone()).collect::<Vec<_>>(),
    );
    assert_eq!(entries[0].text, navigation::HEADING_ONE, "first TOC entry");
    assert_eq!(entries[1].text, navigation::HEADING_TWO, "second TOC entry");
    // Both resolve to the single body page (flat index 0), so the
    // renderer can append a page number.
    assert_eq!(entries[0].page_number, Some(0), "heading one's page");
    assert_eq!(entries[1].page_number, Some(0), "heading two's page");
}
