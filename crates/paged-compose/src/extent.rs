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

//! What a display command PAINTS, in page-space pt — and the pass that
//! sizes a transparency group's `bounds` to fit it.
//!
//! # The invariant
//!
//! `BeginBlendGroup { bounds }` is not a hint. Both rasterizers clip
//! the group's contents to that rect: the CPU through
//! `cpu::group_bounds_mask`, Vello by handing `bounds` to
//! `push_layer` (7751cda made the two agree). Anything a group paints
//! outside its own `bounds` is therefore *gone* — silently, with a
//! hard edge where the rect ends.
//!
//! The emitters cannot size `bounds` correctly on their own. They know
//! the frame's GEOMETRY, and they pad it by [`GROUP_AA_PAD_PT`] for
//! glyph antialiasing — but a drop shadow lands at
//! `geometry + offset + 3σ`, a centred stroke reaches half its weight
//! past the outline, and an outer glow reaches `3σ + spread`. Every one
//! of those overflows a geometry-sized rect. `paged-export-pdf` gives
//! each transparency-group form the media box
//! (`page::content_bbox`), so the exported PDF keeps that paint and the
//! raster does not — the two disagree about the same document.
//!
//! # The fix
//!
//! [`fit_transparency_group_bounds`] runs over the finished list and
//! grows each group's `bounds` to the union of what its contents
//! actually paint. Clipping to `bounds` then becomes *harmless*
//! instead of lossy, which keeps 7751cda's parity work intact — the
//! alternative, "stop clipping", cannot work on the CPU, where the
//! buffer physically IS that size, so not clipping loses the overflow
//! silently instead of sharply.
//!
//! Growing is safe by construction. The CPU composites a group with
//! `draw_pixmap` at INTEGER pixel offsets derived from `bounds`, so a
//! bigger buffer places every previously-covered pixel at exactly the
//! same phase; and a premultiplied α=0 source pixel is a no-op under
//! every separable blend mode. The one behaviour that does read the
//! buffer's full rect is the Q-05 paper-backdrop probe
//! (`cpu.rs`'s `snapshot_is_fully_transparent`), which asks whether the
//! PARENT is untouched under the whole buffer — a group that grows into
//! painted parent territory can stop qualifying for the
//! paper substitution. That only arises for a group nested inside
//! another group (the page pixmap is pre-filled with the background, so
//! its snapshot is never fully transparent), and where it does arise
//! the larger region is the more honest question to ask.
//!
//! # Extents, per command
//!
//! The numbers below mirror the CPU rasterizer's own scratch padding
//! (`paged-gpu/src/cpu.rs`) so the two cannot drift: where that file
//! allocates `3σ + 1` around a path's bbox and composites the whole
//! scratch, this file claims the same rect. The effects that stay
//! INSIDE the path claim only the path's bbox — they multiply their
//! output by the path-interior mask over the whole buffer
//! (`a = inside * … `), so no amount of blur moves them out.
//!
//! | command | extent |
//! |---|---|
//! | `FillPath` / `FillPathBlend` / `FillPathOverprint` | path bbox |
//! | `StrokePath` / `StrokePathOverprint` | bbox + half the weight (× the miter limit on a miter join) |
//! | `DropShadow` | bbox + shadow offset, + `3σ + 1` |
//! | `PathShadow` | as above with σ = `3.5 × blur_radius` (the glyph-shadow scale) |
//! | `Image` | the source pixel grid through `transform` |
//! | `OuterGlow` | bbox + `3σ + spread + 1` |
//! | `Feather` | bbox + `3 × width + 1` |
//! | `DirectionalFeather` | bbox + `3 × max edge width + 1` |
//! | `BevelEmboss` | bbox + `3 × soften` (0 in the common case) |
//! | `InnerShadow` / `InnerGlow` / `Satin` | bbox — masked to the path interior |
//! | `GradientFeather` | bbox — modulates alpha in place, adds no coverage |
//! | `PushClip` / `PopClip` / bracket markers | nothing |
//!
//! `BevelEmboss` is the odd one: its shading is interior-masked, but
//! `soften` blurs the shaded layer AFTERWARDS, so a nonzero soften does
//! reach outside. It is capped by the rasterizer's own `3 × size + 2`
//! scratch.

use std::collections::HashMap;

