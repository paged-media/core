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

//! The region-chain **flow protocol** — the first executable seam of the
//! Paged-native composition model (ADR-021, `thoughts/docs/paged/core/
//! composition-format.md` §5).
//!
//! A **flow** names a content sequence in a part that fragments across an
//! **ordered set of regions**. The composition owns the *chain, order, and
//! overset*; the content engine owns *the content and how it fragments*
//! (the LESSONS #1 rope/view split: composition owns the views/region-chain,
//! the part owns the story).
//!
//! This crate is deliberately **content-agnostic and dependency-light**: it
//! knows nothing about text, HTML, tables, or IDML. It defines the vocabulary
//! ([`FlowId`], [`Region`], [`RegionChain`]) plus a driver ([`run_flow`]) that
//! walks a region-chain and threads a [`FlowContent`] engine's content across
//! it, surfacing **overset as a first-class continuation** ([`Overset`]) —
//! *not* dropped, in contrast to today's IDML story emitter which discards
//! overflow and only emits an `OversetTextDropped` diagnostic.
//!
//! It generalizes two proven implementations of the same shape:
//! - the IDML story→frame threading (`paged-scene::Document::frame_chain` +
//!   the renderer's `StoryEmitter` cursor), and
//! - the paged.web fragmentation lane (`plugin-web/.../flow.rs`: fragment one
//!   HTML flow across a list of frame geometries → per-frame content + overset).
//!
//! The engine-specific concerns those implementations hardwire — *measuring a
//! laid-out box* and *the straddler-fragmentation ladder* — live behind
//! [`FlowContent::place`]; the driver only orchestrates region order and the
//! cursor.

use serde::{Deserialize, Serialize};

/// Stable identifier for a **flow** — a content sequence in a part that
/// fragments across a region-chain (composition-format §5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FlowId(pub String);

impl FlowId {
    pub fn new(id: impl Into<String>) -> Self {
        FlowId(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identifier for a **region** — a placement unit / one link in a
/// region-chain (composition-format §3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionId(pub String);

impl RegionId {
    pub fn new(id: impl Into<String>) -> Self {
        RegionId(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The geometry a flow fragment fills **within one region** — the region's
/// content box. Units are **points (pt)**, matching the renderer.
///
/// This generalizes paged.web's minimal `(width, height)` region descriptor
/// with columns: width drives (re-)line-breaking, height is the cut limit.
/// Position on the page is *not* here — it is the region's positioning
/// constraint (composition-format §4); a flow fragments by *size*, and each
/// region is treated from its own content origin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionGeometry {
    /// Content-box width in pt.
    pub width_pt: f32,
    /// Content-box height in pt (the fragment's fill limit).
    pub height_pt: f32,
    /// Number of text columns the content box splits into (>= 1).
    pub columns: u32,
    /// Gap between adjacent columns, in pt.
    pub column_gap_pt: f32,
}

impl RegionGeometry {
    /// A single-column content box of the given size.
    pub fn new(width_pt: f32, height_pt: f32) -> Self {
        RegionGeometry {
            width_pt,
            height_pt,
            columns: 1,
            column_gap_pt: 0.0,
        }
    }
}

/// One link in a region-chain: a region (its id + content-box geometry).
///
/// A region *windows* a part's content; it never contains content. The order
/// of regions within a [`RegionChain`] is the flow order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub id: RegionId,
    pub geometry: RegionGeometry,
}

impl Region {
    pub fn new(id: impl Into<String>, geometry: RegionGeometry) -> Self {
        Region {
            id: RegionId::new(id),
            geometry,
        }
    }
}

/// An **ordered set of regions** a flow's content fragments across, in order.
/// The composition owns this chain; the content engine (via [`FlowContent`])
/// owns what actually fills each region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionChain {
    pub flow: FlowId,
    pub regions: Vec<Region>,
}

impl RegionChain {
    pub fn new(flow: FlowId, regions: Vec<Region>) -> Self {
        RegionChain { flow, regions }
    }

    /// Number of regions in the chain.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// A chain with no regions — everything is overset (nowhere to place).
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

/// The result of placing content into **one** region.
///
/// `fragment` is the engine's product for this region (laid-out lines, a
/// scene layer, …) and may be *empty* when nothing fit — e.g. an atomic unit
/// taller than the region, which then moves whole to the next region (the
/// paged.web "move-whole" case). `next` is the continuation cursor after what
/// was placed, or `None` when the content is exhausted within this region.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement<Fragment, Cursor> {
    pub fragment: Fragment,
    pub next: Option<Cursor>,
}

/// Overset — the flow's terminal state after the driver has walked the chain.
///
/// Unlike today's renderer (which *drops* overflow past the last frame), the
/// leftover is preserved as a **first-class continuation cursor**, so a caller
/// can grow the chain, report the overset honestly, or continue the flow
/// elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub enum Overset<Cursor> {
    /// The content fit within the chain.
    Fits,
    /// Content remains past the last region; here is where to continue.
    Remains(Cursor),
}

impl<Cursor> Overset<Cursor> {
    /// True when content overran the chain.
    pub fn is_overset(&self) -> bool {
        matches!(self, Overset::Remains(_))
    }
}

/// What a fully-run flow produced.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowRun<Fragment, Cursor> {
    /// `(region id, fragment)` for each region the content reached, in order.
    /// A region reached with content still remaining always gets an entry
    /// (possibly an empty fragment); regions past an exhausted content are not
    /// reached and get no entry.
    pub placements: Vec<(RegionId, Fragment)>,
    /// Content past the last region — a first-class continuation, not dropped.
    pub overset: Overset<Cursor>,
}

/// A content engine that can fragment its content across a region-chain.
///
/// **Content-agnostic:** the [driver][run_flow] knows nothing about text,
/// HTML, or tables — only how to walk regions and thread the cursor. The two
/// content-specific concerns (measuring a laid-out box, and the
/// straddler-fragmentation ladder) live entirely inside [`place`][Self::place].
///
/// - [`Fragment`][Self::Fragment] is the engine's product for one region.
/// - [`Cursor`][Self::Cursor] is an opaque position within the content — a
///   continuation point. `Clone` so overset can hand it back.
pub trait FlowContent {
    /// The engine-defined fragment produced for one region (opaque to the
    /// protocol).
    type Fragment;
    /// The engine-defined position within the content — a continuation point
    /// (opaque to the protocol).
    type Cursor: Clone;

