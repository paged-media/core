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

//! Span and split columns (`SpanColumnType`): where a story's paragraphs
//! go when one of them spans several columns of a multi-column frame, or
//! splits its column into sub-columns.
//!
//! The rules, measured against InDesign 2025 with the `span-columns`
//! paged-gen fixture (2026-10-01):
//!
//! - **Span.** The text above a spanning paragraph, from the top of the
//!   band it starts in, is balanced over the SPANNED columns by line
//!   count (`ceil(lines / k)` per column; 6 lines over 2 → 3 / 3, over 3
//!   → 2 / 2 / 2, 4 lines over a span of 2 in a 3-column frame → 2 / 2
//!   with the third column empty). The span sits at the width of the
//!   spanned columns, below the deepest line above it, by
//!   `max(SpaceBefore, SpanColumnMinSpaceBefore)`; the text after it
//!   starts `max(SpaceAfter, SpanColumnMinSpaceAfter)` below its last
//!   line and fills the spanned columns in turn (NOT balanced). Columns
//!   outside a span (the third of a span of 2 in 3) are ordinary
//!   full-height columns that follow the spanned ones. A span with no
//!   room left in its frame moves to the next frame's top.
//! - **Split.** Consecutive split paragraphs form one block inside the
//!   column the text is in: `k` sub-columns of width
//!   `(column − 2 × outside − (k − 1) × inside) / k` (InDesign's
//!   defaults: inside 6, outside 0), balanced by line count (5 lines in
//!   2 → 3 / 2, 7 in 3 → 3 / 3 / 1; a paragraph may straddle two
//!   sub-columns). The block sits by the same min-space rules, and the
//!   next paragraph starts below its deepest sub-column. A block that
//!   does not fit fills its sub-columns to the bottom and continues,
//!   balanced again, in the next column or frame.
//!
//! The plan is made before the emit from measured line pitches, and is
//! handed to the emitter as an ordinary region chain (one region per
//! column, band, span or sub-column actually used, in reading order)
//! plus the region each paragraph must start in.

use std::collections::{HashMap, VecDeque};

/// InDesign's `[No paragraph style]` defaults (its own `Styles.xml`).
pub(super) const DEFAULT_INSIDE_GUTTER_PT: f32 = 6.0;
pub(super) const DEFAULT_OUTSIDE_GUTTER_PT: f32 = 0.0;

/// Float slack for the fit test (the emitter compares in 1/64 pt).
const EPS: f32 = 0.01;

/// How a paragraph sits, as declared (the frame decides what a span of
/// `All` columns means).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Single,
    /// Spans this many columns; `None` = all of them.
    Span(Option<u32>),
    Split {
        k: u32,
        inside: f32,
        outside: f32,
    },
}

/// One paragraph's planning inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ParaSpec {
    pub kind: Kind,
    /// `SpaceBefore` / `SpaceAfter` as the emitter applies them.
    pub space_before: f32,
    pub space_after: f32,
    /// `SpanColumnMinSpaceBefore` / `After`.
    pub min_before: f32,
    pub min_after: f32,
}

impl ParaSpec {
    pub(super) fn from_attrs(a: &paged_scene::ResolvedParagraphAttrs) -> Self {
        use paged_model::{SpanColumnType, SpanSplitColumnCount};
        let sc = &a.span_columns;
        let kind = match sc.column_type.unwrap_or_default() {
            SpanColumnType::SingleColumn => Kind::Single,
            SpanColumnType::SpanColumns => Kind::Span(match sc.count {
                Some(SpanSplitColumnCount::Count(n)) => Some(n),
                Some(SpanSplitColumnCount::All) | None => None,
            }),
            // InDesign turns a split of `All` into 2 (its export writes
            // `type="short"` 2 for such a range).
            SpanColumnType::SplitColumns => Kind::Split {
                k: match sc.count {
                    Some(SpanSplitColumnCount::Count(n)) => n,
                    Some(SpanSplitColumnCount::All) | None => 2,
                },
                inside: sc
                    .inside_gutter
                    .unwrap_or(DEFAULT_INSIDE_GUTTER_PT)
                    .max(0.0),
                outside: sc
                    .outside_gutter
                    .unwrap_or(DEFAULT_OUTSIDE_GUTTER_PT)
                    .max(0.0),
            },
        };
        ParaSpec {
            kind,
            space_before: a.space_before.unwrap_or(0.0),
            space_after: a.space_after.unwrap_or(0.0),
            min_before: sc.min_space_before.unwrap_or(0.0).max(0.0),
            min_after: sc.min_space_after.unwrap_or(0.0).max(0.0),
        }
    }

