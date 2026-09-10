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

//! Auto-sizing text frames, measured the way InDesign resizes them.
//!
//! An `AutoSizingType` frame is authored at one size and composed at
//! another: InDesign fits the box to its text before the text is laid
//! out. The rules below were measured in InDesign 20.0.1 (2026-09-06)
//! on 130 × 42 pt frames holding 8 pt / 13 pt-leading text, and every
//! mode turned out to be a *fit*, not a growth — a frame shrinks below
//! its authored size just as readily as it grows past it:
//!
//! - **HeightOnly** — the height becomes the last line's baseline
//!   (the descender overhangs, exactly as a line "fits" when its
//!   baseline is inside the frame): 6 lines → 8.29 + 5 × 13 = 73.29 pt,
//!   one short line → 8.29 pt.
//! - **WidthOnly** — the height is fixed, so it holds a fixed number of
//!   lines; the width becomes the *smallest* width at which the text
//!   fits in that many lines (3 lines in 42 pt → 223.0 pt from either a
//!   130 pt or a 400 pt start; "Short text." → 32.8 pt on two lines).
//! - **HeightAndWidth** — the width becomes the smallest width at which
//!   no line overflows the column (every line one unbreakable fragment:
//!   "Au / to-siz / ing / lets the / box …", 26 pt wide from either a
//!   130 pt or a 400 pt start), and the height follows the line count.
//! - **HeightAndWidthProportionally** — both axes scale by the smallest
//!   factor at which the text fits (130 × 42 → 170.3 × 55.0, aspect
//!   kept to four decimals).
//!
//! `MinimumWidthForAutoSizing` / `MinimumHeightForAutoSizing` floor the
//! fitted size, and the `AutoSizingReferencePoint` pins the corner or
//! edge that stays put while the box moves.
//!
//! One quirk is reproduced on purpose. InDesign fits a `WidthOnly`
//! frame by first composing it at unbounded width; when a
//! `NextColumnTextWrap` obstacle anywhere on the spread sits in that
//! growth path (same vertical band, any distance), the unbounded line
//! runs into it, the text jumps to a column that doesn't exist, and the
//! frame is left stretched exactly to the obstacle's edge with nothing
//! composed — measured at 1014.6 pt for an obstacle 507 pt from the
//! centre and 1550 pt for one 775 pt away, with the 223 pt fit never
//! reached. [`AutoSized::text_dropped`] carries that verdict to the
//! emitter, which oversets every line of the story.
//!
//! Every fit is measured with the real composer: the frame is cloned at
//! a candidate size and its story emitted into a scratch page, so the
//! line breaks, hyphenation, indents, insets and first-baseline rule
//! are the ones the final layout uses. A binary search over the size
//! converges to 0.02 pt in ~15 trials.

use super::*;

use paged_model::{
    AutoSizingReferencePoint as RP, AutoSizingType, Bounds, TextFrame, TextWrapMode,
};
use paged_scene::Document;

use super::build_engine::StoryEmitter;
use super::font_table::FontTable;
use super::geom::transform_bounds;

/// Fitted geometry for one auto-sizing frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct AutoSized {
    /// Inner-coord bounds (pre `ItemTransform`) the frame composes and
    /// paints at.
    pub(super) bounds: Bounds,
    /// InDesign composed nothing in this frame (the auto-size stalled on
    /// a `NextColumnTextWrap` obstacle); every line of the story is
    /// overset.
    pub(super) text_dropped: bool,
}

/// Convergence for the binary searches, in pt (width / height) — well
/// under a glyph's advance, so the last trial's line breaks are the
/// final layout's.
const FIT_TOLERANCE_PT: f32 = 0.02;
/// "Unbounded" width / height for the measuring trials.
const UNBOUNDED_PT: f32 = 100_000.0;
/// Largest scale the proportional fit will try before giving up.
const MAX_PROPORTIONAL_SCALE: f32 = 64.0;

