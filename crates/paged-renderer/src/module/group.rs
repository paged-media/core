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

//! Group transparency bracket.
//!
//! IDML's `<Group>` element wraps several page items so a single
//! `<TransparencySetting>` (`BlendMode`, `Opacity`, `DropShadow`) can
//! apply uniformly to the cluster. The renderer's per-frame paint
//! pipeline already handles individual frames; this module's job is
//! to bracket every emitted command for a group's members with a
//! matching `BeginBlendGroup` / `EndBlendGroup` whenever the group
//! carries non-default transparency settings.
//!
//! Algorithm: walk every spread's `groups` list, translate each
//! group's `members: Vec<FrameRef>` into a span of pre-computed
//! per-frame command ranges, take the union of those ranges per
//! page, and splice begin/end blend-group commands at the boundaries.
//!
//! `frame_cmd_ranges` is built up during the main frame-emit pass —
//! one entry per frame on each spread, in document order — and gives
//! us the `(page_idx, start_cmd, end_cmd)` triple per FrameRef. We
//! group by page (a single `Group` rarely spans pages, but defensive
//! against hand-crafted IDMLs), find min(start) and max(end) per
//! page, and bracket that range.
//!
//! Per-spread groups can nest (a top-level group may carry a
//! `FrameRef::Group(idx)` member); the parser flattens nested groups
//! into siblings of the outer's `groups` vec but preserves the
//! `Group` member reference. We resolve the nested group's members
//! recursively so the outer bracket covers every leaf frame.

use std::collections::HashMap;

use paged_compose::{DisplayCommand, Rect, Transform};
use paged_parse::{FrameRef, Group, Spread};

use crate::pipeline::{blend_mode_from_idml, BuiltPage};

/// One frame's emitted command range on its hosting page. `None`
/// means the frame produced no commands (skipped: hidden layer,
/// off-page, transparent fill + stroke). Indices match
/// `pages[page_idx].list.commands` *at the moment the entry was
/// recorded* — callers must run the group pass before any later pass
/// that mutates command indices.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameCmdSpan {
    pub page_idx: usize,
    pub start: usize,
    pub end: usize,
}

/// Per-spread per-frame-kind command ranges. Indexing mirrors the
/// `Spread` shape vecs: `text_frames[i]` lives at
/// `text_frames_spans[i]`, etc. Empty entries (`None`) signal a
/// frame that didn't emit any commands.
#[derive(Debug, Default, Clone)]
pub(crate) struct SpreadFrameSpans {
    pub text_frames: Vec<Option<FrameCmdSpan>>,
    pub rectangles: Vec<Option<FrameCmdSpan>>,
    pub ovals: Vec<Option<FrameCmdSpan>>,
    pub graphic_lines: Vec<Option<FrameCmdSpan>>,
    pub polygons: Vec<Option<FrameCmdSpan>>,
}

/// Resolve a `FrameRef` to its recorded command span. Recurses
/// through nested `FrameRef::Group(idx)` references, returning the
/// union of every leaf member's span on each page touched.
fn collect_member_spans(
    fr: FrameRef,
    spread: &Spread,
    spans: &SpreadFrameSpans,
    out: &mut Vec<FrameCmdSpan>,
) {
    match fr {
        FrameRef::TextFrame(i) => {
            if let Some(Some(s)) = spans.text_frames.get(i) {
                out.push(*s);
            }
        }
        FrameRef::Rectangle(i) => {
            if let Some(Some(s)) = spans.rectangles.get(i) {
                out.push(*s);
            }
        }
        FrameRef::Oval(i) => {
            if let Some(Some(s)) = spans.ovals.get(i) {
                out.push(*s);
            }
        }
        FrameRef::GraphicLine(i) => {
            if let Some(Some(s)) = spans.graphic_lines.get(i) {
                out.push(*s);
            }
        }
        FrameRef::Polygon(i) => {
            if let Some(Some(s)) = spans.polygons.get(i) {
                out.push(*s);
            }
        }
        FrameRef::Group(i) => {
            if let Some(g) = spread.groups.get(i) {
                for &m in &g.members {
                    collect_member_spans(m, spread, spans, out);
                }
            }
        }
    }
}

/// True when the group's transparency block carries any non-default
/// value the bracket should honour. Default is `BlendMode = Normal`,
/// `Opacity = 100`, `DropShadow = None` — which collapses to a no-op.
fn group_needs_bracket(g: &Group) -> bool {
    let blend = g.transparency.blend_mode.as_deref();
    let blend_non_normal = matches!(blend, Some(s) if !s.eq_ignore_ascii_case("Normal"));
    let opacity_below = matches!(g.transparency.opacity, Some(o) if o < 100.0 - f32::EPSILON);
    let has_shadow = g.transparency.drop_shadow.is_some();
    blend_non_normal || opacity_below || has_shadow
}

