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

//! v65 — a story's ADR 026 grow rule is READABLE. `setFlowGrowRule` has
//! written it since protocol 64, and no read could tell a host whether a
//! story grows: `StorySummary.growRule` (the `stories` collection and
//! `paged.stories()`) now carries it in the setter's own shape.

use paged_canvas::channel::CollectionName;
use paged_canvas::{CanvasModel, CanvasOptions, Mutation};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::composer::build()).expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![inter_font()],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

fn rule_of(m: &CanvasModel, story: &str) -> Option<paged_canvas::channel::StoryGrowRule> {
    m.stories()
        .into_iter()
        .find(|s| s.self_id == story)
        .expect("story listed")
        .grow_rule
}

#[test]
fn the_grow_rule_reads_back_as_it_was_set() {
    let mut m = model();
    let story = paged_gen::samples::composer::body_story_id(0, 0);
    assert_eq!(rule_of(&m, &story), None, "an imported chain is fixed");

    m.apply_mutation(&Mutation::SetFlowGrowRule {
        story_id: story.clone(),
        grow: true,
        max_pages: Some(3),
        copy_frame_options: Some(true),
    })
    .expect("set");
    let rule = rule_of(&m, &story).expect("the rule reads back");
    assert!(rule.grow);
    assert_eq!(rule.max_pages, Some(3));
    assert!(rule.copy_frame_options);

    // The collection carries it in the wire spelling, shaped like the
    // setFlowGrowRule payload.
    let rows = m.collection(CollectionName::Stories);
    let row = rows
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["selfId"] == story.as_str())
        .expect("row");
    assert_eq!(
        row["growRule"],
        serde_json::json!({ "grow": true, "maxPages": 3, "copyFrameOptions": true }),
        "{row}"
    );
    // Other stories still read a fixed chain.
    let other = paged_gen::samples::composer::body_story_id(0, 1);
    assert_eq!(rule_of(&m, &other), None);

    m.undo().expect("undo");
    assert_eq!(rule_of(&m, &story), None, "undo clears the rule");

    m.apply_mutation(&Mutation::SetFlowGrowRule {
        story_id: story.clone(),
        grow: true,
        max_pages: None,
        copy_frame_options: None,
    })
    .expect("set defaults");
    let rule = rule_of(&m, &story).expect("rule");
    assert_eq!(rule.max_pages, None);
    assert!(!rule.copy_frame_options);
    m.apply_mutation(&Mutation::SetFlowGrowRule {
        story_id: story.clone(),
        grow: false,
        max_pages: None,
        copy_frame_options: None,
    })
    .expect("clear");
    assert_eq!(rule_of(&m, &story), None, "grow: false clears");
}