    fn before_block(&self) -> f32 {
        self.space_before.max(self.min_before)
    }

    fn after_block(&self) -> f32 {
        self.space_after.max(self.min_after)
    }
}

/// One frame of the authored chain, frame-relative (pt).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FrameSpec {
    /// The frame's height: a line fits while its baseline is not below it.
    pub height: f32,
    pub inset_top: f32,
    /// Column bands: x from the frame's inner left, and width.
    pub columns: Vec<(f32, f32)>,
    /// `FirstBaselineOffset="LeadingOffset"`: a column's first baseline
    /// sits one leading below its top.
    pub leading_offset: bool,
}

/// Measured line pitches of every paragraph at one width.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Measured {
    /// Per paragraph, each line's leading (baseline-to-baseline advance,
    /// paragraph spacing excluded).
    pub leads: Vec<Vec<f32>>,
    /// The story's first baseline below the inset top, for frames whose
    /// first baseline is not one leading down.
    pub first_offset: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Top {
    /// The frame's own top: its inset and first-baseline policy apply.
    Frame,
    /// Mid-frame, frame-relative: the first baseline sits one leading
    /// below.
    Anchor(f32),
}

/// One region of the plan. `x` / `width` are relative to the frame's
/// inner left edge, `bottom` to its top.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Region {
    pub frame: usize,
    pub x: f32,
    pub width: f32,
    pub top: Top,
    pub bottom: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Plan {
    pub regions: Vec<Region>,
    /// Paragraph index → the region its first line opens.
    pub starts: HashMap<u32, usize>,
}

#[derive(Debug, Clone)]
struct Slot {
    frame: usize,
    /// Column index in the frame; `None` for a span or a sub-column.
    col: Option<usize>,
    x: f32,
    width: f32,
    top: Top,
    bottom: f32,
    last: Option<f32>,
    region: Option<usize>,
    /// One of the band's spanned (group) columns.
    band: bool,
}

impl Slot {
    fn column(frame: usize, col: usize, (x, width): (f32, f32), top: Top, bottom: f32) -> Self {
        Slot {
            frame,
            col: Some(col),
            x,
            width,
            top,
            bottom,
            last: None,
            region: None,
            band: true,
        }
    }
}

/// Plan where every paragraph goes. `measure(width)` gives the line
/// pitches of every paragraph composed at `width`.
pub(super) fn plan(
    frames: &[FrameSpec],
    paras: &[ParaSpec],
    measure: &mut dyn FnMut(f32) -> std::rc::Rc<Measured>,
) -> Plan {
    let mut p = Planner {
        frames,
        paras,
        measure,
        out: Plan::default(),
        frame: 0,
        cur: None,
        pending: VecDeque::new(),
        band_deepest: None,
    };
    p.run();
    p.out
}

struct Planner<'a> {
    frames: &'a [FrameSpec],
    paras: &'a [ParaSpec],
    measure: &'a mut dyn FnMut(f32) -> std::rc::Rc<Measured>,
    out: Plan,
    frame: usize,
    cur: Option<Slot>,
    pending: VecDeque<Slot>,
    /// The deepest baseline in the current band's spanned columns.
    band_deepest: Option<f32>,
}

/// The line cursor: paragraph, line within it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct At {
    p: usize,
    i: usize,
}

impl<'a> Planner<'a> {
    fn run(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.open_frame(0);
        let mut at = At { p: 0, i: 0 };
        while at.p < self.paras.len() {
            let ok = match self.kind_here(at.p) {
                Kind::Single => self.single(&mut at),
                Kind::Span(k) => self.span(&mut at, k.unwrap_or(u32::MAX) as usize),
                Kind::Split { .. } => self.split(&mut at),
            };
            if !ok {
                return;
            }
        }
    }

