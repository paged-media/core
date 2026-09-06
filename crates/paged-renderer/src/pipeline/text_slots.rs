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

//! Put a story's glyphs back at their text frame's z-slot.
//!
//! The page walk paints page items in z-order, but a story's glyphs are
//! emitted afterwards — the emitter needs the whole frame chain, which
//! spans pages and spreads, so it cannot run inside the walk. The
//! glyphs therefore land at the END of every page's command list and
//! paint on top of every shape, whatever the text frame's z position.
//!
//! InDesign does not do that: a panel above a text frame hides its
//! text. Our own canvas hit-tester does not do it either — it sorts by
//! `frames_in_order`, so selection and paint disagree today, and the
//! annual's page 59 shows a headline InDesign hides.
//!
//! This is the reconciliation. The walk records, per text frame per
//! page, the command index its glyphs belong before ([`TextSlot`], in
//! `module::group`); this pass rebuilds each page's command vector once,
//! moving every recorded block into its slot and leaving everything
//! else in its original relative order.
//!
//! Why a rebuild rather than N splices: a splice shifts every later
//! index on the page, so N independent moves would each invalidate the
//! others' anchors. One pass over the list is O(n) and needs no
//! bookkeeping between blocks.
//!
//! Safe to move because a `DisplayCommand` addresses nothing by
//! position: paths, gradients, images and inks are pool ids, and the
//! bracket pairs (`PushClip`/`PopClip`, `Begin`/`EndBlendGroup`, the
//! three soft-mask markers) are stack-structured and resolved by a
//! sequential scan. The one absolute-index side channel is
//! `GlyphRunEntry::command_index`, which this pass remaps.

use paged_compose::DisplayList;

/// One contiguous run of commands to relocate, and where it goes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextBlock {
    /// Half-open command range in the page's CURRENT list.
    pub start: usize,
    pub end: usize,
    /// Index this block is spliced in before.
    pub slot: usize,
    /// Walk order of the owning frame; breaks ties on a shared slot.
    pub z_seq: u32,
}

/// Rebuild `list.commands` so every block sits at its slot.
///
/// Blocks must be disjoint and each must start at or after its own slot
/// (a block already in front of its anchor has nowhere to move). Blocks
/// sharing a slot keep `z_seq` order. Commands no block covers keep
/// their relative order, so anything with no slot — text on a path,
/// anchored images, footnote pools — stays exactly where it was.
pub(crate) fn relocate_text_blocks(list: &mut DisplayList, blocks: &mut [TextBlock]) {
    if blocks.is_empty() {
        return;
    }
    let n = list.commands.len();
    blocks.sort_by_key(|b| (b.slot, b.z_seq, b.start));
    debug_assert!(
        blocks.iter().all(|b| b.end <= n && b.start <= b.end),
        "a text block runs past the end of the page's command list"
    );

    let mut covered = vec![false; n];
    for b in blocks.iter() {
        for c in covered.iter_mut().take(b.end.min(n)).skip(b.start) {
            debug_assert!(!*c, "text blocks overlap");
            *c = true;
        }
    }

    // `remap[i]` = where command `i` ended up. u32::MAX marks a command
    // that could not be placed, which the debug assert below rejects.
    let mut remap = vec![u32::MAX; n];
    let mut out: Vec<paged_compose::DisplayCommand> = Vec::with_capacity(n);
    let mut next = 0usize;
    for i in 0..=n {
        while next < blocks.len() && blocks[next].slot <= i {
            let b = blocks[next];
            for (offset, cmd) in list.commands[b.start..b.end.min(n)].iter().enumerate() {
                remap[b.start + offset] = out.len() as u32;
                out.push(cmd.clone());
            }
            next += 1;
        }
        if i < n && !covered[i] {
            remap[i] = out.len() as u32;
            out.push(list.commands[i].clone());
        }
    }
    // A block whose slot sits past the end of the list still has to land.
    while next < blocks.len() {
        let b = blocks[next];
        for (offset, cmd) in list.commands[b.start..b.end.min(n)].iter().enumerate() {
            remap[b.start + offset] = out.len() as u32;
            out.push(cmd.clone());
        }
        next += 1;
    }
    debug_assert_eq!(out.len(), n, "relocation changed the command count");
    debug_assert!(
        remap.iter().all(|&r| r != u32::MAX),
        "relocation dropped a command"
    );

    list.commands = out;
    if let Some(table) = list.glyph_runs.as_mut() {
        for entry in table.entries.iter_mut() {
            let old = entry.command_index as usize;
            if let Some(&new) = remap.get(old) {
                if new != u32::MAX {
                    entry.command_index = new;
                }
            }
        }
    }
}

