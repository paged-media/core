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

//! Seam test (C1 from `docs/inspector.md`): construct a `Document`
//! by hand in Rust, with no IDML / ZIP / XML involved, and feed it to
//! the renderer pipeline. If this test starts requiring access to
//! parser-internal state or ZIP entries, the parser has leaked into
//! the scene graph and the leak should be fixed before more renderer
//! work proceeds.
//!
//! Two scenarios:
//!
//!   1. **Empty document** — no spreads, no stories. The pipeline
//!      falls back to a default letter-sized canvas. This is the
//!      minimum-viable hand-construction case.
//!
//!   2. **One synthetic page with no content** — a single `Spread`
//!      containing one `Page` with explicit bounds. Verifies that the
//!      hand-built spread participates in the page-routing pass.
//!
//! Adding glyph emission would require constructing a `Story` +
//! `ParagraphStyleRange` + `CharacterStyleRange` chain plus reaching
//! into the font table machinery; that's a follow-up the inspector
//! work can pull in when it actually needs it. The two scenarios here
//! are enough to detect parser leaks at the seam.

use std::collections::HashMap;

use paged_parse::{Bounds, DesignMap, Graphic, Page, Spread, StyleSheet};
use paged_renderer::pipeline::{self, PipelineOptions};
use paged_scene::{Document, ParsedSpread};

#[test]
fn build_document_accepts_a_hand_constructed_empty_document() {
    let document = Document {
        source: None,
        designmap: DesignMap::default(),
        palette: Graphic::default(),
        spreads: Vec::new(),
        stories: Vec::new(),
        master_spreads: HashMap::new(),
        frame_for_story: HashMap::new(),
        text_frame_index: HashMap::new(),
        styles: StyleSheet::default(),
        anchors: Vec::new(),
    };

    let built = pipeline::build_document(&document, &PipelineOptions::default())
        .expect("pipeline must accept an empty hand-constructed document");

    assert_eq!(
        built.pages.len(),
        1,
        "empty document should fall back to a single letter-sized page"
    );
    assert_eq!(built.pages[0].width_pt, 612.0);
    assert_eq!(built.pages[0].height_pt, 792.0);
}

#[test]
fn build_document_accepts_a_hand_constructed_single_page_document() {
    let page_bounds = Bounds {
        top: 0.0,
        left: 0.0,
        bottom: 200.0,
        right: 300.0,
    };
    let mut spread = Spread::default();
    spread.pages.push(Page {
        self_id: Some("Page/u1".to_string()),
        name: Some("1".to_string()),
        bounds: page_bounds,
        item_transform: None,
        applied_master: None,
        master_page_transform: None,
        override_list: Vec::new(),
        show_master_items: None,
    });

    let document = Document {
        source: None,
        designmap: DesignMap::default(),
        palette: Graphic::default(),
        spreads: vec![ParsedSpread {
            src: "Spreads/synth.xml".to_string(),
            spread,
        }],
        stories: Vec::new(),
        master_spreads: HashMap::new(),
        frame_for_story: HashMap::new(),
        text_frame_index: HashMap::new(),
        styles: StyleSheet::default(),
        anchors: Vec::new(),
    };

    let built = pipeline::build_document(&document, &PipelineOptions::default())
        .expect("pipeline must accept a single-page hand-constructed document");

    assert_eq!(built.pages.len(), 1);
    assert_eq!(built.pages[0].width_pt, 300.0);
    assert_eq!(built.pages[0].height_pt, 200.0);
}