use crate::display_list::{
    DecodedImage, DisplayCommand, DisplayList, LineJoin, PathBuffer, PathId, PathSegment, Rect,
    Stroke, Transform,
};

/// Anti-aliasing headroom, in pt, kept around a fitted group's extent.
///
/// Same 0.5 pt the frame emitters have always padded their geometry by
/// (`pipeline::blend_shadow::push_blend_group`), for the same reason:
/// a shape's antialiased edge covers pixels just outside its exact
/// bbox, and the group's clip is antialiased too. Keeping it here means
/// a group whose contents match its geometry — the overwhelmingly
/// common case — fits to exactly the bounds it already had.
pub const GROUP_AA_PAD_PT: f32 = 0.5;

/// An axis-aligned page-space extent — a closed rect in pt, used only
/// for unions, intersections and containment. Kept internal; the
/// crate-facing shape is [`Rect`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Extent {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Extent {
    fn from_rect(r: Rect) -> Self {
        // Rects with negative w/h do occur (a mirrored ItemTransform
        // folded into geometry); normalise so min <= max.
        let (x0, x1) = if r.w >= 0.0 {
            (r.x, r.x + r.w)
        } else {
            (r.x + r.w, r.x)
        };
        let (y0, y1) = if r.h >= 0.0 {
            (r.y, r.y + r.h)
        } else {
            (r.y + r.h, r.y)
        };
        Extent {
            min_x: x0,
            min_y: y0,
            max_x: x1,
            max_y: y1,
        }
    }

    fn to_rect(self) -> Rect {
        Rect {
            x: self.min_x,
            y: self.min_y,
            w: self.max_x - self.min_x,
            h: self.max_y - self.min_y,
        }
    }

    fn union(self, other: Extent) -> Extent {
        Extent {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    fn intersect(self, other: Extent) -> Option<Extent> {
        let e = Extent {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        };
        (e.min_x <= e.max_x && e.min_y <= e.max_y).then_some(e)
    }

    fn inflate(self, pad: f32) -> Extent {
        Extent {
            min_x: self.min_x - pad,
            min_y: self.min_y - pad,
            max_x: self.max_x + pad,
            max_y: self.max_y + pad,
        }
    }

    fn translate(self, dx: f32, dy: f32) -> Extent {
        Extent {
            min_x: self.min_x + dx,
            min_y: self.min_y + dy,
            max_x: self.max_x + dx,
            max_y: self.max_y + dy,
        }
    }

    fn is_finite(self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
    }

    /// `self` covers `other`, allowing `eps` pt of slack. Used by the
    /// invariant check, where an exact float compare would trip on the
    /// rounding of the very union that produced the bounds.
    fn covers(self, other: Extent, eps: f32) -> bool {
        other.min_x >= self.min_x - eps
            && other.min_y >= self.min_y - eps
            && other.max_x <= self.max_x + eps
            && other.max_y <= self.max_y + eps
    }
}

fn union_opt(a: Option<Extent>, b: Option<Extent>) -> Option<Extent> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Map a local-space extent through an affine. Transforms the four
/// corners and re-bounds — exact for an axis-aligned map, conservative
/// (the rotated rect's AABB) otherwise, which is what a bounds rect
/// wants.
fn transform_extent(e: Extent, t: &Transform) -> Extent {
    let pts = [
        t.apply(e.min_x, e.min_y),
        t.apply(e.max_x, e.min_y),
        t.apply(e.max_x, e.max_y),
        t.apply(e.min_x, e.max_y),
    ];
    let mut out = Extent {
        min_x: pts[0].0,
        min_y: pts[0].1,
        max_x: pts[0].0,
        max_y: pts[0].1,
    };
    for (x, y) in pts.iter().skip(1) {
        out.min_x = out.min_x.min(*x);
        out.min_y = out.min_y.min(*y);
        out.max_x = out.max_x.max(*x);
        out.max_y = out.max_y.max(*y);
    }
    out
}

/// Local-space bbox per interned path, memoised.
///
/// Paths are interned (one `PathData` per distinct glyph outline), and
/// a text page references the same handful of outlines thousands of
/// times, so the segment walk happens once per outline rather than once
/// per command.
#[derive(Default)]
struct PathBboxes {
    by_id: HashMap<PathId, Option<Extent>>,
}

impl PathBboxes {
    fn local(&mut self, paths: &PathBuffer, id: PathId) -> Option<Extent> {
        if let Some(hit) = self.by_id.get(&id) {
            return *hit;
        }
        let computed = paths.get(id).and_then(path_local_bbox);
        self.by_id.insert(id, computed);
        computed
    }

    fn page(&mut self, paths: &PathBuffer, id: PathId, t: &Transform) -> Option<Extent> {
        let local = self.local(paths, id)?;
        let mapped = transform_extent(local, t);
        mapped.is_finite().then_some(mapped)
    }
}

/// Bbox of a path in its own coordinates. Bezier control points are
/// included rather than solved for: the convex hull of a cubic's
/// control polygon contains the curve, so this over-covers slightly and
/// never under-covers, which is the safe direction for a buffer bound.
fn path_local_bbox(path: &crate::display_list::PathData) -> Option<Extent> {
    let mut out: Option<Extent> = None;
    let mut add = |x: f32, y: f32| {
        let p = Extent {
            min_x: x,
            min_y: y,
            max_x: x,
            max_y: y,
        };
        out = Some(match out {
            Some(e) => e.union(p),
            None => p,
        });
    };
    for seg in &path.segments {
        match *seg {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => add(x, y),
            PathSegment::QuadTo { cx, cy, x, y } => {
                add(cx, cy);
                add(x, y);
            }
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                add(cx1, cy1);
                add(cx2, cy2);
                add(x, y);
            }
            PathSegment::Close => {}
        }
    }
    out.filter(|e| e.is_finite())
}