    /// The paragraph's kind in the current frame: a span of fewer than
    /// two columns, or a split into fewer than two, is an ordinary
    /// paragraph.
    fn kind_here(&self, p: usize) -> Kind {
        let n = self.frames.get(self.frame).map_or(1, |f| f.columns.len());
        match self.paras[p].kind {
            Kind::Span(k) if n >= 2 && k.map_or(true, |k| k >= 2) => Kind::Span(k),
            Kind::Split { k, .. } if k >= 2 => self.paras[p].kind,
            _ => Kind::Single,
        }
    }

    fn open_frame(&mut self, f: usize) {
        self.frame = f;
        self.pending.clear();
        self.band_deepest = None;
        let spec = &self.frames[f];
        for (c, band) in spec.columns.iter().enumerate() {
            self.pending
                .push_back(Slot::column(f, c, *band, Top::Frame, spec.height));
        }
        self.cur = self.pending.pop_front();
    }

    /// Move on to the next slot (the next column, else the next frame's
    /// first). `false` when the chain is exhausted.
    fn next_slot(&mut self) -> bool {
        if let Some(s) = self.pending.pop_front() {
            self.cur = Some(s);
            return true;
        }
        self.next_frame()
    }

    fn next_frame(&mut self) -> bool {
        if self.frame + 1 >= self.frames.len() {
            self.cur = None;
            return false;
        }
        self.open_frame(self.frame + 1);
        true
    }

    fn leads(&mut self, width: f32, p: usize) -> Vec<f32> {
        let m = (self.measure)(width);
        m.leads.get(p).cloned().unwrap_or_default()
    }

    fn first_offset(&mut self, width: f32) -> f32 {
        (self.measure)(width).first_offset
    }

    /// The advance from the previous baseline to line `i` of `p`, when
    /// both share a slot.
    fn gap(&self, p: usize, i: usize) -> f32 {
        if i == 0 && p > 0 {
            self.paras[p - 1].space_after + self.paras[p].space_before
        } else {
            0.0
        }
    }

    /// Where line (`p`, `i`) with leading `lead` would sit in `slot`.
    fn baseline_in(&mut self, slot: &Slot, p: usize, i: usize, lead: f32) -> f32 {
        match (slot.last, slot.top) {
            (Some(y), _) => y + self.gap(p, i) + lead,
            (None, Top::Anchor(y)) => y + lead,
            (None, Top::Frame) => {
                let f = &self.frames[slot.frame];
                let inset = f.inset_top;
                let offset = if f.leading_offset {
                    lead
                } else {
                    self.first_offset(slot.width)
                };
                inset + offset
            }
        }
    }

    /// Record a line placed at `baseline` in `slot`.
    fn commit(&mut self, slot: &mut Slot, p: usize, i: usize, baseline: f32) {
        let region = match slot.region {
            Some(r) => r,
            None => {
                self.out.regions.push(Region {
                    frame: slot.frame,
                    x: slot.x,
                    width: slot.width,
                    top: slot.top,
                    bottom: slot.bottom,
                });
                let r = self.out.regions.len() - 1;
                slot.region = Some(r);
                r
            }
        };
        if i == 0 {
            self.out.starts.insert(p as u32, region);
        }
        slot.last = Some(baseline);
        if slot.band {
            self.band_deepest = Some(self.band_deepest.map_or(baseline, |d| d.max(baseline)));
        }
    }

    /// End a slot after its last line: lower its region's bottom to
    /// half-way to where `next_lead` would put the next line, so a
    /// paragraph running on overflows to the next region exactly there.
    fn cut(&mut self, slot: &mut Slot, next_lead: f32) {
        if let (Some(r), Some(y)) = (slot.region, slot.last) {
            let b = &mut self.out.regions[r].bottom;
            *b = b.min(y + next_lead.max(1.0) * 0.5);
            slot.bottom = *b;
        }
    }

