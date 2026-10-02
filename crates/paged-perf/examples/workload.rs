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

//! Build the annual-scale workload and print what it contains and what
//! one write on it costs.
//!
//! ```text
//! cargo run --release -p paged-perf --example workload -- [PHOTO_PX]
//! ```

fn main() {
    let px: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1600);
    let t = std::time::Instant::now();
    let mut w = paged_perf::build(px);
    let built = w.model.built();
    let pages = built.pages.len();
    let images: usize = built.pages.iter().map(|p| p.list.images.len()).sum();
    println!(
        "built in {:.1}s: {pages} pages, {} body frames, {} photo frames, {} tables, \
         body story {} chars, {images} images in the display lists",
        t.elapsed().as_secs_f32(),
        w.body_frames.len(),
        w.image_frames.len(),
        w.table_stories.len(),
        w.body_len,
    );
    let story = w.body_story.clone();
    for _ in 0..3 {
        let t = std::time::Instant::now();
        w.model
            .apply_mutation(&paged_canvas::Mutation::InsertText {
                story_id: story.clone(),
                offset: 500,
                text: "x".into(),
                cell: None,
            })
            .expect("keystroke");
        println!(
            "keystroke: {:.1} ms  {:?}",
            t.elapsed().as_secs_f32() * 1000.0,
            w.model.last_rebuild_stats()
        );
    }
}