/// One measured composition of a frame at a trial size.
struct Trial {
    /// Some line didn't fit the frame (the emitter's overset verdict).
    overset: bool,
    /// Lines the composer produced (placed or overset).
    lines: usize,
    /// Last placed baseline, pt below the frame's spread top.
    last_baseline_rel: f32,
    /// Right edge of the widest line's ink (trailing whitespace
    /// excluded), pt right of the frame's spread left edge.
    widest_right_rel: f32,
    /// Every line's baseline, pt below the frame's spread top, in
    /// ascending order. `VerticalBalanceColumns` reads this to find
    /// where the k-th line falls before anything is emitted.
    baselines: Vec<f32>,
}

pub(super) struct Measurer<'a> {
    document: &'a Document,
    options: &'a PipelineOptions<'a>,
    palette: &'a Graphic,
    color_ctx: ColorCtx<'a>,
    font_table: &'a FontTable,
    hyphenator: &'a paged_text::Hyphenator,
    /// One label for the scratch page a trial composes into.
    labels: Vec<String>,
}

impl<'a> Measurer<'a> {
    pub(super) fn new(
        document: &'a Document,
        options: &'a PipelineOptions<'a>,
        palette: &'a Graphic,
        color_ctx: ColorCtx<'a>,
        font_table: &'a FontTable,
        hyphenator: &'a paged_text::Hyphenator,
    ) -> Self {
        Self {
            document,
            options,
            palette,
            color_ctx,
            font_table,
            hyphenator,
            labels: vec!["1".to_string()],
        }
    }

    /// Fit every auto-sizing text frame on every spread. Keyed by the
    /// frame's `Self` id; frames whose fit equals their authored bounds
    /// get no entry.
    pub(super) fn fit_all(&self) -> HashMap<String, AutoSized> {
        let mut out = HashMap::new();
        for parsed in &self.document.spreads {
            let obstacles = next_column_obstacles(&parsed.spread);
            for frame in &parsed.spread.text_frames {
                let Some(id) = frame.self_id.as_deref() else {
                    continue;
                };
                if let Some(fitted) = self.fit_frame(frame, &obstacles) {
                    out.insert(id.to_string(), fitted);
                }
            }
        }
        out
    }