    /// Place one line sequentially from the current slot on.
    fn place_seq(&mut self, p: usize, i: usize, width_of: impl Fn(&Slot) -> f32) -> bool {
        loop {
            let Some(mut slot) = self.cur.take() else {
                return false;
            };
            let w = width_of(&slot);
            let leads = self.leads(w, p);
            let lead = leads.get(i).copied().unwrap_or(0.0);
            let base = self.baseline_in(&slot, p, i, lead);
            if base <= slot.bottom + EPS {
                self.commit(&mut slot, p, i, base);
                self.cur = Some(slot);
                return true;
            }
            if !self.next_slot() {
                return false;
            }
        }
    }

    fn line_count(&mut self, width: f32, p: usize) -> usize {
        self.leads(width, p).len()
    }

    /// A run of ordinary paragraphs from `at`.
    fn single(&mut self, at: &mut At) -> bool {
        // Text above a span is balanced over the spanned columns when
        // it starts at the top of a band and all of it fits.
        if at.i == 0 && self.at_band_start() {
            let mut s = at.p;
            while s < self.paras.len() && self.kind_here(s) == Kind::Single {
                s += 1;
            }
            if s < self.paras.len() {
                if let Kind::Span(k) = self.kind_here(s) {
                    if self.balanced_run(at.p, s, k.unwrap_or(u32::MAX) as usize) {
                        *at = At { p: s, i: 0 };
                        return true;
                    }
                }
            }
        }
        let n = {
            let Some(slot) = self.cur.as_ref() else {
                return false;
            };
            self.line_count(slot.width, at.p)
        };
        if at.i >= n {
            *at = At { p: at.p + 1, i: 0 };
            return true;
        }
        if !self.place_seq(at.p, at.i, |s| s.width) {
            return false;
        }
        at.i += 1;
        true
    }

    fn at_band_start(&self) -> bool {
        self.cur
            .as_ref()
            .is_some_and(|s| s.band && s.last.is_none() && self.band_deepest.is_none())
    }

    /// Balance paragraphs `[from, to)` over the band's first `k` columns
    /// (the current slot and the pending ones after it). `false`, with
    /// nothing placed, when they do not all fit.
    fn balanced_run(&mut self, from: usize, to: usize, k: usize) -> bool {
        let Some(cur) = self.cur.clone() else {
            return false;
        };
        let mut slots = vec![cur];
        slots.extend(
            self.pending
                .iter()
                .filter(|s| s.band && s.frame == self.frame)
                .cloned(),
        );
        slots.truncate(k.max(1));
        let width = slots[0].width;
        let lines: Vec<(usize, usize)> = (from..to)
            .flat_map(|p| {
                let n = self.line_count(width, p);
                (0..n).map(move |i| (p, i))
            })
            .collect();
        if lines.is_empty() {
            return false;
        }
        let per = lines.len().div_ceil(slots.len());
        if !self.fits_in(&slots, &lines, per) {
            return false;
        }
        let mut chunks = lines.chunks(per.max(1));
        for slot in slots.iter_mut() {
            let Some(chunk) = chunks.next() else {
                break;
            };
            for &(p, i) in chunk {
                let lead = self.leads(width, p)[i];
                let base = self.baseline_in(slot, p, i, lead);
                self.commit(slot, p, i, base);
            }
            let &(lp, li) = chunk.last().expect("chunks are never empty");
            let next = self
                .leads(width, lp)
                .get(li + 1)
                .copied()
                .unwrap_or(self.leads(width, lp)[li]);
            self.cut(slot, next);
        }
        // The band's spanned columns are spent; what is left of the frame
        // is its other columns, after the span.
        let spent: Vec<Option<usize>> = slots.iter().map(|s| s.col).collect();
        self.pending
            .retain(|s| !(s.band && s.frame == self.frame && spent.contains(&s.col)));
        self.cur = None;
        true
    }

