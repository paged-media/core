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

//! Body-story signatures + master/body display-list delta splicing + path-id rebasing. Extracted from pipeline/mod.rs (1.6b).

use super::*;

use paged_parse::TextFrame;



/// Measure-only pass for one cell paragraph: shapes + lays out at
/// `column_width_pt` and returns the vertical extent the paragraph
/// would consume, without emitting glyphs. Mirrors
/// [`emit_cell_paragraph`]'s layout half so content-driven row
/// growth can sum cell heights before committing row geometry.
///
/// Phase 5 — footnote pool emit. For every page that captured
/// footnotes during the story pass, lay out the footnote bodies at
/// the bottom of the host frame's content area. Bodies stack
/// upward from the frame bottom; per-page running numbers prefix
/// each body ("1. body text").
///
/// W1.8 scope: footnote bodies compose through the SAME styled-run path
/// as body text — per-run point size, weight (bold/italic via the wght
/// axis), and `FillColor` are honoured (`compose_footnote_paragraphs`).
/// The document's `<FootnoteOption>` separator rule is drawn above the
/// pool from real designmap settings, and the W1.7 reserve-then-fill
/// pass keeps bodies clear of the body text.
///
/// Deferred:
/// - Anchor superscript substitution at the host paragraph (the inline
///   footnote-reference number),
/// - Cross-frame footnote SPLITTING — a single footnote taller than the
///   remaining column is not split across frames; it overruns and is
///   reported via a `FootnoteOverflow` diagnostic (see the dated note on
///   `emit_footnote_pools`).
///
/// Perf-BodyStory — signature for a story's emission inputs. Hashes
/// the frame chain's (self_id, bounds, item_transform) plus the
/// wrap_rects on each chain page. A gesture that moves a frame
/// outside this set leaves the signature unchanged → cache hit.
/// Moving a frame INSIDE the chain or a frame whose wrap rect
/// lives on a chain page bumps the signature → cache miss + fresh
/// capture.
pub(super) fn body_story_signature(
    chain: &[&TextFrame],
    chain_pages: &[usize],
    wrap_rects_per_page: &[Vec<WrapShape>],
) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    // Chain — frame identity + geometry.
    chain.len().hash(&mut h);
    for f in chain {
        f.self_id.as_deref().unwrap_or("").hash(&mut h);
        f.bounds.top.to_bits().hash(&mut h);
        f.bounds.left.to_bits().hash(&mut h);
        f.bounds.bottom.to_bits().hash(&mut h);
        f.bounds.right.to_bits().hash(&mut h);
        match f.item_transform {
            Some(m) => {
                1u8.hash(&mut h);
                for v in &m {
                    v.to_bits().hash(&mut h);
                }
            }
            None => 0u8.hash(&mut h),
        }
    }
    // Wrap rects on chain pages — captures wrap-causing frames'
    // movements on pages this story touches. Other pages' wrap
    // changes don't affect this story's line breaking.
    for &page in chain_pages {
        // The page INDEX is part of the key: insert/delete-page shifts
        // the chain's absolute page indices while the frame geometry
        // stays identical, and the cached delta's per_page entries are
        // keyed by absolute index — a stale hit would splice commands
        // into the wrong page, or out of bounds entirely (the
        // editor-suite insertPage-mid-set panic).
        page.hash(&mut h);
        if let Some(rects) = wrap_rects_per_page.get(page) {
            1u8.hash(&mut h);
            rects.len().hash(&mut h);
            for r in rects {
                r.bounds.top.to_bits().hash(&mut h);
                r.bounds.left.to_bits().hash(&mut h);
                r.bounds.bottom.to_bits().hash(&mut h);
                r.bounds.right.to_bits().hash(&mut h);
                for (cx, cy) in &r.corners {
                    cx.to_bits().hash(&mut h);
                    cy.to_bits().hash(&mut h);
                }
            }
        } else {
            0u8.hash(&mut h);
        }
    }
    h.finish()
}

/// Perf-MasterText — splice a cached delta into a page's display
/// list. Appends the delta's path entries (via `push_anon`, no
/// intern dedup — the rebuild's master+frame pass may have already
/// interned the same glyph outlines under different ids, but that
/// wastes a few path slots and not correctness), then pushes the
/// cached commands with their relative path-ids rebased to the
/// page's NEW path-buffer base.
pub(super) fn splice_master_text_delta(list: &mut paged_compose::DisplayList, delta: &MasterTextEmitDelta) {
    let new_base = list.paths.len() as i64;
    for path in &delta.paths {
        list.paths.push_anon(path.clone());
    }
    for cmd in &delta.commands {
        let mut c = cmd.clone();
        rebase_path_ids(&mut c, new_base);
        list.commands.push(c);
    }
}

/// Perf-BodyStory — splice one page's captured body-story emission
/// into a `BuiltPage`: rebase + push the path+command delta, and
/// extend `story_layout` + `footnotes` so caret / hit-test /
/// footnote queries match a from-scratch emit.
pub(super) fn splice_body_story_page_delta(page: &mut BuiltPage, delta: &BodyStoryPageDelta) {
    let new_base = page.list.paths.len() as i64;
    for path in &delta.paths {
        page.list.paths.push_anon(path.clone());
    }
    for cmd in &delta.commands {
        let mut c = cmd.clone();
        rebase_path_ids(&mut c, new_base);
        page.list.commands.push(c);
    }
    page.story_layout.extend(delta.story_layout.iter().cloned());
    page.footnotes.extend(delta.footnotes.iter().cloned());
}

/// Perf-MasterText — adds `offset` to every PathId field on a
/// DisplayCommand. Used (1) at capture-time with `offset = -base`
/// to rebase to relative ids, and (2) at replay-time with
/// `offset = new_base` to rebase the cached relative ids to the
/// active path-buffer position. Variants without a path_id field
/// are no-ops.
pub(super) fn rebase_path_ids(cmd: &mut paged_compose::DisplayCommand, offset: i64) {
    use paged_compose::DisplayCommand::*;
    let add = |pid: &mut paged_compose::PathId| {
        let v = pid.0 as i64 + offset;
        pid.0 = v as u32;
    };
    match cmd {
        FillPath { path_id, .. } => add(path_id),
        FillPathBlend { path_id, .. } => add(path_id),
        StrokePath { path_id, .. } => add(path_id),
        DropShadow { path_id, .. } => add(path_id),
        PathShadow { path_id, .. } => add(path_id),
        PushClip { path_id, .. } => add(path_id),
        InnerShadow { path_id, .. } => add(path_id),
        OuterGlow { path_id, .. } => add(path_id),
        InnerGlow { path_id, .. } => add(path_id),
        BevelEmboss { path_id, .. } => add(path_id),
        Satin { path_id, .. } => add(path_id),
        Feather { path_id, .. } => add(path_id),
        DirectionalFeather { path_id, .. } => add(path_id),
        GradientFeather { path_id, .. } => add(path_id),
        FillPathOverprint { path_id, .. } => add(path_id),
        StrokePathOverprint { path_id, .. } => add(path_id),
        // Variants without a path_id field — no-op.
        Image { .. }
        | PopClip(_)
        | BeginBlendGroup { .. }
        | EndBlendGroup(_)
        | PushLayer { .. }
        | PopLayer(_) => {}
    }
}
