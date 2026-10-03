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

//! ADR 026 — generated pages for growing stories.
//!
//! A story with a [`paged_model::FlowGrowRule`] gets pages appended after
//! the page of its chain's last frame, the way InDesign 2025's Smart Text
//! Reflow adds them (measured with the `reflow` paged-gen fixture and
//! `tools/indesign-export/reflow-probe.sh`):
//!
//! - a generated page copies the size and applied master of that page;
//! - its frame is the page's MARGIN BOX (36 pt all round when the page
//!   declares no margins, InDesign's default), with the margin grid's
//!   columns, threaded onto the chain;
//! - the frame has DEFAULT frame options: no insets, the default first
//!   baseline, no fill or stroke (unless the rule sets
//!   `copy_frame_options`, as a Word section does: then the chain's insets,
//!   first baseline and vertical justification carry over).
//!
//! Generated pages are derived. [`Document::with_generated_pages`] returns a
//! COPY of the document with them materialised; the document itself (what
//! is saved, what undo records) never holds them. Their ids are stable, from
//! the story id and the page's ordinal, so selections and caches survive a
//! regrow, and they are XML names (`u1_grow2_page`), because an IDML export
//! writes them as `Self` attributes and part file names.

use std::collections::HashMap;

use paged_model::{Bounds, MarginPreference, Spread};

use crate::Document;

/// InDesign's margin when a page declares none (measured: the generated
/// frame of a margin-less 372 x 472 pt page came out at 36..336 x 36..436).
pub const DEFAULT_MARGIN_PT: f32 = 36.0;

/// The id of the `k`-th (1-based) generated page of `story`.
pub fn generated_page_id(story: &str, k: u32) -> String {
    format!("{story}_grow{k}_page")
}

/// The id of the `k`-th (1-based) generated text frame of `story`.
pub fn generated_frame_id(story: &str, k: u32) -> String {
    format!("{story}_grow{k}_frame")
}

/// `k` when `frame_id` is [`generated_frame_id`]`(story, k)`.
pub fn generated_frame_ordinal(story: &str, frame_id: &str) -> Option<u32> {
    frame_id
        .strip_prefix(story)?
        .strip_prefix("_grow")?
        .strip_suffix("_frame")?
        .parse()
        .ok()
}

/// The id of the spread holding the `k`-th generated page of `story`.
pub fn generated_spread_id(story: &str, k: u32) -> String {
    format!("{story}_grow{k}_spread")
}

impl Document {
    /// Stories whose chain grows (they carry a grow rule and have at least
    /// one frame), in document order.
    pub fn growing_stories(&self) -> Vec<String> {
        self.stories
            .iter()
            .filter(|s| s.story.grow.is_some())
            .map(|s| s.self_id.clone())
            .filter(|id| !self.frame_chain(id).is_empty())
            .collect()
    }

    /// A copy of this document with `counts[story]` generated pages
    /// appended to each growing story's chain. Stories without a grow rule
    /// or without frames are ignored; a count of 0 adds nothing.
    pub fn with_generated_pages(&self, counts: &HashMap<String, u32>) -> Document {
        let mut doc = self.clone();
        let mut stories: Vec<&String> = counts.keys().collect();
        stories.sort();
        for story in stories {
            let n = counts[story];
            if n == 0
                || !self
                    .stories
                    .iter()
                    .any(|s| &s.self_id == story && s.story.grow.is_some())
            {
                continue;
            }
            let rule = self
                .stories
                .iter()
                .find(|s| &s.self_id == story)
                .and_then(|s| s.story.grow.clone())
                .unwrap_or_default();
            doc.append_generated_pages(story, n, &rule);
        }
        // The stories are untouched, so the clone's story-derived indexes
        // (the heading-anchor table) are already right; each append left
        // the frame indexes current.
        doc
    }