    fn fit_frame(&self, frame: &TextFrame, obstacles: &[Bounds]) -> Option<AutoSized> {
        let at = frame.auto_sizing?;
        if matches!(at, AutoSizingType::Off) {
            return None;
        }
        let story_id = frame.parent_story.as_deref()?;
        let story = self
            .document
            .stories
            .iter()
            .find(|s| s.self_id == story_id)?;
        if matches!(
            story.story.story_direction,
            Some(paged_model::StoryDirection::VerticalWritingDirection)
        ) {
            return None;
        }
        // Only the rectangular text panel is fitted; a polygon / oval
        // text frame keeps its authored outline (the painted shape is
        // the outline, not the AABB).
        if super::text_frame::frame_polygon_spread(frame).is_some()
            || super::text_frame::frame_shape_spread(frame).is_some()
        {
            return None;
        }
        let authored = frame.bounds;
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]); // top, left, bottom, right
        let authored_w = authored.width().max(0.0);
        let authored_h = authored.height().max(0.0);
        let min_w = frame.minimum_width_for_auto_sizing.unwrap_or(0.0).max(0.0);
        let min_h = if frame.use_minimum_height_for_auto_sizing == Some(true) {
            frame.minimum_height_for_auto_sizing.unwrap_or(0.0).max(0.0)
        } else {
            0.0
        };
        // InDesign's default reference point is the CENTRE (measured
        // 2026-09-06: a fresh frame reports CenterPoint, the exporter
        // omits the attribute for CenterPoint and writes every other
        // value, and Preferences.xml / [Normal Text Frame] both carry
        // CenterPoint) — not the TopLeftPoint the IDML docs suggest.
        let rp = frame.auto_sizing_reference_point.unwrap_or(RP::CenterPoint);

        let trial = |w: f32, h: f32| self.trial(frame, story, authored, w, h);

        let (mut grown_w, mut grown_h, mut text_dropped) = (authored_w, authored_h, false);
        match at {
            AutoSizingType::Off => return None,
            AutoSizingType::HeightOnly => {
                let t = trial(authored_w, UNBOUNDED_PT);
                if t.lines == 0 {
                    return None;
                }
                grown_h = (t.last_baseline_rel + insets[2]).max(min_h);
            }
            AutoSizingType::WidthOnly => {
                if let Some(stalled) = stalled_width(authored, frame.item_transform, rp, obstacles)
                {
                    grown_w = stalled;
                    text_dropped = true;
                } else {
                    let fits = |w: f32| {
                        let t = trial(w, authored_h);
                        t.lines > 0 && !t.overset
                    };
                    // Nothing fits even unbounded (the frame is too
                    // short for a single line): InDesign leaves the
                    // frame alone and reports the overset.
                    if !fits(UNBOUNDED_PT) {
                        return None;
                    }
                    let mut hi = authored_w.max(1.0);
                    while !fits(hi) {
                        hi *= 2.0;
                        if hi >= UNBOUNDED_PT {
                            hi = UNBOUNDED_PT;
                            break;
                        }
                    }
                    grown_w = bisect_min(0.0, hi, FIT_TOLERANCE_PT, fits).max(min_w);
                }
            }
            AutoSizingType::HeightAndWidth => {
                let unbounded = trial(UNBOUNDED_PT, UNBOUNDED_PT);
                if unbounded.lines == 0 {
                    return None;
                }
                // The narrowest column no line overflows: every line
                // is one unbreakable fragment.
                let composes = |w: f32| {
                    let t = trial(w, UNBOUNDED_PT);
                    t.lines > 0
                        && !t.overset
                        && t.widest_right_rel <= w - insets[3] + FIT_TOLERANCE_PT
                };
                let mut hi = unbounded.widest_right_rel + insets[3] + FIT_TOLERANCE_PT;
                while !composes(hi) {
                    hi *= 2.0;
                    if hi >= UNBOUNDED_PT {
                        hi = UNBOUNDED_PT;
                        break;
                    }
                }
                grown_w = bisect_min(0.0, hi, FIT_TOLERANCE_PT, composes).max(min_w);
                let t = trial(grown_w, UNBOUNDED_PT);
                grown_h = (t.last_baseline_rel + insets[2]).max(min_h);
            }
            AutoSizingType::HeightAndWidthProportionally => {
                if authored_w <= 0.0 || authored_h <= 0.0 {
                    return None;
                }
                let fits = |f: f32| {
                    let t = trial(authored_w * f, authored_h * f);
                    t.lines > 0 && !t.overset
                };
                if !fits(MAX_PROPORTIONAL_SCALE) {
                    return None;
                }
                let mut hi = 1.0;
                while !fits(hi) {
                    hi *= 2.0;
                    if hi >= MAX_PROPORTIONAL_SCALE {
                        hi = MAX_PROPORTIONAL_SCALE;
                        break;
                    }
                }
                let mut f = bisect_min(0.0, hi, FIT_TOLERANCE_PT / authored_w.max(1.0), fits);
                if min_w > 0.0 {
                    f = f.max(min_w / authored_w);
                }
                if min_h > 0.0 {
                    f = f.max(min_h / authored_h);
                }
                grown_w = authored_w * f;
                grown_h = authored_h * f;
            }
        }

        if !text_dropped
            && (grown_w - authored_w).abs() <= 0.01
            && (grown_h - authored_h).abs() <= 0.01
        {
            return None;
        }
        Some(AutoSized {
            bounds: anchor(authored, grown_w, grown_h, rp),
            text_dropped,
        })
    }

    /// Compose the frame's story at `w × h` (outer, pt) into a scratch
    /// page and read the emitter's verdict. The clone keeps everything
    /// but its bounds — insets, first-baseline rule, columns, the
    /// `ItemTransform` — so the trial composes exactly as the final
    /// layout will.
    /// Every line baseline this story produces at `width_pt`, in pt
    /// below the frame's spread top, in composition order.
    ///
    /// `VerticalBalanceColumns` needs to know where the k-th line falls
    /// before anything is emitted, and the auto-size trial already
    /// composes a story at an arbitrary measure — this is that, asked a
    /// different question. Composing at UNBOUNDED height is what makes
    /// the answer the story's own line list rather than one frame's
    /// worth of it.
    pub(super) fn line_baselines(
        &self,
        frame: &TextFrame,
        story: &paged_scene::ParsedStory,
        width_pt: f32,
    ) -> Vec<f32> {
        // One measure, unbounded in height, so the answer is the
        // STORY's line list rather than one frame's worth of it.
        let mut probe = frame.clone();
        probe.column_count = None;
        probe.column_gutter = None;
        self.trial(&probe, story, frame.bounds, width_pt, UNBOUNDED_PT)
            .baselines
    }

    fn trial(
        &self,
        frame: &TextFrame,
        story: &paged_scene::ParsedStory,
        authored: Bounds,
        w: f32,
        h: f32,
    ) -> Trial {
        let mut probe = frame.clone();
        // The emitter never oversets a frame whose mode grows height
        // (its lines keep placing past the bottom); the trial needs the
        // honest verdict for the size under test, so the probe composes
        // as a fixed frame.
        probe.auto_sizing = Some(AutoSizingType::Off);
        probe.bounds = Bounds {
            top: authored.top,
            left: authored.left,
            bottom: authored.top + h.max(0.0),
            right: authored.left + w.max(0.0),
        };
        let probe_spread = transform_bounds(probe.bounds, probe.item_transform);
        // One scratch page at the spread origin: page-local == spread
        // coords, so line records read straight against `probe_spread`.
        let mut pages = vec![BuiltPage {
            id: PageId::synthetic(usize::MAX, 0),
            width_pt: UNBOUNDED_PT,
            height_pt: UNBOUNDED_PT,
            spread_origin: (0.0, 0.0),
            spread_transform: Transform::IDENTITY,
            list: DisplayList::new(),
            layout_generation: 0,
            numbering_generation: 0,
            stats: PipelineStats::default(),
            story_layout: Vec::new(),
            footnotes: Vec::new(),
            diagnostics: Vec::new(),
            cell_rects: Vec::new(),
            resource_tiles_needed: Vec::new(),
        }];
        let mut stats = PipelineStats::default();
        let chain: Vec<&TextFrame> = vec![&probe];
        let mut emitter = StoryEmitter::new(
            self.document,
            self.options,
            self.palette,
            self.color_ctx,
            self.font_table,
            chain,
            vec![0],
            &self.labels,
            Some(self.hyphenator),
            &[],
            vec![&[]],
        )
        .with_optical_margin(
            story.story.optical_margin_alignment,
            story.story.optical_margin_size,
        )
        .with_story_id(&story.self_id)
        .with_page_count(1);
        for paragraph in &story.story.paragraphs {
            emitter.emit_paragraph(paragraph, &mut pages, &mut stats);
        }
        let overset = emitter
            .take_diagnostics()
            .iter()
            .any(|d| d.code == DiagnosticCode::OversetTextDropped);

        // Paragraph texts, for the trailing-whitespace test on clusters.
        let texts: Vec<String> = story
            .story
            .paragraphs
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect();
        let mut lines = 0usize;
        let mut last_baseline_rel = 0.0f32;
        let mut widest_right_rel = 0.0f32;
        let mut baselines: Vec<f32> = Vec::new();
        for line in pages[0]
            .story_layout
            .iter()
            .filter(|l| l.cell.is_none() && l.story_id == story.self_id)
        {
            lines += 1;
            let rel = line.baseline_y_pt - probe_spread.top;
            baselines.push(rel);
            last_baseline_rel = last_baseline_rel.max(rel);
            let text = texts.get(line.paragraph_idx as usize).map(|s| s.as_bytes());
            for cluster in &line.clusters {
                let blank = text
                    .and_then(|t| t.get(cluster.byte as usize))
                    .is_some_and(|b| b.is_ascii_whitespace());
                if blank {
                    continue;
                }
                widest_right_rel =
                    widest_right_rel.max(cluster.x_pt + cluster.advance_pt - probe_spread.left);
            }
        }
        baselines.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Trial {
            overset,
            lines,
            last_baseline_rel,
            widest_right_rel,
            baselines,
        }
    }
}