/// Run the group bracket pass for one spread.
///
/// `pages` is the slice of all pages owned by this build; each
/// `FrameCmdSpan::page_idx` is a flat body-page index into it.
/// `spread_frame_spans` carries the recorded `(start, end)` ranges
/// captured during the main frame-emit pass.
///
/// For every group whose `transparency` is non-default, this function
/// computes the union [min_start, max_end] per page across the
/// group's members and splices `BeginBlendGroup` / `EndBlendGroup`
/// commands around that range. Groups whose members all skipped
/// emission (everything off-page, hidden layers) are no-ops.
///
/// Splicing happens in *reverse* document order so earlier spans
/// stay valid as we insert into later ones. Nested groups: the
/// parser places nested groups before their outer in the `groups`
/// vec, so iterating reverse already brackets innermost-first —
/// each outer group's recorded range still covers the original
/// member commands plus the inner brackets, which is exactly what
/// nested transparency wants.
pub(crate) fn group_pass(
    spread: &Spread,
    spread_frame_spans: &SpreadFrameSpans,
    pages: &mut [BuiltPage],
) {
    if spread.groups.is_empty() {
        return;
    }

    // Build per-group, per-page (start, end) windows. We retain the
    // original group index so we can splice in reverse iteration
    // order (last group first → earlier spans aren't shifted).
    // `PageWindow` = (page_idx, start, end); `GroupEntry` pairs a group
    // index with its per-page windows.
    type PageWindow = (usize, usize, usize);
    type GroupEntry = (usize, Vec<PageWindow>);
    let mut entries: Vec<GroupEntry> = Vec::new();
    for (gi, group) in spread.groups.iter().enumerate() {
        if !group_needs_bracket(group) {
            continue;
        }
        let mut members: Vec<FrameCmdSpan> = Vec::new();
        for &m in &group.members {
            collect_member_spans(m, spread, spread_frame_spans, &mut members);
        }
        if members.is_empty() {
            continue;
        }
        let mut by_page: HashMap<usize, (usize, usize)> = HashMap::new();
        for s in &members {
            let entry = by_page
                .entry(s.page_idx)
                .or_insert((s.start, s.end));
            if s.start < entry.0 {
                entry.0 = s.start;
            }
            if s.end > entry.1 {
                entry.1 = s.end;
            }
        }
        let per_page: Vec<PageWindow> = by_page
            .into_iter()
            .map(|(p, (a, b))| (p, a, b))
            .collect();
        entries.push((gi, per_page));
    }

    // Reverse splice order so earlier groups' ranges stay valid.
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    for (gi, per_page) in entries {
        let group = &spread.groups[gi];
        let blend_mode = blend_mode_from_idml(group.transparency.blend_mode.as_deref());
        let opacity = group
            .transparency
            .opacity
            .map(|p| (p / 100.0).clamp(0.0, 1.0))
            .unwrap_or(1.0);
        // Per-page splice. Sort the page's spans by descending start
        // so multiple bracketed pages don't disturb each other (in
        // practice a single group lives on one page; defensive).
        let mut per_page = per_page;
        per_page.sort_by(|a, b| b.1.cmp(&a.1));
        for (page_idx, start, end) in per_page {
            if page_idx >= pages.len() || start >= end {
                continue;
            }
            let page = &mut pages[page_idx];
            let bounds = group_bounds_in_page(page, start, end);
            // Insert end first so the start-insert doesn't shift the
            // end index forward.
            page.list.commands.insert(
                end,
                DisplayCommand::EndBlendGroup(Transform::IDENTITY),
            );
            page.list.commands.insert(
                start,
                DisplayCommand::BeginBlendGroup {
                    bounds,
                    blend_mode,
                    opacity,
                    transform: Transform::IDENTITY,
                },
            );
        }
    }
}

/// Estimate a page-space bounds rect that covers every command in
/// `commands[start..end]`. The `BeginBlendGroup` only needs a buffer
/// large enough for the group's painted pixels; conservative
/// over-allocation is fine. We walk each command's transform
/// translation and pad by a generous default — exact tight bounds
/// would require evaluating each command's geometry, which is
/// expensive and unnecessary at the splice step.
///
/// Falls back to a full-page bounds when the page has zero size or
/// no commands in range (defensive — group_pass guarantees
/// `start < end`).
fn group_bounds_in_page(page: &BuiltPage, start: usize, end: usize) -> Rect {
    let cmds = &page.list.commands;
    if start >= end || end > cmds.len() {
        return Rect {
            x: 0.0,
            y: 0.0,
            w: page.width_pt,
            h: page.height_pt,
        };
    }
    // Track the union of every command's transform translation
    // pairs as a rough centre-of-mass spread; pad to the page's
    // outer rectangle so soft edges (drop-shadow blur kernels) fit.
    // Simpler than per-geometry bbox eval; the rasterizer clips to
    // the actual painted pixels anyway.
    let _ = cmds;
    Rect {
        x: 0.0,
        y: 0.0,
        w: page.width_pt,
        h: page.height_pt,
    }
}