    /// The cursor at the start of the content.
    fn start(&self) -> Self::Cursor;

    /// Place remaining content (from `cursor`) into `region`, returning the
    /// fragment placed and the continuation cursor (`None` = exhausted).
    ///
    /// Contract: an engine must either make progress (return `next` advanced
    /// past `cursor`) or, when an atomic unit does not fit, return an empty
    /// fragment with `next == Some(cursor)` so the unit moves whole to the
    /// next region. The driver walks each region at most once, so it always
    /// terminates regardless.
    fn place(
        &self,
        region: &Region,
        cursor: Self::Cursor,
    ) -> Placement<Self::Fragment, Self::Cursor>;
}

/// Run a content engine's content across a region-chain, in order.
///
/// Orchestrates region order and threads the cursor; **content-agnostic**.
/// Always terminates: it walks each region at most once. Overset (content
/// past the last region) is returned as a first-class [`Overset::Remains`]
/// continuation cursor.
pub fn run_flow<C: FlowContent>(
    content: &C,
    chain: &RegionChain,
) -> FlowRun<C::Fragment, C::Cursor> {
    let mut placements = Vec::with_capacity(chain.regions.len());
    let mut cursor = Some(content.start());

    for region in &chain.regions {
        // Content already exhausted by an earlier region → stop; later
        // regions are simply empty in this flow (they get no placement).
        let Some(cur) = cursor.take() else { break };
        let placement = content.place(region, cur);
        placements.push((region.id.clone(), placement.fragment));
        cursor = placement.next;
    }

    let overset = match cursor {
        // Ran out of regions before the content was exhausted.
        Some(cur) => Overset::Remains(cur),
        None => Overset::Fits,
    };

    FlowRun {
        placements,
        overset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic content engine that proves the flow protocol is
    /// content-agnostic — **no IDML, no text engine, no renderer**. Content is
    /// a list of fixed-height "lines"; a region holds as many *whole* lines as
    /// fit under its height (a line taller than the region does not fit and
    /// moves whole to the next region). This is the protocol-level analogue of
    /// the C1 test "hand-build a composition with a native part, no IDML".
    struct LineList {
        line_heights: Vec<f32>,
    }

    impl FlowContent for LineList {
        /// Indices of the lines placed in this region.
        type Fragment = Vec<usize>;
        /// Index of the next line to place.
        type Cursor = usize;

        fn start(&self) -> usize {
            0
        }

        fn place(&self, region: &Region, cursor: usize) -> Placement<Vec<usize>, usize> {
            let mut placed = Vec::new();
            let mut used = 0.0f32;
            let mut i = cursor;
            while i < self.line_heights.len() {
                let h = self.line_heights[i];
                // A tiny epsilon so exact fits land (float slack).
                if used + h <= region.geometry.height_pt + 0.01 {
                    used += h;
                    placed.push(i);
                    i += 1;
                } else {
                    break;
                }
            }
            let next = if i < self.line_heights.len() {
                Some(i)
            } else {
                None
            };
            Placement {
                fragment: placed,
                next,
            }
        }
    }

    fn chain(heights: &[f32]) -> RegionChain {
        let regions = heights
            .iter()
            .enumerate()
            .map(|(i, &h)| Region::new(format!("r{i}"), RegionGeometry::new(200.0, h)))
            .collect();
        RegionChain::new(FlowId::new("f1"), regions)
    }

    #[test]
    fn exact_fit_no_overset() {
        let content = LineList {
            line_heights: vec![10.0, 10.0, 10.0],
        };
        let run = run_flow(&content, &chain(&[30.0]));
        assert_eq!(run.placements.len(), 1);
        assert_eq!(run.placements[0].0, RegionId::new("r0"));
        assert_eq!(run.placements[0].1, vec![0, 1, 2]);
        assert_eq!(run.overset, Overset::Fits);
    }

    #[test]
    fn overflows_into_second_region() {
        let content = LineList {
            line_heights: vec![10.0, 10.0, 10.0, 10.0],
        };
        let run = run_flow(&content, &chain(&[25.0, 25.0]));
        assert_eq!(run.placements.len(), 2);
        assert_eq!(run.placements[0].1, vec![0, 1]);
        assert_eq!(run.placements[1].1, vec![2, 3]);
        assert_eq!(run.overset, Overset::Fits);
    }

    #[test]
    fn overset_past_last_region_is_a_continuation() {
        let content = LineList {
            line_heights: vec![10.0, 10.0, 10.0, 10.0],
        };
        // One 25pt region holds two 10pt lines; lines 2 & 3 overrun.
        let run = run_flow(&content, &chain(&[25.0]));
        assert_eq!(run.placements.len(), 1);
        assert_eq!(run.placements[0].1, vec![0, 1]);
        // The leftover is a first-class cursor, not dropped: continue at line 2.
        assert_eq!(run.overset, Overset::Remains(2));
        assert!(run.overset.is_overset());
    }

    #[test]
    fn variable_region_heights() {
        let content = LineList {
            line_heights: vec![10.0, 10.0, 10.0, 10.0],
        };
        // r0=15pt holds one line (20>15), r1=25pt holds two (30>25) → line 3 overruns.
        let run = run_flow(&content, &chain(&[15.0, 25.0]));
        assert_eq!(run.placements[0].1, vec![0]);
        assert_eq!(run.placements[1].1, vec![1, 2]);
        assert_eq!(run.overset, Overset::Remains(3));
    }

    #[test]
    fn empty_chain_is_all_overset() {
        let content = LineList {
            line_heights: vec![10.0],
        };
        let run = run_flow(&content, &chain(&[]));
        assert!(run.placements.is_empty());
        // Nowhere to place → continue from the start.
        assert_eq!(run.overset, Overset::Remains(0));
    }

    #[test]
    fn atomic_taller_than_region_moves_whole_then_fits() {
        let content = LineList {
            line_heights: vec![30.0, 10.0],
        };
        // r0=20pt cannot hold the 30pt line (empty, move whole); r1=40pt holds both.
        let run = run_flow(&content, &chain(&[20.0, 40.0]));
        assert_eq!(run.placements[0].1, Vec::<usize>::new());
        assert_eq!(run.placements[1].1, vec![0, 1]);
        assert_eq!(run.overset, Overset::Fits);
    }

    #[test]
    fn exhausted_content_leaves_trailing_regions_unreached() {
        let content = LineList {
            line_heights: vec![10.0],
        };
        // Three regions but content fits in the first → regions 1 & 2 unreached.
        let run = run_flow(&content, &chain(&[50.0, 50.0, 50.0]));
        assert_eq!(run.placements.len(), 1);
        assert_eq!(run.placements[0].1, vec![0]);
        assert_eq!(run.overset, Overset::Fits);
    }

    #[test]
    fn region_chain_json_roundtrips() {
        // The composition data types serialize to `document.pgd` (JSON).
        let c = chain(&[100.0, 200.0]);
        let json = serde_json::to_string(&c).unwrap();
        let back: RegionChain = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
