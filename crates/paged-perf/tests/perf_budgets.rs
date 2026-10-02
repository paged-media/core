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

//! WORK budgets on the annual-scale workload.
//!
//! Each test does one thing a user does, then asserts how much expensive
//! work the engine did for it, through the counters in
//! `paged_compose::perf` and `RebuildStats`. Counts are deterministic, so
//! these run in the ordinary `cargo test` on any runner; wall-clock lives
//! in the benches. A budget is the measured value when it was written: a
//! change that does MORE work fails here and says which work.

use paged_canvas::Mutation;
use paged_compose::perf::{self, PerfCounters};
use paged_perf::Workload;

fn wire(json: serde_json::Value) -> Mutation {
    serde_json::from_value(json).expect("wire mutation")
}

/// Run `f` on the workload; return the work it did and the rebuilds.
fn work(w: &mut Workload, f: impl FnOnce(&mut Workload)) -> (PerfCounters, u64) {
    let rebuilds = w.model.last_rebuild_stats().rebuilds;
    let before = perf::snapshot();
    f(w);
    (
        perf::snapshot().since(&before),
        w.model.last_rebuild_stats().rebuilds - rebuilds,
    )
}

fn keystroke(w: &mut Workload, offset: u32) {
    w.model
        .apply_mutation(&Mutation::InsertText {
            story_id: w.body_story.clone(),
            offset,
            text: "x".into(),
            cell: None,
        })
        .expect("keystroke");
}

/// Loading hashes the registered fonts about once. The whole-file font
/// hash once ran per paragraph: gigabytes per rebuild on this document.
/// `font_id` runs for the faces paragraphs USE, through whichever buffer
/// the path holds, so a face reached through two buffers (registry and
/// default font) counts twice: 1.12x of every registered byte today.
#[test]
fn loading_hashes_the_fonts_about_once() {
    let fonts: u64 = std::fs::read_dir(paged_perf::corpus("fonts"))
        .expect("corpus/fonts")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "ttf" || x == "otf")
        })
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    let inter = std::fs::metadata(paged_perf::corpus("fonts/Inter.ttf"))
        .unwrap()
        .len();
    let before = perf::snapshot();
    let _w = paged_perf::build(320);
    let load = perf::snapshot().since(&before);
    let once = fonts + inter;
    assert!(
        load.font_bytes_hashed * 10 <= once * 12,
        "load + authoring hashed {} font bytes; every registered face once is {once}",
        load.font_bytes_hashed,
    );
}

/// A keystroke in the long threaded story: one rebuild, no font hashing,
/// no image decoding, and the story laid out a bounded number of times.
#[test]
fn a_keystroke_does_only_layout_work() {
    let mut w = paged_perf::build(320);
    keystroke(&mut w, 500); // warm
    let (c, rebuilds) = work(&mut w, |w| keystroke(w, 501));
    assert_eq!(rebuilds, 1, "one keystroke, one rebuild");
    assert_eq!(c.font_bytes_hashed, 0, "a keystroke re-hashed fonts: {c:?}");
    assert_eq!(
        c.pipeline_image_decodes, 0,
        "a keystroke re-decoded images: {c:?}"
    );
    assert!(
        c.story_emits <= 2,
        "a keystroke laid the story out {} times: {c:?}",
        c.story_emits
    );
}

/// A frame property write touches no text and no image bytes.
#[test]
fn a_frame_write_does_no_text_or_image_work() {
    let mut w = paged_perf::build(320);
    let frame = w.image_frames[0].clone();
    let (c, rebuilds) = work(&mut w, |w| {
        w.model
            .apply_mutation(&wire(serde_json::json!({ "op": "setElementProperty", "args": {
                "elementId": { "kind": "rectangle", "id": frame },
                "path": "frameFillColor", "value": { "type": "colorRef", "value": "Color/Black" } } })))
            .expect("frame write");
    });
    assert_eq!(rebuilds, 1);
    assert_eq!(c.font_bytes_hashed, 0, "{c:?}");
    assert_eq!(
        c.pipeline_image_decodes, 0,
        "a frame write re-decoded images: {c:?}"
    );
    // KNOWN COST, pinned so it can only go down: a write that changes no
    // text re-emits every story (52 here, vs 2 for a keystroke) — the
    // body-story emit cache is invalidated by any non-text operation.
    // Narrowing that is the next layout optimisation; lower this budget
    // when it lands.
    assert!(
        c.story_emits <= 52,
        "a frame write laid out {} stories: {c:?}",
        c.story_emits
    );
}

/// A batch of edits settles ONE rebuild, however many children it has.
#[test]
fn a_batch_of_edits_rebuilds_once() {
    let mut w = paged_perf::build(320);
    let story = w.body_story.clone();
    let ops: Vec<Mutation> = (0..20)
        .map(|i| Mutation::InsertText {
            story_id: story.clone(),
            offset: 500 + i,
            text: "y".into(),
            cell: None,
        })
        .collect();
    let (c, rebuilds) = work(&mut w, |w| {
        w.model
            .apply_mutation(&Mutation::Batch { ops })
            .expect("batch");
    });
    assert_eq!(rebuilds, 1, "a 20-child batch rebuilt {rebuilds} times");
    assert_eq!(c.font_bytes_hashed, 0, "{c:?}");
}

/// A growing story (Word-style page growth) converges in a bounded number
/// of grow passes after an edit, instead of re-doubling from zero.
#[test]
fn an_edit_in_a_growing_story_takes_few_grow_passes() {
    let mut w = paged_perf::build(320);
    let story = w.body_story.clone();
    let (grow, _) = work(&mut w, |w| {
        w.model
            .apply_mutation(&wire(serde_json::json!({ "op": "setFlowGrowRule",
                "args": { "storyId": story, "grow": true } })))
            .expect("grow rule");
    });
    let pages = w.model.built().pages.len();
    let (c, rebuilds) = work(&mut w, |w| keystroke(w, 500));
    assert!(pages > 134, "the overset story grew pages ({pages})");
    assert!(
        grow.grow_passes <= 3,
        "setting the rule took {} grow passes",
        grow.grow_passes
    );
    assert_eq!(rebuilds, 1);
    assert!(c.grow_passes <= 1, "an edit re-grew from scratch: {c:?}");
    assert!(c.story_emits <= 2, "{c:?}");
}
