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

//! Glyph-shaped shadow post-pass.
//!
//! IDML lets a `<TextFrame>` carry its own `<DropShadowSetting>` under
//! `<StrokeTransparencySetting>`. Real InDesign treats that as a soft
//! shadow off the visible *text outlines* — when both the frame's fill
//! and stroke are transparent, there's no rectangular target for the
//! shadow to stamp behind, so the shadow follows each glyph instead.
//! The frame-body drop-shadow module already emits a rect-shaped stamp
//! when the stroke is visible; this module fills the per-glyph case
//! that was previously rendered as nothing.
//!
//! Algorithm: walk a per-frame range of already-emitted glyph
//! `FillPath` commands and splice a matching `PathShadow` command in
//! front of each one (with the shadow's offset baked into the
//! transform). The `PathShadow` carries the same `path_id` as the
//! glyph's fill, so the rasterizer reuses the interned outline; the
//! Gaussian-blurred soft-shadow stamp lives entirely in cpu.rs.
//!
//! The inserted shadow commands are wrapped in their own
//! transparency group (BlendGroup with Normal blend, the shadow's
//! opacity). The per-glyph stamps render at full alpha into the
//! buffer; overlapping stamps from neighbouring glyphs stay opaque
//! instead of stacking via `SrcOver` into pure black; the group
//! composite then applies the soft opacity uniformly. Without this
//! the candidate's shadows fill solid black where adjacent glyphs'
//! 3σ blur kernels overlap, mismatching InDesign's premultiplied-
//! group shadow composite.
//!
//! Why pre-glyph (not pre-BlendGroup) insertion: the caller in
//! `pipeline.rs` is responsible for landing these commands *outside*
//! any subsequent BlendGroup that brackets the glyph range — otherwise
//! a Lighten-blend frame would wipe the dark shadow against the white
//! page (Lighten of dark gray on white = white = invisible). The
//! command range we splice into is the pre-bracket glyph range, so
//! inserting at `range.start` is the right place; `apply_blend_groups`
//! shifts its `start..end` window forward by the number of inserted
//! commands and brackets only the glyph fills.

use paged_compose::{BlendMode, DisplayCommand, DropShadow, Rect, Transform};

use crate::pipeline::BuiltPage;