/// How far a stroke reaches outside the path it follows, in pt.
///
/// `Stroke::width` is a page-space pt width applied to the already-
/// transformed path (see `DisplayCommand::StrokePath`'s contract), so
/// this is a straight pad on the page-space bbox. A miter join spikes
/// further than half the weight; the miter limit is exactly the cap on
/// how much further.
fn stroke_outset(stroke: &Stroke) -> f32 {
    let half = stroke.width.max(0.0) * 0.5;
    match stroke.join {
        LineJoin::Miter => half * stroke.miter_limit.max(1.0),
        _ => half,
    }
}

/// The page-space rect a single command can put paint in, or `None`
/// when it paints nothing (clip pushes, bracket markers, a command
/// whose path or image is missing).
///
/// Group brackets return `None` here — the walker handles their nesting
/// itself, because a group's contribution to its parent is its own
/// (fitted) bounds, not its contents'.
fn painted_extent(
    cmd: &DisplayCommand,
    paths: &PathBuffer,
    images: &[DecodedImage],
    cache: &mut PathBboxes,
) -> Option<Extent> {
    use DisplayCommand as C;
    match cmd {
        C::FillPath {
            path_id, transform, ..
        }
        | C::FillPathBlend {
            path_id, transform, ..
        }
        | C::FillPathOverprint {
            path_id, transform, ..
        } => cache.page(paths, *path_id, transform),

        C::StrokePath {
            path_id,
            transform,
            stroke,
            ..
        }
        | C::StrokePathOverprint {
            path_id,
            transform,
            stroke,
            ..
        } => cache
            .page(paths, *path_id, transform)
            .map(|e| e.inflate(stroke_outset(stroke))),

        // The rasterizer translates the stamp by `shadow.offset_*` and
        // pads its scratch by `3σ + 1` (cpu.rs, the DropShadow arm).
        // `PathShadow` shares that arm with a 3.5× σ scale — glyph
        // shadows want a wider Gaussian so neighbouring stamps' tails
        // meet inside the shadow group.
        C::DropShadow {
            path_id,
            transform,
            shadow,
        }
        | C::PathShadow {
            path_id,
            transform,
            shadow,
        } => {
            let sigma_scale = if matches!(cmd, C::PathShadow { .. }) {
                3.5
            } else {
                1.0
            };
            let pad = 3.0 * shadow.blur_radius.max(0.0) * sigma_scale + 1.0;
            cache
                .page(paths, *path_id, transform)
                .map(|e| e.translate(shadow.offset_x, shadow.offset_y).inflate(pad))
        }

        C::Image {
            image_id,
            transform,
        } => {
            let img = images.get(image_id.0 as usize)?;
            if img.width == 0 || img.height == 0 {
                return None;
            }
            let src = Extent {
                min_x: 0.0,
                min_y: 0.0,
                max_x: img.width as f32,
                max_y: img.height as f32,
            };
            let mapped = transform_extent(src, transform);
            mapped.is_finite().then_some(mapped)
        }

        // Halo outside the path: `3σ + |spread| + 1` (render_outer_glow).
        C::OuterGlow {
            path_id,
            transform,
            params,
        } => {
            let pad = 3.0 * params.blur_radius.max(0.0) + params.spread.abs() + 1.0;
            cache
                .page(paths, *path_id, transform)
                .map(|e| e.inflate(pad))
        }

        // The feather family blurs the path's interior mask and
        // composites the RESULT unmasked, so the soft edge really does
        // spill outside. A negative choke lifts the whole scratch off
        // zero as well; both are inside the rasterizer's own pad.
        C::Feather {
            path_id,
            transform,
            params,
        } => {
            let pad = params.width.abs() * 3.0 + 1.0;
            cache
                .page(paths, *path_id, transform)
                .map(|e| e.inflate(pad))
        }
        C::DirectionalFeather {
            path_id,
            transform,
            params,
        } => {
            let max_w = params
                .left_width
                .max(params.right_width)
                .max(params.top_width)
                .max(params.bottom_width)
                .max(0.0);
            let pad = max_w * 3.0 + 1.0;
            cache
                .page(paths, *path_id, transform)
                .map(|e| e.inflate(pad))
        }

        // Interior-masked: `a = inside * …` over the whole scratch, so
        // however wide the blur, nothing survives outside the path.
        C::InnerShadow {
            path_id, transform, ..
        }
        | C::InnerGlow {
            path_id, transform, ..
        }
        | C::Satin {
            path_id, transform, ..
        } => cache.page(paths, *path_id, transform),

        // Interior-masked too — except `soften`, which blurs the shaded
        // layer after the mask. Capped by the scratch the rasterizer
        // allocated (`3 × size + 2`).
        C::BevelEmboss {
            path_id,
            transform,
            params,
        } => {
            let scratch = 3.0 * params.size.max(0.0) + 2.0;
            let pad = (3.0 * params.soften.max(0.0)).min(scratch);
            cache
                .page(paths, *path_id, transform)
                .map(|e| e.inflate(pad))
        }

        // A multiplicative alpha mask over what is already in the
        // buffer, applied only where the path covers. It cannot create
        // coverage outside the path.
        C::GradientFeather {
            path_id, transform, ..
        } => cache.page(paths, *path_id, transform),

        // No paint of their own.
        C::PushClip { .. }
        | C::PopClip(_)
        | C::BeginBlendGroup { .. }
        | C::EndBlendGroup(_)
        | C::PushLayer { .. }
        | C::PopLayer(_)
        | C::BeginSoftMask { .. }
        | C::BeginMaskedContent(_)
        | C::EndSoftMask(_) => None,
    }
}

