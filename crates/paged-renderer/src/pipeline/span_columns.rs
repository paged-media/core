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
//!   sub-columns). The next paragraph starts below its deepest
//!   sub-column. A block that does not fit fills its sub-columns to the
//!   bottom and continues, balanced again, in the next column or frame.
//! - **Block boundaries** (the `split-boundaries` fixture, same day). A
//!   new block starts where the count or either gutter changes (2 → 3,
//!   inside 6 → 20, outside 0 → 10); paragraphs that differ only in min
//!   space, or spell the defaults out, stay one block. The next block
//!   starts directly below the deepest sub-column of the last, and at
//!   every boundary between a split paragraph and its neighbour (a
//!   block, or ordinary text) the spacing is
//!   `max(SpaceAfter + SpaceBefore, the split side's min space)`: the
//!   ending block's `SpanColumnMinSpaceAfter`, the starting one's
//!   `SpanColumnMinSpaceBefore` (after 12 / before 6 → 12, after 6 /
//!   before 12 → 12, space 4 + 3 → 7, space 4 + 3 with min before 10 →
//!   10). Inside a block paragraphs are spaced as ever.
//! - **A split above a span.** When the text balanced above a span holds
//!   a split block, InDesign sets it at the least column height at which
//!   all of it fits the spanned columns, a split block filling its
//!   sub-columns to that height rather than balancing them (P01 and a
//!   split of four over two columns: P01 / P02 | P03, then P04 over P05).
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
    /// One of the band's spanned (group) columns (a sub-column inherits
    /// its column's).
    band: bool,
    /// The paragraph this slot's `Anchor` top follows across a block
    /// boundary: the first line opening it adds that boundary's spacing.
    follows: Option<usize>,
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
            follows: None,
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
        confined: false,
        overflow: None,
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
    /// A balancing trial: the slots in hand are all there is (no next
    /// frame), and split blocks fill their sub-columns to the bottom.
    confined: bool,
    /// During a trial, the shallowest baseline that did not fit.
    overflow: Option<f32>,
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
            if at.i == 0 && self.at_band_start() && self.balance_above_span(&mut at) {
                continue;
            }
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
        if self.confined {
            self.cur = None;
            return false;
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

    /// The split block a paragraph belongs to: its count and gutters
    /// (InDesign starts a new block when any of them changes, not when
    /// only the min spaces do). `None` for a paragraph in no block.
    fn block_key(&self, p: usize) -> Option<Kind> {
        match self.kind_here(p) {
            k @ Kind::Split { .. } => Some(k),
            _ => None,
        }
    }

    /// The spacing between paragraph `p - 1` and line `i` of `p`. Where
    /// a split block begins or ends between them it is the larger of
    /// `SpaceAfter + SpaceBefore` and the split side's min space.
    fn gap(&self, p: usize, i: usize) -> f32 {
        if i != 0 || p == 0 {
            return 0.0;
        }
        let (a, b) = (&self.paras[p - 1], &self.paras[p]);
        let mut g = a.space_after + b.space_before;
        let (ka, kb) = (self.block_key(p - 1), self.block_key(p));
        if ka != kb {
            if ka.is_some() {
                g = g.max(a.min_after);
            }
            if kb.is_some() {
                g = g.max(b.min_before);
            }
        }
        g
    }

    /// The top a slot opens with for line (`p`, `i`): an anchor that
    /// follows a block boundary takes that boundary's spacing.
    fn opening_top(&self, slot: &Slot, p: usize, i: usize) -> Top {
        match (slot.top, slot.follows) {
            (Top::Anchor(y), Some(_)) => Top::Anchor(y + self.gap(p, i)),
            (t, _) => t,
        }
    }

    /// Where line (`p`, `i`) with leading `lead` would sit in `slot`.
    fn baseline_in(&mut self, slot: &Slot, p: usize, i: usize, lead: f32) -> f32 {
        match (slot.last, self.opening_top(slot, p, i)) {
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
                let top = self.opening_top(slot, p, i);
                self.out.regions.push(Region {
                    frame: slot.frame,
                    x: slot.x,
                    width: slot.width,
                    top,
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
            self.note_overflow(base);
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

    fn note_overflow(&mut self, base: f32) {
        if self.confined {
            self.overflow = Some(self.overflow.map_or(base, |o| o.min(base)));
        }
    }

    /// Text above a span, from the top of a band, is balanced over the
    /// spanned columns. `true` when it was (and `at` is the span).
    fn balance_above_span(&mut self, at: &mut At) -> bool {
        if self.confined {
            return false;
        }
        let mut s = at.p;
        let mut split = false;
        while s < self.paras.len() {
            match self.kind_here(s) {
                Kind::Single => {}
                Kind::Split { .. } => split = true,
                Kind::Span(_) => break,
            }
            s += 1;
        }
        let Some(Kind::Span(k)) = (s < self.paras.len()).then(|| self.kind_here(s)) else {
            return false;
        };
        let k = k.unwrap_or(u32::MAX) as usize;
        let done = if split {
            self.balanced_by_height(at.p, s, k)
        } else {
            self.balanced_run(at.p, s, k)
        };
        if done {
            *at = At { p: s, i: 0 };
        }
        done
    }

    /// [`Self::balanced_run`] for text that holds split blocks: InDesign
    /// sets it at the least column height at which all of it fits the
    /// spanned columns, a split block filling its sub-columns to that
    /// height (measured: 1 line + a split of 4 over 2 columns sets
    /// P01 / P02 | P03 in the first, P04 over P05 in the second).
    fn balanced_by_height(&mut self, from: usize, to: usize, k: usize) -> bool {
        let Some(cur) = self.cur.clone() else {
            return false;
        };
        let f = self.frame;
        let (band, others): (Vec<Slot>, Vec<Slot>) = {
            let mut band = vec![cur];
            let mut others = Vec::new();
            for slot in &self.pending {
                if slot.band && slot.frame == f && band.len() < k.max(1) {
                    band.push(slot.clone());
                } else {
                    others.push(slot.clone());
                }
            }
            (band, others)
        };
        let saved = (self.out.clone(), self.band_deepest, self.pending.clone());
        let limit = self.frames[f].height;
        let first = {
            let lead = self
                .leads(band[0].width, from)
                .first()
                .copied()
                .unwrap_or(0.0);
            self.baseline_in(&band[0], from, 0, lead)
        };
        let mut bottom = first;
        self.confined = true;
        let placed = loop {
            if bottom > limit + EPS {
                break false;
            }
            self.out = saved.0.clone();
            self.band_deepest = saved.1;
            self.overflow = None;
            let mut slots: VecDeque<Slot> = band
                .iter()
                .map(|s| Slot {
                    bottom: bottom.min(s.bottom),
                    ..s.clone()
                })
                .collect();
            self.cur = slots.pop_front();
            self.pending = slots;
            let mut at = At { p: from, i: 0 };
            let mut ok = true;
            while ok && at.p < to {
                ok = match self.kind_here(at.p) {
                    Kind::Split { .. } => self.split(&mut at),
                    _ => self.single(&mut at),
                };
            }
            if ok {
                break true;
            }
            match self.overflow {
                Some(o) if o > bottom + EPS => bottom = o,
                _ => break false,
            }
        };
        self.confined = false;
        self.overflow = None;
        if !placed {
            self.out = saved.0;
            self.band_deepest = saved.1;
            self.pending = saved.2;
            self.cur = band.into_iter().next();
            return false;
        }
        // The band's spanned columns are spent; what is left of the frame
        // is its other columns, after the span.
        self.pending = others.into();
        self.cur = None;
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
                follows: None,
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
        let key = self.block_key(at.p);
        let Some(Kind::Split { k, inside, outside }) = key else {
            return false;
        };
        let mut end = at.p;
        while end < self.paras.len() && self.block_key(end) == key {
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
                (Some(y), _) if at.i == 0 => Top::Anchor(y + self.gap(at.p, 0)),
                (Some(y), _) => Top::Anchor(y),
                (None, Top::Anchor(_)) if at.i == 0 && container.follows.is_some() => {
                    self.opening_top(&container, at.p, 0)
                }
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
                    band: container.band,
                    follows: None,
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
            let balanced = !self.confined && self.fits_in(&subs, &lines, per);
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
                        self.note_overflow(base);
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
                // What follows starts below the deepest sub-column, by
                // the boundary's spacing (`gap`).
                let deepest = subs
                    .iter()
                    .filter_map(|s| s.last)
                    .fold(f32::NEG_INFINITY, f32::max);
                let remainder = Slot {
                    top: Top::Anchor(deepest),
                    last: None,
                    region: None,
                    follows: Some(end - 1),
                    ..container
                };
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

    fn split(k: u32, inside: f32) -> ParaSpec {
        with(Kind::Split {
            k,
            inside,
            outside: 0.0,
        })
    }

    #[test]
    fn a_new_count_starts_a_new_block_below_the_last() {
        // P01, four split 2, six split 3, P12 (InDesign: 2 / 2, then
        // 2 / 2 / 2 directly below, then P12).
        let mut paras = vec![single(); 12];
        for p in &mut paras[1..5] {
            *p = split(2, 20.0);
        }
        for p in &mut paras[5..11] {
            *p = split(3, 5.0);
        }
        let plan = plan(&[frame(1)], &paras, &mut one_liners(12));
        let s = starts(&plan, 12);
        assert_eq!((s[1], s[3]), (s[2], s[4]));
        assert_ne!(s[1], s[3]);
        assert_eq!(&s[5..11], &[s[5], s[5], s[7], s[7], s[9], s[9]]);
        let b2 = &plan.regions[s[5]];
        assert_eq!((b2.width, b2.top), (30.0, Top::Anchor(36.0)));
        assert_eq!(plan.regions[s[9]].x, 70.0);
        assert_eq!(plan.regions[s[11]].top, Top::Anchor(60.0));
    }

    #[test]
    fn a_change_of_min_space_alone_keeps_one_block() {
        let mut paras = vec![single(); 8];
        for p in &mut paras[1..4] {
            *p = split(2, 20.0);
        }
        for p in &mut paras[4..7] {
            *p = ParaSpec {
                min_before: 6.0,
                min_after: 12.0,
                ..split(2, 20.0)
            };
        }
        let plan = plan(&[frame(1)], &paras, &mut one_liners(8));
        let s = starts(&plan, 8);
        // Six lines, 3 / 3 (rows 24–48); P08 12 below (the last
        // paragraph's min space after).
        assert_eq!(&s[1..7], &[s[1], s[1], s[1], s[4], s[4], s[4]]);
        assert_ne!(s[1], s[4]);
        assert_eq!(plan.regions[s[7]].top, Top::Anchor(60.0));
    }

    #[test]
    fn block_boundary_spacing_is_the_larger_of_space_and_min_space() {
        let run = |a: ParaSpec, b: ParaSpec| {
            let mut paras = vec![single(); 12];
            for p in &mut paras[1..5] {
                *p = a;
            }
            for p in &mut paras[5..11] {
                *p = b;
            }
            let plan = plan(&[frame(1)], &paras, &mut one_liners(12));
            plan.regions[plan.starts[&5]].top
        };
        let a = |space_after, min_after| ParaSpec {
            space_after,
            min_after,
            ..split(2, 20.0)
        };
        let b = |space_before, min_before| ParaSpec {
            space_before,
            min_before,
            ..split(3, 5.0)
        };
        // Block one's rows sit at 24 and 36 (36 + the inner space after,
        // where there is one).
        assert_eq!(run(a(0.0, 12.0), b(0.0, 6.0)), Top::Anchor(48.0));
        assert_eq!(run(a(0.0, 6.0), b(0.0, 12.0)), Top::Anchor(48.0));
        assert_eq!(run(a(4.0, 0.0), b(3.0, 0.0)), Top::Anchor(47.0));
        assert_eq!(run(a(4.0, 0.0), b(3.0, 10.0)), Top::Anchor(50.0));
    }

    #[test]
    fn a_split_above_a_span_fills_to_the_least_height_that_fits() {
        // P01, four split 2, span, P06–P11 in two columns (InDesign:
        // P01 / P02 | P03, then P04 over P05; the span below 24).
        let mut paras = vec![single(); 12];
        for p in &mut paras[1..5] {
            *p = split(2, 20.0);
        }
        paras[5] = with(Kind::Span(None));
        let plan = plan(&[frame(2)], &paras, &mut one_liners(12));
        let s = starts(&plan, 12);
        assert_ne!(s[1], s[2]);
        assert_eq!(plan.regions[s[2]].x, 60.0);
        assert_eq!(s[3], s[4], "P04 and P05 share a sub-column");
        assert_eq!(plan.regions[s[3]].x, 120.0);
        assert_eq!(plan.regions[s[5]].top, Top::Anchor(24.0));
    }

    #[test]
    fn a_split_above_a_span_that_cannot_balance_flows_on() {
        // Fifty split lines (25 rows) cannot sit in two 10-row columns:
        // the block fills column one, then column two, then frame B,
        // and the span follows it there.
        let mut paras = vec![split(2, 20.0); 51];
        paras[50] = with(Kind::Span(None));
        let plan = plan(&[frame(2), frame(2)], &paras, &mut one_liners(51));
        let s = starts(&plan, 51);
        let col2 = &plan.regions[s[20]];
        assert_eq!((col2.frame, col2.x), (0, 120.0));
        assert_eq!(plan.regions[s[40]].frame, 1);
        assert_eq!(plan.regions[s[50]].frame, 1);
    }
}