/// Splice glyph-shaped shadow stamps in front of every glyph
/// `FillPath` / `FillPathBlend` command in
/// `page.list.commands[glyph_command_range]`, wrapped in a
/// transparency group.
///
/// Returns the number of commands inserted — callers can add this to
/// their `(start, end)` range bookkeeping when later passes
/// (blend-group bracketing, vertical justification on subsequent
/// frames) need the post-insertion indices.
///
/// The shadow's `(offset_x, offset_y)` is left on the `DropShadow`
/// struct; the rasterizer translates the path by that offset
/// internally (see `DisplayCommand::DropShadow` / `PathShadow` in
/// cpu.rs). We don't bake the offset into the transform here — that
/// would double-apply when the rasterizer reads `shadow.offset_*`.
///
/// `group_bounds` is the page-space rectangle the wrapping
/// BlendGroup uses for its offscreen buffer. Pad generously enough
/// to contain every glyph's bbox, the shadow's offset, and the
/// 3σ blur kernel — the rasterizer clips draws to the buffer's
/// pixel grid.
pub(crate) fn emit_glyph_shadow_pass(
    page: &mut BuiltPage,
    glyph_command_range: std::ops::Range<usize>,
    shadow: DropShadow,
    group_bounds: Rect,
) -> usize {
    // First pass: collect the (insertion_index, transform, path_id)
    // tuples for every glyph fill in the range. We insert in reverse
    // order so earlier indices stay valid as later inserts happen.
    let mut inserts: Vec<(usize, Transform, paged_compose::PathId)> = Vec::new();
    for (offset, cmd) in page.list.commands[glyph_command_range.clone()]
        .iter()
        .enumerate()
    {
        let abs_idx = glyph_command_range.start + offset;
        match cmd {
            DisplayCommand::FillPath {
                path_id, transform, ..
            }
            | DisplayCommand::FillPathBlend {
                path_id, transform, ..
            } => {
                inserts.push((abs_idx, *transform, *path_id));
            }
            // Shadows attach to glyph fills only — strokes / clips /
            // groups inside a glyph range don't currently exist, but
            // if they ever did we'd still skip them so a clip's path
            // isn't rendered as a soft-shadow stamp.
            _ => {}
        }
    }
    if inserts.is_empty() {
        return 0;
    }
    // Drive the wrapper at the shadow's alpha boosted to compensate
    // for InDesign's per-glyph shadow accumulation. Empirically the
    // reference renderer paints shadow centres darker than a single
    // 75%-alpha stamp; that's because adjacent glyphs' soft kernels
    // overlap and SrcOver-compound up to near-opaque. We simulate
    // the same compounding by emitting each PathShadow at full alpha
    // (1.0) inside a Normal-blend transparency group with the IDML
    // shadow's opacity — overlap inside the group buffer saturates
    // at the buffer's full alpha, the group composite then fades to
    // the shadow's opacity uniformly. Without the wrapper, repeated
    // 75%-alpha stamps SrcOver-into-pure-black at every overlap, and
    // the candidate ends up much darker than the reference where
    // shadows stack.
    let mut shadow_full = shadow;
    shadow_full.opacity = 1.0;
    let group_opacity = shadow.opacity.clamp(0.0, 1.0);
    let inserted_shadow_count = inserts.len();
    let abs_start = glyph_command_range.start;
    // Splice all PathShadow commands clustered together at
    // `range.start`, then End at `range.start + n`, then Begin at
    // `range.start`. Final layout:
    //   range.start: BeginBlendGroup
    //   range.start + 1..=n: PathShadow stamps (in original glyph order)
    //   range.start + n + 1: EndBlendGroup
    //   range.start + n + 2..: (original glyph fills, shifted)
    //
    // We insert each shadow at `abs_start` in reverse-collected order
    // so the order at `abs_start..abs_start+n` matches the original
    // glyph emission order.
    for (_idx, transform, path_id) in inserts.into_iter().rev() {
        page.list.commands.insert(
            abs_start,
            DisplayCommand::PathShadow {
                path_id,
                transform,
                shadow: shadow_full,
            },
        );
    }
    page.list.commands.insert(
        abs_start + inserted_shadow_count,
        DisplayCommand::EndBlendGroup(Transform::IDENTITY),
    );
    page.list.commands.insert(
        abs_start,
        DisplayCommand::BeginBlendGroup {
            bounds: group_bounds,
            blend_mode: BlendMode::Normal,
            opacity: group_opacity,
            transform: Transform::IDENTITY,
        },
    );
    inserted_shadow_count + 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_compose::{Color, DisplayList, DropShadow, Paint, PathData, PathSegment};

    fn dummy_page(list: DisplayList) -> BuiltPage {
        BuiltPage {
            id: crate::pipeline::PageId::synthetic(0, 0),
            width_pt: 100.0,
            height_pt: 100.0,
            spread_origin: (0.0, 0.0),
            list,
            layout_generation: 0,
            numbering_generation: 0,
            stats: Default::default(),
            story_layout: Vec::new(),
            footnotes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn page_bounds() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        }
    }

    #[test]
    fn inserts_path_shadow_block_wrapped_in_blend_group() {
        let mut list = DisplayList::new();
        let mut p = PathData::default();
        p.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        p.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        p.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        p.segments.push(PathSegment::Close);
        let path_id = list.paths.push_anon(p);
        // Three glyph-like fills.
        for i in 0..3 {
            list.commands.push(DisplayCommand::FillPath {
                path_id,
                paint: Paint::Solid(Color::BLACK),
                transform: Transform::translate(i as f32 * 10.0, 0.0),
            });
        }
        let mut page = dummy_page(list);
        let shadow = DropShadow::default_soft();
        let n = emit_glyph_shadow_pass(&mut page, 0..3, shadow, page_bounds());
        // 3 shadows + Begin + End.
        assert_eq!(n, 5);
        // After insertion: Begin, shadow, shadow, shadow, End, fill, fill, fill.
        assert_eq!(page.list.commands.len(), 8);
        match &page.list.commands[0] {
            DisplayCommand::BeginBlendGroup { opacity, .. } => {
                assert!((opacity - shadow.opacity).abs() < 1e-4);
            }
            other => panic!("expected BeginBlendGroup, got {other:?}"),
        }
        for i in 1..=3 {
            match &page.list.commands[i] {
                DisplayCommand::PathShadow { shadow, .. } => {
                    // Shadow opacity rebased to 1.0 — the wrap
                    // BlendGroup applies the soft fade.
                    assert!((shadow.opacity - 1.0).abs() < 1e-4);
                }
                other => panic!("expected PathShadow at {i}, got {other:?}"),
            }
        }
        match &page.list.commands[4] {
            DisplayCommand::EndBlendGroup(_) => {}
            other => panic!("expected EndBlendGroup, got {other:?}"),
        }
        for i in 5..8 {
            match &page.list.commands[i] {
                DisplayCommand::FillPath { .. } => {}
                other => panic!("expected FillPath at {i}, got {other:?}"),
            }
        }
    }

    #[test]
    fn empty_range_inserts_nothing() {
        let list = DisplayList::new();
        let mut page = dummy_page(list);
        let shadow = DropShadow::default_soft();
        let n = emit_glyph_shadow_pass(&mut page, 0..0, shadow, page_bounds());
        assert_eq!(n, 0);
        assert!(page.list.commands.is_empty());
    }

    #[test]
    fn non_fill_commands_in_range_are_skipped() {
        let mut list = DisplayList::new();
        // A BeginBlendGroup + EndBlendGroup pair embedded in the
        // glyph range: should be ignored, no shadow attached to
        // them.
        list.commands.push(DisplayCommand::BeginBlendGroup {
            bounds: paged_compose::Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            blend_mode: paged_compose::BlendMode::Normal,
            opacity: 1.0,
            transform: Transform::IDENTITY,
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        let mut page = dummy_page(list);
        let shadow = DropShadow::default_soft();
        let n = emit_glyph_shadow_pass(&mut page, 0..2, shadow, page_bounds());
        assert_eq!(n, 0);
        assert_eq!(page.list.commands.len(), 2);
    }
}