/// One transparency-group bracket, measured.
#[derive(Clone, Copy, Debug)]
pub struct GroupFit {
    /// Index of the opening command in `list.commands`.
    pub index: usize,
    /// The `bounds` the emitter declared.
    pub declared: Rect,
    /// The rect its contents need — `None` when the group paints
    /// nothing at all.
    pub required: Option<Rect>,
}

/// What kind of bracket is open, and what it does to its contents on
/// the way out.
#[derive(Clone, Copy)]
enum OpenKind {
    /// `BeginBlendGroup` — `bounds` is the group's clip in both back
    /// ends.
    Group,
    /// `PushLayer` — same, plus a Gaussian that carries paint `3σ`
    /// further out when it composites.
    Layer { sigma_pt: f32 },
    /// The artwork half of a `BeginSoftMask` bracket. It paints into a
    /// mask buffer that becomes a clip, never onto the parent, so it
    /// contributes nothing.
    SoftMaskArtwork,
}

struct Open {
    index: usize,
    kind: OpenKind,
    declared: Rect,
    acc: Option<Extent>,
    clip_depth: usize,
}

/// Walk `commands`, pairing every transparency-group bracket with the
/// extent its contents paint.
///
/// Clips are tracked and intersected in, so a group whose content is
/// clipped away doesn't grow for it. A nested group contributes its own
/// FITTED bounds to its parent rather than its raw contents — the
/// parent only ever receives what the child's own clip lets through.
fn measure_groups(
    commands: &[DisplayCommand],
    paths: &PathBuffer,
    images: &[DecodedImage],
) -> Vec<GroupFit> {
    let mut cache = PathBboxes::default();
    let mut open: Vec<Open> = Vec::new();
    // Running intersection of the active clips; `None` entries mean
    // "unknown, don't restrict" (a clip whose path we couldn't read).
    let mut clips: Vec<Option<Extent>> = Vec::new();
    let mut out: Vec<GroupFit> = Vec::new();

    // Contribute `e` to the innermost open bracket, after clipping.
    fn contribute(open: &mut [Open], clips: &[Option<Extent>], e: Option<Extent>) {
        let Some(mut e) = e else { return };
        if !e.is_finite() {
            return;
        }
        for c in clips.iter().flatten() {
            match e.intersect(*c) {
                Some(next) => e = next,
                None => return, // fully clipped away
            }
        }
        if let Some(top) = open.last_mut() {
            if matches!(top.kind, OpenKind::SoftMaskArtwork) {
                return;
            }
            top.acc = union_opt(top.acc, Some(e));
        }
    }

    for (i, cmd) in commands.iter().enumerate() {
        match cmd {
            DisplayCommand::PushClip {
                path_id, transform, ..
            } => {
                let e = cache.page(paths, *path_id, transform);
                clips.push(e);
            }
            DisplayCommand::PopClip(_) => {
                clips.pop();
            }
            DisplayCommand::BeginBlendGroup { bounds, .. } => {
                open.push(Open {
                    index: i,
                    kind: OpenKind::Group,
                    declared: *bounds,
                    acc: None,
                    clip_depth: clips.len(),
                });
            }
            DisplayCommand::PushLayer { bounds, effect, .. } => {
                open.push(Open {
                    index: i,
                    kind: OpenKind::Layer {
                        sigma_pt: effect.sigma_pt(),
                    },
                    declared: *bounds,
                    acc: None,
                    clip_depth: clips.len(),
                });
            }
            // Both closers share one rasterizer arm ("pop whatever is on
            // top"); mirror that rather than matching marker kinds, so a
            // mismatched pair degrades the same way here as there.
            DisplayCommand::EndBlendGroup(_) | DisplayCommand::PopLayer(_) => {
                let Some(frame) = open.pop() else { continue };
                clips.truncate(frame.clip_depth);
                if matches!(frame.kind, OpenKind::SoftMaskArtwork) {
                    // Malformed: an `EndBlendGroup` closing a soft-mask
                    // capture. The rasterizer composites it as an
                    // ordinary group rather than panicking; here it has
                    // no `bounds` to fit and nothing to report.
                    continue;
                }
                let fitted = fit_bounds(frame.declared, frame.acc);
                out.push(GroupFit {
                    index: frame.index,
                    declared: frame.declared,
                    required: frame.acc.map(Extent::to_rect),
                });
                // The parent receives the group's own extent, padded by
                // the blur a `PushLayer` applies at composite time. Same
                // `3σ + 1` the CPU allocates the layer buffer with, so
                // the whole kernel tail is accounted for.
                let mut up = Extent::from_rect(fitted);
                if let OpenKind::Layer { sigma_pt } = frame.kind {
                    if sigma_pt > 0.0 {
                        up = up.inflate(3.0 * sigma_pt + 1.0);
                    }
                }
                contribute(&mut open, &clips, Some(up));
            }
            DisplayCommand::BeginSoftMask { .. } => {
                open.push(Open {
                    index: i,
                    kind: OpenKind::SoftMaskArtwork,
                    declared: Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    },
                    acc: None,
                    clip_depth: clips.len(),
                });
            }
            DisplayCommand::BeginMaskedContent(_) => {
                // The artwork is complete and becomes a clip. We don't
                // model the mask's coverage (an inverted mask shows
                // content the artwork never touched), so push an
                // unrestricted entry — it only exists to keep the clip
                // stack balanced against `EndSoftMask`, which the
                // rasterizer pops as a clip.
                if matches!(open.last().map(|o| o.kind), Some(OpenKind::SoftMaskArtwork)) {
                    let frame = open.pop().expect("checked above");
                    clips.truncate(frame.clip_depth);
                }
                clips.push(None);
            }
            DisplayCommand::EndSoftMask(_) => {
                clips.pop();
            }
            other => {
                let e = painted_extent(other, paths, images, &mut cache);
                contribute(&mut open, &clips, e);
            }
        }
    }

    // Unbalanced brackets: close what's still open, innermost first, so
    // the invariant check still sees them.
    while let Some(frame) = open.pop() {
        if matches!(frame.kind, OpenKind::SoftMaskArtwork) {
            continue;
        }
        out.push(GroupFit {
            index: frame.index,
            declared: frame.declared,
            required: frame.acc.map(Extent::to_rect),
        });
    }
    out
}