/// Smallest `x` in `[lo, hi]` for which `fits(x)` holds, assuming
/// `fits(hi)` and monotone `fits`. Converges to `tol`.
fn bisect_min(mut lo: f32, mut hi: f32, tol: f32, mut fits: impl FnMut(f32) -> bool) -> f32 {
    let mut guard = 0;
    while hi - lo > tol && guard < 64 {
        let mid = 0.5 * (lo + hi);
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
        guard += 1;
    }
    hi
}

/// Which fraction of a width change lands on the LEFT edge (the rest
/// goes right) for a reference point.
fn left_fraction(rp: RP) -> f32 {
    match rp {
        RP::TopLeftPoint | RP::CenterLeftPoint | RP::BottomLeftPoint => 0.0,
        RP::TopCenterPoint | RP::CenterPoint | RP::BottomCenterPoint => 0.5,
        RP::TopRightPoint | RP::CenterRightPoint | RP::BottomRightPoint => 1.0,
    }
}

/// Which fraction of a height change lands on the TOP edge.
fn top_fraction(rp: RP) -> f32 {
    match rp {
        RP::TopLeftPoint | RP::TopCenterPoint | RP::TopRightPoint => 0.0,
        RP::CenterLeftPoint | RP::CenterPoint | RP::CenterRightPoint => 0.5,
        RP::BottomLeftPoint | RP::BottomCenterPoint | RP::BottomRightPoint => 1.0,
    }
}