/// Cut a story's per-page command block into one piece per chain frame.
///
/// The emitter records each chain frame's `(start, end)` as it goes.
/// The STARTS are reliable — a frame's first command lands there — but
/// the ENDS are not: a paragraph rule below, a table and the anchored
/// objects of the last paragraph are all emitted after the range was
/// last extended. So a frame's piece runs from its own start to the
/// NEXT frame's start, and the last piece to the end of the block;
/// whatever trails belongs to the frame it trails.
///
/// Returns `(chain_idx, absolute start)` ascending. When no frame on
/// the page recorded a range (a table-only frame never touches them),
/// the whole block is credited to the lowest chain index on that page.
pub(crate) fn segment_story_block(
    ranges: &[Option<(usize, usize)>],
    chain_pages: &[usize],
    page_idx: usize,
    block_start: usize,
    block_end: usize,
) -> Vec<(usize, usize)> {
    let mut segs: Vec<(usize, usize)> = ranges
        .iter()
        .enumerate()
        .filter(|(i, _)| chain_pages.get(*i).copied() == Some(page_idx))
        .filter_map(|(i, r)| r.map(|(start, _)| (i, start)))
        .filter(|(_, start)| *start >= block_start && *start < block_end)
        .collect();
    segs.sort_by_key(|(i, start)| (*start, *i));
    segs.dedup_by_key(|(_, start)| *start);
    if segs.is_empty() {
        let first = chain_pages.iter().position(|&p| p == page_idx).unwrap_or(0);
        return vec![(first, block_start)];
    }
    // Anything before the first recorded start (a leading table, a rule
    // above the first line) belongs to that same frame.
    segs[0].1 = block_start;
    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_compose::{Color, DisplayCommand, Paint, PathId, Transform};

    /// A `FillPath` whose path id is `n`, so a rebuilt list reads back
    /// as the sequence of ids it was assembled from.
    fn cmd(n: u32) -> DisplayCommand {
        DisplayCommand::FillPath {
            path_id: PathId(n),
            paint: Paint::Solid(Color::BLACK),
            transform: Transform::IDENTITY,
        }
    }

    fn ids(list: &DisplayList) -> Vec<u32> {
        list.commands
            .iter()
            .map(|c| match c {
                DisplayCommand::FillPath { path_id, .. } => path_id.0,
                _ => u32::MAX,
            })
            .collect()
    }

    fn list_of(n: u32) -> DisplayList {
        let mut list = DisplayList::new();
        for i in 0..n {
            list.commands.push(cmd(i));
        }
        list
    }

    #[test]
    fn a_story_block_is_cut_at_each_frames_first_command() {
        // Two chain frames on page 3; the block runs 10..40 and the
        // second frame's first command is at 25. Anything before the
        // first recorded start belongs to frame 0.
        let ranges = [Some((12usize, 20usize)), Some((25, 30)), None];
        let chain_pages = [3usize, 3, 4];
        assert_eq!(
            segment_story_block(&ranges, &chain_pages, 3, 10, 40),
            vec![(0, 10), (1, 25)]
        );
        // A page whose frames recorded nothing (a table-only frame)
        // still credits the whole block to its first chain frame.
        assert_eq!(
            segment_story_block(&[None, None], &[7, 7], 7, 5, 9),
            vec![(0, 5)]
        );
    }

    #[test]
    fn a_block_moves_to_its_slot_and_the_rest_keeps_its_order() {
        // 0 1 | 2 3 (panel) | 4 5 (the story's glyphs) — the glyphs
        // belong at slot 2, i.e. in front of the panel.
        let mut list = list_of(6);
        let mut blocks = [TextBlock {
            start: 4,
            end: 6,
            slot: 2,
            z_seq: 1,
        }];
        relocate_text_blocks(&mut list, &mut blocks);
        assert_eq!(ids(&list), vec![0, 1, 4, 5, 2, 3]);
    }

    #[test]
    fn blocks_sharing_a_slot_keep_walk_order() {
        let mut list = list_of(8);
        let mut blocks = [
            TextBlock {
                start: 6,
                end: 8,
                slot: 1,
                z_seq: 9,
            },
            TextBlock {
                start: 4,
                end: 6,
                slot: 1,
                z_seq: 2,
            },
        ];
        relocate_text_blocks(&mut list, &mut blocks);
        // z_seq 2 first, then z_seq 9, both before the old index 1.
        assert_eq!(ids(&list), vec![0, 4, 5, 6, 7, 1, 2, 3]);
    }

    #[test]
    fn a_slot_at_the_end_is_a_no_op_and_nothing_is_lost() {
        let mut list = list_of(5);
        let mut blocks = [TextBlock {
            start: 3,
            end: 5,
            slot: 3,
            z_seq: 1,
        }];
        relocate_text_blocks(&mut list, &mut blocks);
        assert_eq!(ids(&list), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn glyph_run_indices_follow_their_commands() {
        let mut list = list_of(6);
        let mut runs = paged_compose::GlyphRunTable::default();
        for i in [0u32, 4, 5] {
            runs.push(paged_compose::GlyphRunEntry {
                command_index: i,
                font_id: 0,
                glyph_id: i,
                font_size: 10.0,
                transform: Transform::IDENTITY,
                paint: Paint::Solid(Color::BLACK),
                is_stroke: false,
                unicode: None,
            });
        }
        list.glyph_runs = Some(runs);
        let mut blocks = [TextBlock {
            start: 4,
            end: 6,
            slot: 2,
            z_seq: 1,
        }];
        relocate_text_blocks(&mut list, &mut blocks);
        let table = list.glyph_runs.as_ref().expect("table");
        let moved: Vec<u32> = table.entries.iter().map(|e| e.command_index).collect();
        // command 0 stayed; commands 4,5 became 2,3.
        assert_eq!(moved, vec![0, 2, 3]);
        // and each entry still addresses the command it was made for
        for e in &table.entries {
            match &list.commands[e.command_index as usize] {
                DisplayCommand::FillPath { path_id, .. } => {
                    assert_eq!(path_id.0, e.glyph_id, "entry lost its command");
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