    fn append_generated_pages(&mut self, story: &str, n: u32, rule: &paged_model::FlowGrowRule) {
        let Some(last) = self.frame_chain(story).last().map(|f| (*f).clone()) else {
            return;
        };
        let Some(last_id) = last.self_id.clone() else {
            return;
        };
        let Some(&(spread_idx, _)) = self.text_frame_index.get(&last_id) else {
            return;
        };
        let template_spread = self.spreads[spread_idx].spread.clone();
        let page_idx =
            crate::page_index_for_bounds(&template_spread.pages, last.bounds, last.item_transform)
                .unwrap_or(0);
        let Some(template_page) = template_spread.pages.get(page_idx).cloned() else {
            return;
        };
        let margins = template_page
            .self_id
            .as_ref()
            .and_then(|id| template_spread.page_margins.get(id))
            .cloned()
            .unwrap_or(MarginPreference {
                top: DEFAULT_MARGIN_PT,
                bottom: DEFAULT_MARGIN_PT,
                left: DEFAULT_MARGIN_PT,
                right: DEFAULT_MARGIN_PT,
                column_count: 1,
                column_gutter: 12.0,
            });
        let pb = template_page.bounds;
        let margin_box = Bounds {
            top: pb.top + margins.top,
            left: pb.left + margins.left,
            bottom: pb.bottom - margins.bottom,
            right: pb.right - margins.right,
        };

        // Where the chain's current last frame sits. Spreads are only ever
        // inserted AFTER it, so its position stays valid without
        // re-indexing the whole document per generated page.
        let Some(&first_link) = self.text_frame_index.get(&last_id) else {
            return;
        };
        let mut prev_at = first_link;
        let mut insert_at = spread_idx + 1;
        for k in 1..=n {
            let page_id = generated_page_id(story, k);
            let frame_id = generated_frame_id(story, k);

            let mut page = template_page.clone();
            page.self_id = Some(page_id.clone());
            // The renderer numbers an unnamed page by its position.
            page.name = None;
            page.override_list = Vec::new();

            let mut frame = last.clone();
            frame.self_id = Some(frame_id.clone());
            frame.parent_story = Some(story.to_string());
            frame.bounds = margin_box;
            // Frame inner coords = page inner coords.
            frame.item_transform = template_page.item_transform;
            frame.next_text_frame = None;
            // InDesign's new frame takes DEFAULT options, not the chain's.
            frame.anchors = Vec::new();
            frame.subpath_starts = Vec::new();
            frame.subpath_open = Vec::new();
            frame.fill_color = None;
            frame.fill_tint = None;
            frame.stroke_color = None;
            frame.stroke_weight = None;
            frame.stroke_type = None;
            frame.stroke_gap_color = None;
            frame.stroke_gap_tint = None;
            frame.stroke_dash = Vec::new();
            frame.drop_shadow = None;
            frame.stroke_drop_shadow = None;
            if !rule.copy_frame_options {
                frame.vertical_justification = None;
                frame.first_baseline_offset = None;
                frame.minimum_first_baseline_offset = None;
                frame.inset_spacing = None;
            }
            frame.auto_sizing = None;
            frame.auto_sizing_reference_point = None;
            frame.minimum_width_for_auto_sizing = None;
            frame.minimum_height_for_auto_sizing = None;
            frame.use_minimum_height_for_auto_sizing = None;
            frame.column_count = (margins.column_count > 1).then_some(margins.column_count);
            frame.column_gutter = (margins.column_count > 1).then_some(margins.column_gutter);
            frame.column_balance = None;
            frame.applied_object_style = None;
            frame.text_wrap = None;
            frame.is_anchored = false;
            frame.opacity = None;
            frame.blend_mode = None;
            frame.effects = None;
            frame.corner_radius = None;
            frame.corner_option = None;
            frame.corners = Default::default();

            let mut page_margins = HashMap::new();
            page_margins.insert(page_id.clone(), margins.clone());
            let spread = Spread {
                self_id: Some(generated_spread_id(story, k)),
                item_transform: template_spread.item_transform,
                pages: vec![page],
                text_frames: vec![frame],
                page_margins,
                ..Default::default()
            };

            // Thread the previous chain end onto the new frame.
            let (si, fi) = prev_at;
            self.spreads[si].spread.text_frames[fi].next_text_frame = Some(frame_id);
            self.spreads.insert(
                insert_at,
                crate::ParsedSpread {
                    src: format!("Spreads/Spread_{}.xml", generated_spread_id(story, k)),
                    spread,
                },
            );
            prev_at = (insert_at, 0);
            insert_at += 1;
        }
        // Once per story, not per page: the next story's chain lookup needs
        // the shifted spread indices.
        self.rebuild_frame_indexes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(generated_page_id("u1", 2), "u1_grow2_page");
        assert_ne!(generated_frame_id("u1", 1), generated_frame_id("u1", 2));
        assert_ne!(generated_spread_id("u1", 1), generated_page_id("u1", 1));
        assert_eq!(
            generated_frame_ordinal("u1", &generated_frame_id("u1", 300)),
            Some(300)
        );
        assert_eq!(generated_frame_ordinal("u1", "u12_grow3_frame"), None);
        assert_eq!(
            generated_frame_ordinal("u1", &generated_page_id("u1", 3)),
            None
        );
    }
}