/// Place a `w × h` box against the authored bounds so the reference
/// point stays where it was. Deltas may be negative (a fit shrinks as
/// readily as it grows).
pub(super) fn anchor(authored: Bounds, w: f32, h: f32, rp: RP) -> Bounds {
    let dw = w - authored.width();
    let dh = h - authored.height();
    let lf = left_fraction(rp);
    let tf = top_fraction(rp);
    Bounds {
        left: authored.left - dw * lf,
        right: authored.right + dw * (1.0 - lf),
        top: authored.top - dh * tf,
        bottom: authored.bottom + dh * (1.0 - tf),
    }
}

/// Spread-coord AABBs (wrap offsets applied) of every item on the
/// spread whose text wrap is `NextColumnTextWrap`.
fn next_column_obstacles(spread: &paged_model::Spread) -> Vec<Bounds> {
    fn inflate(b: Bounds, m: Option<[f32; 6]>, wrap: paged_model::TextWrap) -> Option<Bounds> {
        if !matches!(wrap.mode, TextWrapMode::NextColumnTextWrap) {
            return None;
        }
        let [t, l, btm, r] = wrap.offsets;
        let inner = Bounds {
            top: b.top - t,
            left: b.left - l,
            bottom: b.bottom + btm,
            right: b.right + r,
        };
        Some(transform_bounds(inner, m))
    }
    let mut out = Vec::new();
    out.extend(spread.text_frames.iter().filter_map(|f| {
        f.text_wrap
            .and_then(|w| inflate(f.bounds, f.item_transform, w))
    }));
    out.extend(spread.rectangles.iter().filter_map(|r| {
        r.text_wrap
            .and_then(|w| inflate(r.bounds, r.item_transform, w))
    }));
    out.extend(spread.ovals.iter().filter_map(|o| {
        o.text_wrap
            .and_then(|w| inflate(o.bounds, o.item_transform, w))
    }));
    out.extend(spread.polygons.iter().filter_map(|p| {
        p.text_wrap
            .and_then(|w| inflate(p.bounds, p.item_transform, w))
    }));
    out.extend(spread.graphic_lines.iter().filter_map(|l| {
        l.text_wrap
            .and_then(|w| inflate(l.bounds, l.item_transform, w))
    }));
    out
}