    /// Whether `lines`, `per` to a slot, all fit.
    fn fits_in(&mut self, slots: &[Slot], lines: &[(usize, usize)], per: usize) -> bool {
        let mut trial = slots.to_vec();
        let width = trial[0].width;
        for (slot, chunk) in trial.iter_mut().zip(lines.chunks(per.max(1))) {
            for &(p, i) in chunk {
                let lead = self.leads(width, p)[i];
                let base = self.baseline_in(slot, p, i, lead);
                if base > slot.bottom + EPS {
                    return false;
                }
                slot.last = Some(base);
            }
        }
        lines.len() <= per * trial.len()
    }

    /// A spanning paragraph from `at`, over `k` columns.
    fn span(&mut self, at: &mut At, k: usize) -> bool {
        loop {
            let f = self.frame;
            let cols = self.frames[f].columns.clone();
            let k = k.clamp(1, cols.len());
            let (x0, _) = cols[0];
            let (xl, wl) = cols[k - 1];
            let width = xl + wl - x0;
            let n = self.line_count(width, at.p);
            let spec = self.paras[at.p];
            // Below the deepest line of the band, or at the band's top.
            let top = match (
                self.band_deepest,
                self.cur.as_ref().map(|s| (s.top, s.band)),
            ) {
                (Some(d), _) => Top::Anchor(d + spec.before_block()),
                (None, Some((Top::Anchor(y), true))) if at.i == 0 => {
                    Top::Anchor(y + spec.before_block())
                }
                (None, Some((t, true))) => t,
                // Not in a band (a column outside the span): next frame.
                _ => {
                    if !self.next_frame() {
                        return false;
                    }
                    continue;
                }
            };
            let mut slot = Slot {
                frame: f,
                col: None,
                x: x0,
                width,
                top,
                bottom: self.frames[f].height,
                last: None,
                region: None,
                band: false,
            };
            while at.i < n {
                let lead = self.leads(width, at.p)[at.i];
                let base = self.baseline_in(&slot, at.p, at.i, lead);
                if base > slot.bottom + EPS {
                    break;
                }
                self.commit(&mut slot, at.p, at.i, base);
                at.i += 1;
            }
            if at.i < n {
                // No room (left) in this frame: the rest opens the next.
                if !self.next_frame() {
                    return false;
                }
                continue;
            }
            // The text after the span fills the spanned columns from
            // below it; the frame's other columns follow.
            let below = slot
                .last
                .map(|y| y + spec.after_block())
                .unwrap_or(match top {
                    Top::Anchor(y) => y,
                    Top::Frame => 0.0,
                });
            let height = self.frames[f].height;
            let mut next: VecDeque<Slot> = cols
                .iter()
                .take(k)
                .enumerate()
                .map(|(c, band)| Slot::column(f, c, *band, Top::Anchor(below), height))
                .collect();
            let rest: Vec<Slot> = self
                .pending
                .drain(..)
                .chain(self.cur.take())
                .filter(|s| s.frame == f && s.col.is_some_and(|c| c >= k) && s.last.is_none())
                .collect();
            let mut rest = rest;
            rest.sort_by_key(|s| s.col);
            next.extend(rest);
            self.pending = next;
            self.band_deepest = None;
            self.cur = self.pending.pop_front();
            *at = At { p: at.p + 1, i: 0 };
            return true;
        }
    }