/// The bounds a group should carry: never smaller than what the emitter
/// declared, and never smaller than what it paints plus the antialias
/// headroom.
fn fit_bounds(declared: Rect, painted: Option<Extent>) -> Rect {
    let Some(painted) = painted else {
        return declared;
    };
    if !painted.is_finite() {
        return declared;
    }
    Extent::from_rect(declared)
        .union(painted.inflate(GROUP_AA_PAD_PT))
        .to_rect()
}

/// Grow every transparency group's `bounds` to cover what its contents
/// paint. Idempotent; a group whose contents already fit is untouched.
///
/// Run this once, on the finished list, after every pass that may
/// insert or move commands — the brackets themselves are spliced in
/// post-hoc by several passes (frame blend groups, glyph-range
/// brackets, the glyph-shadow wrapper, IDML `<Group>` transparency),
/// and each one sizes `bounds` from geometry it has to hand.
pub fn fit_transparency_group_bounds(list: &mut DisplayList) {
    let fits = measure_groups(&list.commands, &list.paths, &list.images);
    for fit in fits {
        let fitted = fit_bounds(fit.declared, fit.required.map(Extent::from_rect));
        if fitted == fit.declared {
            continue;
        }
        match &mut list.commands[fit.index] {
            DisplayCommand::BeginBlendGroup { bounds, .. }
            | DisplayCommand::PushLayer { bounds, .. } => *bounds = fitted,
            _ => {}
        }
    }
}