/// The width a `WidthOnly` fit stalls at when a `NextColumnTextWrap`
/// obstacle sits in its unbounded growth path, or `None` when the path
/// is clear. Obstacles must share the frame's vertical band and lie
/// wholly beyond the edge that grows (one already overlapping the frame
/// is an ordinary wrap, not a stall). A centred frame grows both ways
/// and stops at the nearer obstacle.
fn stalled_width(
    authored: Bounds,
    m: Option<[f32; 6]>,
    rp: RP,
    obstacles: &[Bounds],
) -> Option<f32> {
    if obstacles.is_empty() {
        return None;
    }
    let fs = transform_bounds(authored, m);
    let in_band = |o: &Bounds| o.bottom > fs.top && o.top < fs.bottom;
    let nearest_right = obstacles
        .iter()
        .filter(|o| in_band(o) && o.left >= fs.right)
        .map(|o| o.left)
        .fold(None, |acc: Option<f32>, x| {
            Some(acc.map_or(x, |a| a.min(x)))
        });
    let nearest_left = obstacles
        .iter()
        .filter(|o| in_band(o) && o.right <= fs.left)
        .map(|o| o.right)
        .fold(None, |acc: Option<f32>, x| {
            Some(acc.map_or(x, |a| a.max(x)))
        });
    let lf = left_fraction(rp);
    let cx = 0.5 * (fs.left + fs.right);
    let width = if lf == 0.0 {
        nearest_right? - fs.left
    } else if lf == 1.0 {
        fs.right - nearest_left?
    } else {
        let half_right = nearest_right.map(|x| x - cx);
        let half_left = nearest_left.map(|x| cx - x);
        let half = match (half_right, half_left) {
            (Some(r), Some(l)) => r.min(l),
            (Some(r), None) => r,
            (None, Some(l)) => l,
            (None, None) => return None,
        };
        2.0 * half
    };
    (width > fs.width()).then_some(width)
}

#[cfg(test)]
mod unit {
    use super::*;

    fn b(top: f32, left: f32, bottom: f32, right: f32) -> Bounds {
        Bounds {
            top,
            left,
            bottom,
            right,
        }
    }

    #[test]
    fn a_centred_fit_stalls_at_twice_the_nearer_obstacle_distance() {
        // The annual's page-28 battery: frame at spread x −480..−350
        // (centre −415), NextColumn rectangle at 92..156 in the same
        // band → InDesign stretched the frame to 2 × (92 − (−415)).
        let frame = b(400.0, -480.0, 442.0, -350.0);
        let obstacle = b(408.0, 92.0, 448.0, 156.0);
        let w = stalled_width(frame, None, RP::CenterPoint, &[obstacle]).expect("stalls");
        assert!((w - 1014.0).abs() < 1e-3, "width {w}");
        // Left-anchored: grows right only, to the obstacle's left edge.
        let w = stalled_width(frame, None, RP::TopLeftPoint, &[obstacle]).expect("stalls");
        assert!((w - 572.0).abs() < 1e-3, "width {w}");
        // Right-anchored: grows left only — the obstacle is not in the path.
        assert!(stalled_width(frame, None, RP::TopRightPoint, &[obstacle]).is_none());
        // Out of the band: no stall.
        let above = b(300.0, 92.0, 340.0, 156.0);
        assert!(stalled_width(frame, None, RP::CenterPoint, &[above]).is_none());
    }

    #[test]
    fn anchoring_pins_the_reference_point_for_shrink_and_growth() {
        let authored = b(100.0, 60.0, 142.0, 190.0);
        let c = anchor(authored, 32.758, 42.0, RP::CenterPoint);
        assert!((c.left - 108.621).abs() < 1e-3 && (c.right - 141.379).abs() < 1e-3);
        let br = anchor(authored, 170.545, 55.099, RP::BottomRightPoint);
        assert!((br.right - 190.0).abs() < 1e-6 && (br.bottom - 142.0).abs() < 1e-6);
        assert!((br.left - 19.455).abs() < 1e-3 && (br.top - 86.901).abs() < 1e-3);
    }

    #[test]
    fn bisection_finds_the_smallest_fitting_value() {
        let w = bisect_min(0.0, 1000.0, 0.01, |x| x >= 223.0);
        assert!((w - 223.0).abs() <= 0.01, "{w}");
    }
}