    /// A block of consecutive split paragraphs from `at`.
    fn split(&mut self, at: &mut At) -> bool {
        let Kind::Split { k, inside, outside } = self.paras[at.p].kind else {
            return false;
        };
        let mut end = at.p;
        while end < self.paras.len() && matches!(self.kind_here(end), Kind::Split { .. }) {
            end += 1;
        }
        loop {
            if self.cur.is_none() && !self.next_slot() {
                return false;
            }
            let container = self.cur.take().expect("a slot");
            let k = k as usize;
            let sub_w =
                ((container.width - 2.0 * outside - (k as f32 - 1.0) * inside) / k as f32).max(1.0);
            let first = self.paras[at.p];
            let top = match (container.last, container.top) {
                (Some(y), _) if at.i == 0 => Top::Anchor(y + first.before_block()),
                (Some(y), _) => Top::Anchor(y),
                (None, Top::Anchor(y)) if at.i == 0 => Top::Anchor(y + first.before_block()),
                (None, t) => t,
            };
            let mut subs: Vec<Slot> = (0..k)
                .map(|j| Slot {
                    frame: container.frame,
                    col: None,
                    x: container.x + outside + j as f32 * (sub_w + inside),
                    width: sub_w,
                    top,
                    bottom: container.bottom,
                    last: None,
                    region: None,
                    band: false,
                })
                .collect();
            let mut lines: Vec<(usize, usize)> = Vec::new();
            for p in at.p..end {
                let n = self.line_count(sub_w, p);
                let from = if p == at.p { at.i } else { 0 };
                lines.extend((from..n).map(|i| (p, i)));
            }
            if lines.is_empty() {
                self.cur = Some(container);
                *at = At { p: end, i: 0 };
                return true;
            }
            let per = lines.len().div_ceil(k);
            let balanced = self.fits_in(&subs, &lines, per);
            // Balanced when the block fits; else each sub-column to the
            // bottom and the rest in the next column or frame.
            let mut li = 0;
            for slot in subs.iter_mut() {
                let mut placed = 0;
                while li < lines.len() && (!balanced || placed < per) {
                    let (p, i) = lines[li];
                    let lead = self.leads(sub_w, p)[i];
                    let base = self.baseline_in(slot, p, i, lead);
                    if base > slot.bottom + EPS {
                        break;
                    }
                    self.commit(slot, p, i, base);
                    li += 1;
                    placed += 1;
                }
                if balanced {
                    if let Some(&(p, i)) = lines.get(li) {
                        let lead = self.leads(sub_w, p)[i];
                        self.cut(slot, lead);
                    }
                }
            }
            if li >= lines.len() {
                let last = &self.paras[end - 1];
                let deepest = subs
                    .iter()
                    .filter_map(|s| s.last)
                    .fold(f32::NEG_INFINITY, f32::max);
                let below = deepest + last.after_block();
                let remainder = Slot {
                    top: Top::Anchor(below),
                    last: None,
                    region: None,
                    ..container
                };
                if remainder.band {
                    self.band_deepest = Some(self.band_deepest.map_or(deepest, |d| d.max(deepest)));
                }
                self.cur = Some(remainder);
                *at = At { p: end, i: 0 };
                return true;
            }
            let (p, i) = lines[li];
            *at = At { p, i };
            if !self.next_slot() {
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn single() -> ParaSpec {
        ParaSpec {
            kind: Kind::Single,
            space_before: 0.0,
            space_after: 0.0,
            min_before: 0.0,
            min_after: 0.0,
        }
    }

    fn with(kind: Kind) -> ParaSpec {
        ParaSpec { kind, ..single() }
    }

    /// Two 100 pt columns, 20 pt gutter, 126 pt tall: ten 12 pt lines.
    fn frame(columns: usize) -> FrameSpec {
        FrameSpec {
            height: 126.0,
            inset_top: 0.0,
            columns: (0..columns).map(|c| (c as f32 * 120.0, 100.0)).collect(),
            leading_offset: true,
        }
    }

    /// Every paragraph one 12 pt line at any width.
    fn one_liners(n: usize) -> impl FnMut(f32) -> Rc<Measured> {
        move |_| {
            Rc::new(Measured {
                leads: vec![vec![12.0]; n],
                first_offset: 12.0,
            })
        }
    }

    /// (region index, baseline) of every paragraph's first line.
    fn starts(plan: &Plan, n: usize) -> Vec<usize> {
        (0..n as u32).map(|p| plan.starts[&p]).collect()
    }

    #[test]
    fn text_above_a_span_is_balanced_and_the_text_below_fills() {
        // P01–P06, span, P07–P18 in two columns (InDesign: 3 / 3, the
        // span at the fourth line, then six and six).
        let mut paras = vec![single(); 19];
        paras[6] = with(Kind::Span(None));
        let plan = plan(&[frame(2), frame(2)], &paras, &mut one_liners(19));
        let s = starts(&plan, 19);
        assert_eq!(&s[0..3], &[0, 0, 0]);
        assert_eq!(&s[3..6], &[1, 1, 1]);
        let span = &plan.regions[s[6]];
        assert_eq!((span.x, span.width), (0.0, 220.0));
        assert_eq!(span.top, Top::Anchor(36.0));
        assert_eq!(plan.regions[s[7]].top, Top::Anchor(48.0));
        assert_eq!(s[7], s[12]);
        assert_eq!(plan.regions[s[13]].x, 120.0);
        assert_eq!(s[13], s[18]);
    }

    #[test]
    fn a_span_over_two_of_three_leaves_the_third_column_full_height() {
        let mut paras = vec![single(); 41];
        paras[6] = with(Kind::Span(Some(2)));
        let plan = plan(&[frame(3), frame(3)], &paras, &mut one_liners(41));
        let s = starts(&plan, 41);
        assert_eq!(plan.regions[s[3]].x, 120.0, "P04 opens column two");
        assert_eq!(plan.regions[s[6]].width, 220.0);
        // P19 (after the span, so index 19) opens column three at the
        // frame's top.
        let third = &plan.regions[s[19]];
        assert_eq!((third.x, third.top), (240.0, Top::Frame));
        assert_eq!(plan.regions[s[29]].frame, 1, "P29 opens the next frame");
    }

    #[test]
    fn a_span_with_no_room_moves_to_the_next_frame() {
        let mut paras = vec![single(); 27];
        paras[20] = with(Kind::Span(None));
        let plan = plan(&[frame(2), frame(2)], &paras, &mut one_liners(27));
        let span = &plan.regions[plan.starts[&20]];
        assert_eq!((span.frame, span.top), (1, Top::Frame));
    }

    #[test]
    fn span_spacing_is_the_larger_of_space_and_min_space() {
        let mut paras = vec![single(); 15];
        paras[4] = ParaSpec {
            kind: Kind::Span(None),
            space_before: 10.0,
            min_before: 4.0,
            space_after: 9.0,
            min_after: 5.0,
        };
        let plan = plan(&[frame(2)], &paras, &mut one_liners(15));
        // Two lines above (24), +10 → anchor 34; span at 46; +9 → 55.
        assert_eq!(plan.regions[plan.starts[&4]].top, Top::Anchor(34.0));
        assert_eq!(plan.regions[plan.starts[&5]].top, Top::Anchor(55.0));
    }

    #[test]
    fn a_split_block_balances_by_line_count() {
        // P01, five split paragraphs (3 / 2), P07.
        let split = Kind::Split {
            k: 2,
            inside: 20.0,
            outside: 0.0,
        };
        let mut paras = vec![single(); 7];
        for p in &mut paras[1..6] {
            *p = with(split);
        }
        let plan = plan(&[frame(1)], &paras, &mut one_liners(7));
        let s = starts(&plan, 7);
        assert_eq!(s[1], s[3]);
        assert_eq!(s[4], s[5]);
        assert_ne!(s[1], s[4]);
        let (a, b) = (&plan.regions[s[1]], &plan.regions[s[4]]);
        assert_eq!((a.x, a.width, b.x), (0.0, 40.0, 60.0));
        // Below the deeper sub-column: three lines from 12 → 48.
        assert_eq!(plan.regions[s[6]].top, Top::Anchor(48.0));
    }

    #[test]
    fn a_split_block_that_does_not_fit_continues_balanced() {
        let split = Kind::Split {
            k: 2,
            inside: 20.0,
            outside: 0.0,
        };
        let mut paras = vec![single(); 24];
        for p in &mut paras[6..22] {
            *p = with(split);
        }
        let plan = plan(&[frame(1), frame(1)], &paras, &mut one_liners(24));
        let s = starts(&plan, 24);
        // Four lines fit in each sub-column of frame A (P07–P14), the
        // other eight go 4 / 4 into frame B.
        assert_eq!(plan.regions[s[10]].frame, 0);
        assert_eq!(plan.regions[s[14]].frame, 1);
        assert_eq!(s[14], s[17]);
        assert_eq!(s[18], s[21]);
        // P23 below the deeper sub-column (baseline 48 in frame B).
        assert_eq!(plan.regions[s[22]].top, Top::Anchor(48.0));
    }
}