/// The first transparency group in `list` that paints outside its own
/// `bounds`, or `None` when the invariant holds.
///
/// This is the check behind the `debug_assert!` at the end of the
/// build: both rasterizers CLIP to `bounds`, so a violation is paint
/// that will be cut, silently, with a hard edge — the exact defect
/// [`fit_transparency_group_bounds`] exists to prevent. Finding one
/// means a bracket was emitted (or moved) after the fit pass ran.
pub fn transparency_group_overflow(list: &DisplayList) -> Option<GroupFit> {
    const EPS: f32 = 1e-3;
    measure_groups(&list.commands, &list.paths, &list.images)
        .into_iter()
        .find(|fit| match fit.required {
            Some(req) => !Extent::from_rect(fit.declared).covers(Extent::from_rect(req), EPS),
            None => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_list::{BlendMode, Color, DropShadow, Paint, PathData};

    fn unit_rect(list: &mut DisplayList) -> PathId {
        list.paths.push_anon(PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 1.0, y: 0.0 },
                PathSegment::LineTo { x: 1.0, y: 1.0 },
                PathSegment::LineTo { x: 0.0, y: 1.0 },
                PathSegment::Close,
            ],
        })
    }

    fn begin(bounds: Rect) -> DisplayCommand {
        DisplayCommand::BeginBlendGroup {
            bounds,
            blend_mode: BlendMode::Multiply,
            opacity: 0.5,
            transform: Transform::IDENTITY,
        }
    }

    fn group_bounds(list: &DisplayList, at: usize) -> Rect {
        match &list.commands[at] {
            DisplayCommand::BeginBlendGroup { bounds, .. } => *bounds,
            other => panic!("expected BeginBlendGroup at {at}, got {other:?}"),
        }
    }

    /// The reported defect, in miniature: a 100×60 rect at (20, 10)
    /// with a drop shadow 8 pt down-right and 6 pt of blur. The
    /// emitter's geometry+0.5 pt bounds cut the shadow dead; the fit
    /// pass has to reach `offset + 3σ + 1` past the rect.
    #[test]
    fn a_drop_shadow_grows_its_group_past_the_frame_geometry() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let geom = Rect {
            x: 20.0,
            y: 10.0,
            w: 100.0,
            h: 60.0,
        };
        let declared = Rect {
            x: geom.x - 0.5,
            y: geom.y - 0.5,
            w: geom.w + 1.0,
            h: geom.h + 1.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::DropShadow {
            path_id: rect,
            transform: Transform::for_rect_in(geom, Transform::IDENTITY),
            shadow: DropShadow {
                offset_x: 8.0,
                offset_y: 8.0,
                blur_radius: 6.0,
                color: Color::BLACK,
                opacity: 0.75,
            },
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));

        assert!(
            transparency_group_overflow(&list).is_some(),
            "the un-fitted group must be reported as overflowing"
        );
        fit_transparency_group_bounds(&mut list);
        let b = group_bounds(&list, 0);
        // 3σ + 1 = 19 pt of stamp pad, ±8 pt of offset, +0.5 AA.
        let pad = 3.0 * 6.0 + 1.0;
        assert!(
            b.x <= geom.x - pad + 8.0 && b.y <= geom.y - pad + 8.0,
            "shadow's leading edge must be inside the group: {b:?}"
        );
        assert!(
            b.x + b.w >= geom.x + geom.w + 8.0 + pad && b.y + b.h >= geom.y + geom.h + 8.0 + pad,
            "shadow's trailing edge must be inside the group: {b:?}"
        );
        assert!(transparency_group_overflow(&list).is_none());
    }

    /// A group whose contents match its geometry keeps the bounds it
    /// was given — the fit must be a no-op on the common case, or every
    /// page in the corpus moves.
    #[test]
    fn a_group_that_already_fits_is_untouched() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let geom = Rect {
            x: 20.0,
            y: 10.0,
            w: 100.0,
            h: 60.0,
        };
        let declared = Rect {
            x: geom.x - 0.5,
            y: geom.y - 0.5,
            w: geom.w + 1.0,
            h: geom.h + 1.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            transform: Transform::for_rect_in(geom, Transform::IDENTITY),
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        assert_eq!(group_bounds(&list, 0), declared);
        assert!(transparency_group_overflow(&list).is_none());
    }

    /// A centred stroke puts half its weight outside the outline. The
    /// 0.5 pt AA pad covers a 1 pt hairline and nothing wider — a 6 pt
    /// stroke overflows by 3 pt and has to grow the group.
    #[test]
    fn a_wide_stroke_grows_its_group_by_half_the_weight() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let geom = Rect {
            x: 10.0,
            y: 10.0,
            w: 40.0,
            h: 40.0,
        };
        let declared = Rect {
            x: 9.5,
            y: 9.5,
            w: 41.0,
            h: 41.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::StrokePath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            stroke: Stroke {
                join: LineJoin::Round,
                ..Stroke::new(6.0)
            },
            transform: Transform::for_rect_in(geom, Transform::IDENTITY),
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        let b = group_bounds(&list, 0);
        assert!(b.x <= 7.0 && b.y <= 7.0, "half the 6 pt weight: {b:?}");
        assert!(
            b.x + b.w >= 53.0 && b.y + b.h >= 53.0,
            "half the 6 pt weight: {b:?}"
        );
    }

    /// An interior-masked effect must NOT grow the group, however wide
    /// its blur — `render_inner_shadow` multiplies by the path-interior
    /// mask over the whole scratch, so nothing it paints leaves the
    /// path.
    #[test]
    fn an_inner_shadow_does_not_grow_its_group() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let geom = Rect {
            x: 10.0,
            y: 10.0,
            w: 40.0,
            h: 40.0,
        };
        let declared = Rect {
            x: 9.5,
            y: 9.5,
            w: 41.0,
            h: 41.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::InnerShadow {
            path_id: rect,
            transform: Transform::for_rect_in(geom, Transform::IDENTITY),
            params: crate::display_list::InnerShadow {
                blur_radius: 30.0,
                offset_x: 12.0,
                offset_y: 12.0,
                ..crate::display_list::InnerShadow::default_soft()
            },
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        assert_eq!(group_bounds(&list, 0), declared);
    }

    /// An outer glow is the opposite: it paints `3σ + spread` outside.
    #[test]
    fn an_outer_glow_grows_its_group() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let geom = Rect {
            x: 50.0,
            y: 50.0,
            w: 20.0,
            h: 20.0,
        };
        list.commands.push(begin(Rect {
            x: 49.5,
            y: 49.5,
            w: 21.0,
            h: 21.0,
        }));
        list.commands.push(DisplayCommand::OuterGlow {
            path_id: rect,
            transform: Transform::for_rect_in(geom, Transform::IDENTITY),
            params: crate::display_list::OuterGlow {
                blur_radius: 6.0,
                spread: 3.0,
                ..crate::display_list::OuterGlow::default_soft()
            },
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        let b = group_bounds(&list, 0);
        // 3σ + spread + 1 = 22 pt.
        assert!(b.x <= 50.0 - 22.0 && b.x + b.w >= 70.0 + 22.0, "{b:?}");
    }

    /// Content clipped away doesn't grow the group — the clip is what
    /// the rasterizer honours, so growing for paint it discards would
    /// only inflate the buffer.
    #[test]
    fn a_clip_bounds_the_growth() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let clip_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 30.0,
        };
        let declared = Rect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 30.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::PushClip {
            path_id: rect,
            transform: Transform::for_rect_in(clip_rect, Transform::IDENTITY),
        });
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            transform: Transform::for_rect_in(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 400.0,
                    h: 400.0,
                },
                Transform::IDENTITY,
            ),
        });
        list.commands
            .push(DisplayCommand::PopClip(Transform::IDENTITY));
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        let b = group_bounds(&list, 0);
        assert!(
            b.w <= 31.0 && b.h <= 31.0,
            "clipped-away paint must not grow the group: {b:?}"
        );
    }

    /// A nested group contributes its OWN bounds to its parent, not its
    /// contents' — the child's clip is what reaches the parent.
    #[test]
    fn a_nested_group_contributes_its_own_bounds() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let outer = Rect {
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 50.0,
        };
        let inner = Rect {
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 20.0,
        };
        list.commands.push(begin(outer));
        list.commands.push(begin(inner));
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            transform: Transform::for_rect_in(inner, Transform::IDENTITY),
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        assert_eq!(group_bounds(&list, 0), outer, "outer already covers inner");
        // The inner group's own declared bounds carried no AA pad, so
        // fitting adds one — but only that, not the parent's extent.
        assert_eq!(
            group_bounds(&list, 1),
            Rect {
                x: 9.5,
                y: 9.5,
                w: 21.0,
                h: 21.0
            }
        );
    }

    /// The fit is idempotent — running it twice must not creep the
    /// bounds outward by another AA pad each time.
    #[test]
    fn fitting_twice_changes_nothing() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        list.commands.push(begin(Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        }));
        list.commands.push(DisplayCommand::DropShadow {
            path_id: rect,
            transform: Transform::for_rect_in(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                },
                Transform::IDENTITY,
            ),
            shadow: DropShadow::default_soft(),
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        let once = group_bounds(&list, 0);
        fit_transparency_group_bounds(&mut list);
        assert_eq!(group_bounds(&list, 0), once);
    }

    /// A soft mask's ARTWORK paints into a mask buffer, never onto the
    /// parent, so it must not grow the enclosing group.
    #[test]
    fn soft_mask_artwork_does_not_grow_the_enclosing_group() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let declared = Rect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 20.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::BeginSoftMask {
            mask_type: crate::display_list::SoftMaskType::Luminosity,
            invert: false,
            transform: Transform::IDENTITY,
        });
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            transform: Transform::for_rect_in(
                Rect {
                    x: -500.0,
                    y: -500.0,
                    w: 1000.0,
                    h: 1000.0,
                },
                Transform::IDENTITY,
            ),
        });
        list.commands
            .push(DisplayCommand::BeginMaskedContent(Transform::IDENTITY));
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            transform: Transform::for_rect_in(declared, Transform::IDENTITY),
        });
        list.commands
            .push(DisplayCommand::EndSoftMask(Transform::IDENTITY));
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        // The masked content fills exactly `declared`, so the group
        // grows by the AA pad and nothing else — emphatically not by
        // the 1000 pt mask artwork.
        let b = group_bounds(&list, 0);
        assert!(
            b.w <= declared.w + 2.0 * GROUP_AA_PAD_PT + 1e-4,
            "mask artwork must not grow the group: {b:?}"
        );
    }

    /// A degenerate transform must not poison the bounds with NaN.
    #[test]
    fn a_non_finite_transform_leaves_the_bounds_alone() {
        let mut list = DisplayList::new();
        let rect = unit_rect(&mut list);
        let declared = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        list.commands.push(begin(declared));
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect,
            paint: Paint::Solid(Color::BLACK),
            transform: Transform([f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]),
        });
        list.commands
            .push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        fit_transparency_group_bounds(&mut list);
        assert_eq!(group_bounds(&list, 0), declared);
    }
}
