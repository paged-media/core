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

//! Spread_*.xml parser.
//!
//! Extracts page bounds and text-frame geometry from a Spread. This is
//! the minimal schema slice needed to know *where* text goes on the
//! page — a TextFrame's bounding rectangle becomes a column width for
//! the composer.
//!
//! Coverage:
//! - `<Page GeometricBounds="...">` — one entry per page.
//! - `<TextFrame ParentStory="..." GeometricBounds="..." ItemTransform="...">`
//!   at spread level. Text frames nested inside `<Group>` are
//!   intentionally out of scope for now; a warning surfaces via the
//!   parse result counters so higher layers can detect loss.
//!
//! GeometricBounds is `y1 x1 y2 x2` in points (IDML convention:
//! y-axis grows downward from page origin).

use quick_xml::events::Event;

use crate::util::{attr, parse_f, parse_tint_attr};
use crate::ParseError;

// N5 — these pure model types now live in `paged-model`; re-exported here
// so `idml_import::*` and every dependent are unchanged.
pub use paged_model::{
    ArrowheadType, CornerOption, CornerSpec, FrameRef, Group, GroupTransparency, GuideOrientation,
    MarginPreference, Page, RulerGuide,
};

pub use paged_model::Spread;

// N5 — these pure model types now live in `paged-model`; re-exported here
// so `idml_import::*` and every dependent are unchanged.
pub use paged_model::{
    BevelEmbossParams, ClippingPathSettings, ContourOptionType, DirectionalFeatherParams,
    FeatherParams, FrameEffects, FrameFittingOption, GradientFeatherParams, GradientFeatherStop,
    GraphicLine, InnerGlowParams, InnerShadowParams, OuterGlowParams, Oval, PathAnchor, Polygon,
    Rectangle, SatinParams, TextFrame, TextPath, TextWrap, TextWrapMode,
};

// N5 — these pure model types now live in `paged-model`; re-exported here
// so `idml_import::*` and every dependent are unchanged.
pub use paged_model::{
    AutoSizingReferencePoint, AutoSizingType, ClippingType, DropShadowSetting, FirstBaselineOffset,
    ImageMetadata, VerticalJustification,
};

// N5 — `Bounds` now lives in `paged-model` (the Paged-owned model); re-exported
// here so `idml_import::Bounds` and every dependent are unchanged.
pub use paged_model::Bounds;

/// Identifies the most recently opened shape element so child
/// elements (DropShadowSetting, TextFramePreference, PathPointType,
/// Image, Link) can attach to the right frame.
#[derive(Debug, Clone, Copy)]
enum CurrentFrameKind {
    Text(usize),
    Rect(usize),
    Oval(usize),
    Line(usize),
    Polygon(usize),
}

/// Per-`<Group>` parser state. Accumulates the group's child page
/// items and the transparency block as each fires; finalised into a
/// `Group` record on the closing `</Group>` tag.
struct GroupBuilder {
    self_id: Option<String>,
    item_transform: Option<[f32; 6]>,
    members: Vec<FrameRef>,
    transparency: GroupTransparency,
    /// Depth counter for nested `<StrokeTransparencySetting>` /
    /// `<ContentTransparencySetting>` containers seen *while no inner
    /// page-item is open*. Routes child `<DropShadowSetting>` blocks
    /// to the right place: stroke-only / content-only shadows attached
    /// to a Group don't map onto our model and are skipped.
    stroke_transparency_depth: u32,
    content_transparency_depth: u32,
}

/// Per-frame parser state held while a shape element is open.
/// Tracks whether the bounds came from a `GeometricBounds` attribute
/// (the legacy synthetic-IDML shape) or need to be derived from the
/// frame's `<PathGeometry>` (the InDesign-export shape — the format
/// real-world IDMLs use almost exclusively).
struct CurrentFrame {
    kind: CurrentFrameKind,
    /// True if the open tag had no `GeometricBounds` — bounds must
    /// then come from `<PathPointType Anchor="...">` children.
    needs_bounds: bool,
    /// Path-point anchors accumulated while the frame is open.
    /// Always collected for Polygons (so the renderer can rasterise
    /// the curved path); collected for the other shapes only when
    /// `needs_bounds` is true so we can derive an AABB on close.
    anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `anchors` — one per
    /// `<GeometryPathType>` opening tag while the shape is open.
    /// Allows the renderer to lift compound paths (square-with-hole
    /// etc.) into multiple `MoveTo`/`Close` segments rather than
    /// joining them into one broken polyline.
    subpath_starts: Vec<usize>,
    /// Parallel to `subpath_starts`: the open/closed flag harvested
    /// from each `<GeometryPathType PathOpen="...">` (P-15).
    subpath_open: Vec<bool>,
    /// True for Polygons even when `needs_bounds` is false, so the
    /// emitter still gets the curved-path data.
    keep_anchors: bool,
    /// True while a `<TextWrapPreference>` block is open, so the
    /// child `<TextWrapOffset>` knows to write back to the current
    /// shape.
    in_text_wrap: bool,
    /// Depth counter for nested `<StrokeTransparencySetting>`
    /// containers. When > 0, child `<DropShadowSetting>` blocks
    /// describe stroke-only shadows — captured into
    /// `stroke_drop_shadow` on the shape so the renderer can emit
    /// them only when the stroke is actually visible.
    stroke_transparency_depth: u32,
    /// Depth counter for nested `<ContentTransparencySetting>`
    /// containers. When > 0, child `<DropShadowSetting>` blocks
    /// describe content-only shadows that don't map onto our
    /// single-shadow-per-frame model and are skipped.
    content_transparency_depth: u32,
    /// W1.21: depth inside a nested `<Image>` (and `<EPSImage>` etc.).
    /// While > 0 the image's own `<PathGeometry>` is NOT the frame's
    /// outline, so geometry events must not pollute `anchors`
    /// (otherwise a Polygon host would absorb the image's 4-corner box
    /// as part of its silhouette). Clip-path geometry is routed by the
    /// `clip` builder instead.
    in_image_depth: u32,
    /// W1.21: the in-progress `<ClippingPathSettings>` of the nested
    /// image, if one was opened. Geometry events route into its
    /// `clip_anchors` while `in_clipping_path` holds; the whole record
    /// is written onto the host shape's `image_clip` at frame close.
    clip: Option<ClippingPathSettings>,
    /// W1.21: true between `<ClippingPathSettings>` and its end tag, so
    /// `<GeometryPathType>` / `<PathPointType>` children feed the clip
    /// path rather than the frame outline.
    in_clipping_path: bool,
}

/// Read whatever `text_wrap.offsets` has already been recorded on the
/// current shape, defaulting to all zeros.
fn current_text_wrap_offsets(out: &Spread, kind: CurrentFrameKind) -> [f32; 4] {
    let cur = match kind {
        CurrentFrameKind::Text(i) => out.text_frames.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Rect(i) => out.rectangles.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Oval(i) => out.ovals.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Line(i) => out.graphic_lines.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Polygon(i) => out.polygons.get(i).and_then(|s| s.text_wrap),
    };
    cur.map(|w| w.offsets).unwrap_or([0.0; 4])
}

/// W2.5 — read the current contour type + include-inside-edges off the
/// in-progress shape's `TextWrap` (so re-applying the wrap on the
/// `<TextWrapPreference>` open tag doesn't drop a `<ContourOption>`
/// that was folded in earlier).
fn current_text_wrap_contour(
    out: &Spread,
    kind: CurrentFrameKind,
) -> (Option<ContourOptionType>, Option<bool>) {
    let cur = match kind {
        CurrentFrameKind::Text(i) => out.text_frames.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Rect(i) => out.rectangles.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Oval(i) => out.ovals.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Line(i) => out.graphic_lines.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Polygon(i) => out.polygons.get(i).and_then(|s| s.text_wrap),
    };
    cur.map(|w| (w.contour_type, w.include_inside_edges))
        .unwrap_or((None, None))
}

fn apply_text_wrap(out: &mut Spread, kind: CurrentFrameKind, wrap: Option<TextWrap>) {
    match kind {
        CurrentFrameKind::Text(i) => out.text_frames[i].text_wrap = wrap,
        CurrentFrameKind::Rect(i) => out.rectangles[i].text_wrap = wrap,
        CurrentFrameKind::Oval(i) => out.ovals[i].text_wrap = wrap,
        CurrentFrameKind::Line(i) => out.graphic_lines[i].text_wrap = wrap,
        CurrentFrameKind::Polygon(i) => out.polygons[i].text_wrap = wrap,
    }
}

fn set_text_wrap_offsets(out: &mut Spread, kind: CurrentFrameKind, offsets: [f32; 4]) {
    let take = |w: &mut Option<TextWrap>| {
        if let Some(existing) = w.as_mut() {
            existing.offsets = offsets;
        } else {
            *w = Some(TextWrap {
                mode: TextWrapMode::None,
                offsets,
                invert: None,
                contour_type: None,
                include_inside_edges: None,
            });
        }
    };
    match kind {
        CurrentFrameKind::Text(i) => take(&mut out.text_frames[i].text_wrap),
        CurrentFrameKind::Rect(i) => take(&mut out.rectangles[i].text_wrap),
        CurrentFrameKind::Oval(i) => take(&mut out.ovals[i].text_wrap),
        CurrentFrameKind::Line(i) => take(&mut out.graphic_lines[i].text_wrap),
        CurrentFrameKind::Polygon(i) => take(&mut out.polygons[i].text_wrap),
    }
}

/// W2.5 — fold a `<ContourOption>` child's `ContourType` /
/// `IncludeInsideEdges` into the enclosing shape's `TextWrap`,
/// materialising a default wrap if the `<TextWrapPreference>` carried
/// no recognised mode yet (mirrors `set_text_wrap_offsets`).
fn set_text_wrap_contour(
    out: &mut Spread,
    kind: CurrentFrameKind,
    contour_type: Option<ContourOptionType>,
    include_inside_edges: Option<bool>,
) {
    let take = |w: &mut Option<TextWrap>| {
        if let Some(existing) = w.as_mut() {
            existing.contour_type = contour_type;
            existing.include_inside_edges = include_inside_edges;
        } else {
            *w = Some(TextWrap {
                mode: TextWrapMode::None,
                offsets: [0.0; 4],
                invert: None,
                contour_type,
                include_inside_edges,
            });
        }
    };
    match kind {
        CurrentFrameKind::Text(i) => take(&mut out.text_frames[i].text_wrap),
        CurrentFrameKind::Rect(i) => take(&mut out.rectangles[i].text_wrap),
        CurrentFrameKind::Oval(i) => take(&mut out.ovals[i].text_wrap),
        CurrentFrameKind::Line(i) => take(&mut out.graphic_lines[i].text_wrap),
        CurrentFrameKind::Polygon(i) => take(&mut out.polygons[i].text_wrap),
    }
}

/// Cross-cutting attributes shared by every shape element
/// (`<TextFrame>`, `<Rectangle>`, `<Oval>`, `<Polygon>`,
/// `<GraphicLine>`). Read once via [`read_common_attrs`] so each
/// per-shape arm doesn't repeat the same `attr(&e, b"...")` block.
///
/// `item_transform` is the *raw* parsed `[a b c d tx ty]` matrix —
/// callers compose it with the surrounding `group_transforms` via
/// [`effective_item_transform`] exactly like before.
struct CommonAttrs {
    self_id: Option<String>,
    item_transform: Option<[f32; 6]>,
    fill_color: Option<String>,
    fill_tint: Option<f32>,
    gradient_fill_angle: Option<f32>,
    gradient_fill_length: Option<f32>,
    gradient_stroke_angle: Option<f32>,
    gradient_stroke_length: Option<f32>,
    stroke_color: Option<String>,
    stroke_weight: Option<f32>,
    /// `StrokeType` reference. Defined by IDML on every page item; the
    /// renderer consumes it to pick built-in dash names or look up a
    /// custom `<DashedStrokeStyle>` (cycle-3 4a). Lives on `CommonAttrs`
    /// rather than `StrokeStyleAttrs` so Oval / Polygon / GraphicLine /
    /// TextFrame all get it without each shape duplicating the read.
    stroke_type: Option<String>,
    /// `GapColor` reference for a dashed / striped stroke — the colour
    /// painted in the gaps between dashes. `Swatch/None` ⇒ transparent
    /// gaps (the IDML default). Lives on `CommonAttrs` like
    /// `stroke_type` so every stroked page-item kind picks it up.
    stroke_gap_color: Option<String>,
    /// `GapTint` percentage (0..=100) for the gap colour. `None` ⇒ use
    /// the swatch at full strength.
    stroke_gap_tint: Option<f32>,
    /// W1.1 — `StrokeDashAndGap` per-frame override: alternating on/off
    /// dash lengths in pt. IDML serialises it as a space-separated list
    /// (sometimes wrapped in a `<StrokeDashAndGap>` list element); an
    /// absent attribute yields the empty vec (no per-frame override).
    /// Lives on `CommonAttrs` like `stroke_type` so every stroked
    /// page-item kind picks it up.
    stroke_dash: Vec<f32>,
    applied_object_style: Option<String>,
    item_layer: Option<String>,
    /// `OverprintFill="true"` on the IDML element. Absent attribute
    /// or unparseable value ⇒ `false` (IDML default).
    overprint_fill: bool,
    /// `OverprintStroke="true"` analogue.
    overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting="true"` excludes the
    /// item from print/export. Renderer keeps it visible on canvas
    /// but suppresses it from output passes. Default: `false`.
    nonprinting: bool,
    /// W2.5 — element-level `Visible="true|false"` (default `true`).
    /// Distinct from layer visibility: a page item can be individually
    /// hidden (InDesign's Layers-panel eye on the object row). Absent
    /// attribute ⇒ visible. The renderer skips emitting items whose
    /// `Visible="false"` (matches the layer-visibility skip).
    visible: bool,
    /// W2.5 — element-level `Locked="true|false"` (default `false`).
    /// IDML stores it on page items; InDesign blocks selection of a
    /// locked item. The renderer ignores it (locked items still paint);
    /// the canvas hit-tester gates selection on it, matching the
    /// `LayerLocked` precedent.
    locked: bool,
}

fn read_common_attrs(e: &quick_xml::events::BytesStart) -> CommonAttrs {
    CommonAttrs {
        self_id: attr(e, b"Self"),
        item_transform: attr(e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
        fill_color: attr(e, b"FillColor"),
        fill_tint: parse_tint_attr(e, b"FillTint"),
        gradient_fill_angle: attr(e, b"GradientFillAngle").and_then(|s| s.parse().ok()),
        gradient_fill_length: attr(e, b"GradientFillLength").and_then(|s| s.parse().ok()),
        gradient_stroke_angle: attr(e, b"GradientStrokeAngle").and_then(|s| s.parse().ok()),
        gradient_stroke_length: attr(e, b"GradientStrokeLength").and_then(|s| s.parse().ok()),
        stroke_color: attr(e, b"StrokeColor"),
        stroke_weight: attr(e, b"StrokeWeight").and_then(|s| s.parse().ok()),
        stroke_type: attr(e, b"StrokeType"),
        stroke_gap_color: attr(e, b"GapColor"),
        stroke_gap_tint: parse_tint_attr(e, b"GapTint"),
        // W1.1 — per-frame `StrokeDashAndGap` override parsed as a
        // space-separated list of pt lengths (the same encoding the
        // custom `<DashedStrokeStyle Pattern="…">` uses). Absent ⇒
        // empty vec (no override).
        stroke_dash: attr(e, b"StrokeDashAndGap")
            .map(|s| {
                s.split_ascii_whitespace()
                    .filter_map(|tok| tok.parse::<f32>().ok())
                    .collect()
            })
            .unwrap_or_default(),
        applied_object_style: attr(e, b"AppliedObjectStyle"),
        item_layer: attr(e, b"ItemLayer"),
        overprint_fill: attr(e, b"OverprintFill")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        overprint_stroke: attr(e, b"OverprintStroke")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        nonprinting: attr(e, b"Nonprinting")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        // W2.5 — element-level Visible defaults true, Locked false
        // (the IDML defaults when the attribute is absent).
        visible: attr(e, b"Visible")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(true),
        locked: attr(e, b"Locked")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
    }
}

/// Rectangle-only stroke style attributes (`StrokeAlignment`,
/// `EndCap`, `EndJoin`, `MiterLimit`). `StrokeType` moved to
/// [`CommonAttrs`] in cycle 4 so non-rectangle shapes can also
/// honour custom dash patterns.
struct StrokeStyleAttrs {
    stroke_alignment: Option<String>,
    end_cap: Option<String>,
    end_join: Option<String>,
    miter_limit: Option<f32>,
}

fn read_stroke_style_attrs(e: &quick_xml::events::BytesStart) -> StrokeStyleAttrs {
    StrokeStyleAttrs {
        stroke_alignment: attr(e, b"StrokeAlignment"),
        end_cap: attr(e, b"EndCap"),
        end_join: attr(e, b"EndJoin"),
        miter_limit: attr(e, b"MiterLimit").and_then(|s| s.parse().ok()),
    }
}

/// Rectangle-only corner attributes (`CornerRadius`, `CornerOption`,
/// plus the four per-corner overrides Q-16 added). The per-corner
/// values default to `None`; the renderer falls back to the legacy
/// global pair when a corner spec is empty.
struct CornerAttrs {
    corner_radius: Option<f32>,
    corner_option: Option<String>,
    corners: [CornerSpec; 4],
}

fn read_corner_attrs(e: &quick_xml::events::BytesStart) -> CornerAttrs {
    // Order: [top_left, top_right, bottom_right, bottom_left] —
    // matches the clockwise-from-top-left walk Rectangle::corners
    // documents.
    let per = [
        (
            b"TopLeftCornerOption".as_ref(),
            b"TopLeftCornerRadius".as_ref(),
        ),
        (
            b"TopRightCornerOption".as_ref(),
            b"TopRightCornerRadius".as_ref(),
        ),
        (
            b"BottomRightCornerOption".as_ref(),
            b"BottomRightCornerRadius".as_ref(),
        ),
        (
            b"BottomLeftCornerOption".as_ref(),
            b"BottomLeftCornerRadius".as_ref(),
        ),
    ];
    let mut corners = [CornerSpec::default(); 4];
    for (i, (oname, rname)) in per.iter().enumerate() {
        corners[i].option = attr(e, oname).as_deref().and_then(CornerOption::from_idml);
        corners[i].radius = attr(e, rname).and_then(|s| s.parse().ok());
    }
    CornerAttrs {
        corner_radius: attr(e, b"CornerRadius").and_then(|s| s.parse().ok()),
        corner_option: attr(e, b"CornerOption"),
        corners,
    }
}

/// Compute the axis-aligned bounding box of a non-empty point set,
/// using only the anchors (control points pull beyond the visible
/// curve and would inflate the bbox).
fn bounds_from_anchors(anchors: &[PathAnchor]) -> Bounds {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for a in anchors {
        let (x, y) = a.anchor;
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }
    Bounds {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    }
}

pub fn parse_spread(xml: &[u8]) -> Result<Spread, ParseError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut out = Spread::default();
    // Stack of <Group> ItemTransforms encountered, outermost
    // first. When a frame appears inside one or more groups, its
    // effective spread-space transform is the composition of
    // those group transforms with its own ItemTransform.
    let mut group_transforms: Vec<Option<[f32; 6]>> = Vec::new();
    // Stack of `<Group>` builders parallel to `group_transforms`.
    // Each entry accumulates the group's members + transparency
    // block until the closing tag fires, at which point the
    // builder is finalised into `out.groups`. Sub-groups register
    // themselves with the outer builder once they close, so the
    // outer group's `members` can carry a `FrameRef::Group(idx)`.
    let mut group_builders: Vec<GroupBuilder> = Vec::new();
    let mut current_frame: Option<CurrentFrame> = None;
    // Tracks the rectangle index whose `<GradientFeatherSetting>`
    // is currently open, so nested `<GradientStop>` children can
    // be appended to the right effects bag. Cleared on the
    // matching close tag. `<GradientStop>` is also a child of
    // `<Gradient>` swatches in graphic.rs — those live in a
    // different parser entirely, so the state here can stay
    // scoped to spread.rs.
    let mut current_gradient_feather: Option<CurrentFrameKind> = None;
    // Q-03: state for capturing inline `<Image><Properties><Contents>`
    // base64 CDATA. `Some(frame_kind)` between `<Contents>` start and
    // end while a frame is the active nested context; we append
    // text / cdata events into `current_contents_buf` then
    // base64-decode and stash on the parent shape at end-tag time.
    // `<Contents>` only appears under image-bearing elements in
    // spread.xml so we don't need to filter by parent tag.
    let mut current_image_contents_target: Option<CurrentFrameKind> = None;
    let mut current_contents_buf: Vec<u8> = Vec::new();
    let mut buf = Vec::new();

    // Register a freshly-opened frame with the innermost
    // `<Group>` builder, if one is active. The builder records
    // a `FrameRef` keyed by the frame's index in its backing
    // vec — that index is stable for the rest of the parse
    // (frames never get reordered after creation).
    //
    // Top-level frames (no group active) instead get appended to
    // `out.frames_in_order`, which feeds the renderer's
    // cross-shape z-order sort (Q-10).
    //
    // Registration happens at open time so self-closing
    // `<Rectangle/>` etc. (which fire as `Event::Empty` and
    // never visit the `End` arm) still get recorded. The
    // close handler below unregisters frames that ultimately
    // got dropped for missing bounds.
    fn register_with_group(
        out: &mut Spread,
        group_builders: &mut [GroupBuilder],
        frame_ref: FrameRef,
    ) {
        if let Some(b) = group_builders.last_mut() {
            b.members.push(frame_ref);
        } else {
            out.frames_in_order.push(frame_ref);
        }
    }
    fn unregister_last_in_group(
        out: &mut Spread,
        group_builders: &mut [GroupBuilder],
        expected: FrameRef,
    ) {
        if let Some(b) = group_builders.last_mut() {
            if b.members.last() == Some(&expected) {
                b.members.pop();
            }
        } else if out.frames_in_order.last() == Some(&expected) {
            out.frames_in_order.pop();
        }
    }

    // Pop the just-closed frame from its backing vec when no
    // bounds were ever supplied (neither GeometricBounds attr
    // nor PathGeometry anchors). Preserves the prior "skip
    // bounds-less frames" behaviour while letting the open-tag
    // path stay simple.
    fn drop_pending(out: &mut Spread, kind: CurrentFrameKind) {
        match kind {
            CurrentFrameKind::Text(i) => {
                debug_assert_eq!(i + 1, out.text_frames.len());
                out.text_frames.pop();
            }
            CurrentFrameKind::Rect(i) => {
                debug_assert_eq!(i + 1, out.rectangles.len());
                out.rectangles.pop();
            }
            CurrentFrameKind::Oval(i) => {
                debug_assert_eq!(i + 1, out.ovals.len());
                out.ovals.pop();
            }
            CurrentFrameKind::Line(i) => {
                debug_assert_eq!(i + 1, out.graphic_lines.len());
                out.graphic_lines.pop();
            }
            CurrentFrameKind::Polygon(i) => {
                debug_assert_eq!(i + 1, out.polygons.len());
                out.polygons.pop();
            }
        }
    }
    // Apply path-derived bounds to the just-closed frame.
    fn set_pending_bounds(out: &mut Spread, kind: CurrentFrameKind, bounds: Bounds) {
        match kind {
            CurrentFrameKind::Text(i) => out.text_frames[i].bounds = bounds,
            CurrentFrameKind::Rect(i) => out.rectangles[i].bounds = bounds,
            CurrentFrameKind::Oval(i) => out.ovals[i].bounds = bounds,
            CurrentFrameKind::Line(i) => out.graphic_lines[i].bounds = bounds,
            CurrentFrameKind::Polygon(i) => out.polygons[i].bounds = bounds,
        }
    }

    loop {
        let raw_event = reader.read_event_into(&mut buf)?;
        // W1.21: a `<Image>` container fires `Start` (it has child
        // <Properties>/<Link>/<ClippingPathSettings>); a self-closed
        // image fires `Empty`. We need that distinction to balance
        // `in_image_depth` against the matching `</Image>` End — a
        // self-closed image has no End, so it must not increment.
        let event_is_start = matches!(raw_event, Event::Start(_));
        match raw_event {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                b"Spread" | b"MasterSpread" => {
                    if out.self_id.is_none() {
                        out.self_id = attr(&e, b"Self");
                        out.item_transform =
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s));
                    }
                }
                b"Group" => {
                    let t = attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s));
                    group_transforms.push(t);
                    group_builders.push(GroupBuilder {
                        self_id: attr(&e, b"Self"),
                        item_transform: t,
                        members: Vec::new(),
                        transparency: GroupTransparency::default(),
                        stroke_transparency_depth: 0,
                        content_transparency_depth: 0,
                    });
                }
                b"Guide" => {
                    // Plan-2 §8.3 ruler guides. Both `<Guide>`
                    // and `<Empty Guide />` variants surface here.
                    // The `Orientation` + `Location` attributes
                    // are required for the guide to mean anything;
                    // unparseable entries get dropped.
                    let orientation = attr(&e, b"Orientation");
                    let location = attr(&e, b"Location").and_then(|s| s.parse::<f32>().ok());
                    if let (Some(orient), Some(loc)) = (orientation, location) {
                        let orient = match orient.as_str() {
                            "Vertical" => Some(GuideOrientation::Vertical),
                            "Horizontal" => Some(GuideOrientation::Horizontal),
                            _ => None,
                        };
                        let page_index = attr(&e, b"PageIndex")
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        if let Some(orient) = orient {
                            out.guides.push(RulerGuide {
                                orientation: orient,
                                location: loc,
                                page_index,
                            });
                        }
                    }
                }
                b"Page" => {
                    if let Some(bounds) =
                        attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                    {
                        out.pages.push(Page {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            applied_master: attr(&e, b"AppliedMaster"),
                            item_transform: attr(&e, b"ItemTransform")
                                .and_then(|s| parse_matrix(&s)),
                            master_page_transform: attr(&e, b"MasterPageTransform")
                                .and_then(|s| parse_matrix(&s)),
                            override_list: attr(&e, b"OverrideList")
                                .map(|s| s.split_whitespace().map(str::to_string).collect())
                                .unwrap_or_default(),
                            name: attr(&e, b"Name"),
                            show_master_items: attr(&e, b"ShowMasterItems")
                                .and_then(|s| s.parse().ok()),
                        });
                    }
                }
                b"MarginPreference" => {
                    // `<MarginPreference>` is a child of the enclosing
                    // `<Page>`; the most-recently-pushed page is its
                    // host. Recorded in the spread's side map keyed by
                    // the page `Self` id (panels.md gap 10). Pages with
                    // no `Self` id (synthetic) can't be keyed, so skip.
                    if let Some(host) = out.pages.last().and_then(|p| p.self_id.clone()) {
                        let f = |k: &[u8]| attr(&e, k).and_then(|s| s.parse::<f32>().ok());
                        out.page_margins.insert(
                            host,
                            MarginPreference {
                                top: f(b"Top").unwrap_or(0.0),
                                bottom: f(b"Bottom").unwrap_or(0.0),
                                left: f(b"Left").unwrap_or(0.0),
                                right: f(b"Right").unwrap_or(0.0),
                                column_count: attr(&e, b"ColumnCount")
                                    .and_then(|s| s.parse::<u32>().ok())
                                    .unwrap_or(1),
                                column_gutter: f(b"ColumnGutter").unwrap_or(0.0),
                            },
                        );
                    }
                }
                b"TextFrame" => {
                    let bounds_attr = attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                    let common = read_common_attrs(&e);
                    let item_transform =
                        effective_item_transform(&group_transforms, common.item_transform);
                    out.text_frames.push(TextFrame {
                        self_id: common.self_id,
                        parent_story: attr(&e, b"ParentStory"),
                        bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                        item_transform,
                        fill_color: common.fill_color,
                        fill_tint: common.fill_tint,
                        stroke_color: common.stroke_color,
                        stroke_weight: common.stroke_weight,
                        stroke_type: common.stroke_type,
                        stroke_gap_color: common.stroke_gap_color,
                        stroke_gap_tint: common.stroke_gap_tint,
                        stroke_dash: common.stroke_dash,
                        drop_shadow: None,
                        stroke_drop_shadow: None,
                        next_text_frame: attr(&e, b"NextTextFrame"),
                        vertical_justification: None,
                        first_baseline_offset: None,
                        minimum_first_baseline_offset: None,
                        inset_spacing: None,
                        auto_sizing: None,
                        auto_sizing_reference_point: None,
                        minimum_width_for_auto_sizing: None,
                        minimum_height_for_auto_sizing: None,
                        use_minimum_height_for_auto_sizing: None,
                        column_count: None,
                        column_gutter: None,
                        column_balance: None,
                        applied_object_style: common.applied_object_style,
                        text_wrap: None,
                        item_layer: common.item_layer,
                        is_anchored: false,
                        opacity: None,
                        blend_mode: None,
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        effects: None,
                        gradient_fill_angle: common.gradient_fill_angle,
                        gradient_fill_length: common.gradient_fill_length,
                        gradient_stroke_angle: common.gradient_stroke_angle,
                        gradient_stroke_length: common.gradient_stroke_length,
                        applied_toc_style: attr(&e, b"AppliedTOCStyle"),
                        overprint_fill: common.overprint_fill,
                        overprint_stroke: common.overprint_stroke,
                        nonprinting: common.nonprinting,
                        visible: common.visible,
                        locked: common.locked,
                    });
                    let idx = out.text_frames.len() - 1;
                    register_with_group(&mut out, &mut group_builders, FrameRef::TextFrame(idx));
                    current_frame = Some(CurrentFrame {
                        kind: CurrentFrameKind::Text(idx),
                        needs_bounds: bounds_attr.is_none(),
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        // Always retain Bezier path anchors so the
                        // renderer can detect non-rectangular text
                        // frame outlines (triangle, pentagon, …)
                        // and clip layout to the actual polygon
                        // interior rather than the AABB.
                        keep_anchors: true,
                        in_text_wrap: false,
                        stroke_transparency_depth: 0,
                        content_transparency_depth: 0,
                        in_image_depth: 0,
                        clip: None,
                        in_clipping_path: false,
                    });
                }
                b"Rectangle" => {
                    let bounds_attr = attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                    let common = read_common_attrs(&e);
                    let stroke = read_stroke_style_attrs(&e);
                    let corner = read_corner_attrs(&e);
                    let item_transform =
                        effective_item_transform(&group_transforms, common.item_transform);
                    out.rectangles.push(Rectangle {
                        self_id: common.self_id,
                        bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                        item_transform,
                        fill_color: common.fill_color,
                        fill_tint: common.fill_tint,
                        stroke_color: common.stroke_color,
                        stroke_weight: common.stroke_weight,
                        drop_shadow: None,
                        stroke_drop_shadow: None,
                        image_link: None,
                        image_bytes: None,
                        image_clip: None,
                        has_image_element: false,
                        has_inline_pdf: false,
                        image_item_transform: None,
                        applied_object_style: common.applied_object_style,
                        text_wrap: None,
                        frame_fitting: None,
                        stroke_type: common.stroke_type,
                        stroke_alignment: stroke.stroke_alignment,
                        end_cap: stroke.end_cap,
                        end_join: stroke.end_join,
                        miter_limit: stroke.miter_limit,
                        stroke_gap_color: common.stroke_gap_color,
                        stroke_gap_tint: common.stroke_gap_tint,
                        stroke_dash: common.stroke_dash,
                        item_layer: common.item_layer,
                        corner_radius: corner.corner_radius,
                        corner_option: corner.corner_option,
                        corners: corner.corners,
                        is_anchored: false,
                        opacity: None,
                        blend_mode: None,
                        effects: None,
                        gradient_fill_angle: common.gradient_fill_angle,
                        gradient_fill_length: common.gradient_fill_length,
                        gradient_stroke_angle: common.gradient_stroke_angle,
                        gradient_stroke_length: common.gradient_stroke_length,
                        text_paths: Vec::new(),
                        overprint_fill: common.overprint_fill,
                        overprint_stroke: common.overprint_stroke,
                        nonprinting: common.nonprinting,
                        visible: common.visible,
                        locked: common.locked,
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                    });
                    let idx = out.rectangles.len() - 1;
                    register_with_group(&mut out, &mut group_builders, FrameRef::Rectangle(idx));
                    current_frame = Some(CurrentFrame {
                        kind: CurrentFrameKind::Rect(idx),
                        needs_bounds: bounds_attr.is_none(),
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        // Q-11: retain anchors so stylised
                        // non-rectangular outlines (torn-paper,
                        // multi-anchor) can route through
                        // `Geometry::Polygon` instead of collapsing
                        // to the AABB.
                        keep_anchors: true,
                        in_text_wrap: false,
                        stroke_transparency_depth: 0,
                        content_transparency_depth: 0,
                        in_image_depth: 0,
                        clip: None,
                        in_clipping_path: false,
                    });
                }
                b"Oval" => {
                    let bounds_attr = attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                    let common = read_common_attrs(&e);
                    let item_transform =
                        effective_item_transform(&group_transforms, common.item_transform);
                    out.ovals.push(Oval {
                        self_id: common.self_id,
                        bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                        item_transform,
                        fill_color: common.fill_color,
                        fill_tint: common.fill_tint,
                        stroke_color: common.stroke_color,
                        stroke_weight: common.stroke_weight,
                        stroke_type: common.stroke_type,
                        stroke_alignment: attr(&e, b"StrokeAlignment"),
                        stroke_gap_color: common.stroke_gap_color,
                        stroke_gap_tint: common.stroke_gap_tint,
                        stroke_dash: common.stroke_dash,
                        drop_shadow: None,
                        stroke_drop_shadow: None,
                        applied_object_style: common.applied_object_style,
                        text_wrap: None,
                        item_layer: common.item_layer,
                        gradient_fill_angle: common.gradient_fill_angle,
                        gradient_fill_length: common.gradient_fill_length,
                        gradient_stroke_angle: common.gradient_stroke_angle,
                        gradient_stroke_length: common.gradient_stroke_length,
                        opacity: None,
                        blend_mode: None,
                        image_link: None,
                        image_bytes: None,
                        image_clip: None,
                        has_image_element: false,
                        has_inline_pdf: false,
                        image_item_transform: None,
                        effects: None,
                        overprint_fill: common.overprint_fill,
                        overprint_stroke: common.overprint_stroke,
                        nonprinting: common.nonprinting,
                        visible: common.visible,
                        locked: common.locked,
                    });
                    let idx = out.ovals.len() - 1;
                    register_with_group(&mut out, &mut group_builders, FrameRef::Oval(idx));
                    current_frame = Some(CurrentFrame {
                        kind: CurrentFrameKind::Oval(idx),
                        needs_bounds: bounds_attr.is_none(),
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        keep_anchors: false,
                        in_text_wrap: false,
                        stroke_transparency_depth: 0,
                        content_transparency_depth: 0,
                        in_image_depth: 0,
                        clip: None,
                        in_clipping_path: false,
                    });
                }
                b"StrokeTransparencySetting" => {
                    // Drop shadows under this wrapper describe a
                    // shadow cast by the frame's stroke — captured
                    // separately so the renderer can gate emission
                    // on stroke visibility.
                    if let Some(cf) = current_frame.as_mut() {
                        cf.stroke_transparency_depth += 1;
                    } else if let Some(b) = group_builders.last_mut() {
                        b.stroke_transparency_depth += 1;
                    }
                }
                b"ContentTransparencySetting" => {
                    // Drop shadows under this wrapper describe
                    // content-only shadows that don't map onto our
                    // single-shadow-per-frame model; skipped.
                    if let Some(cf) = current_frame.as_mut() {
                        cf.content_transparency_depth += 1;
                    } else if let Some(b) = group_builders.last_mut() {
                        b.content_transparency_depth += 1;
                    }
                }
                b"DropShadowSetting" => {
                    if let Some(setting) = parse_drop_shadow(&e) {
                        // Only "Drop"/"Default" mode results in a
                        // visible shadow. "None" means the shadow
                        // is disabled even though the setting is
                        // serialised.
                        if setting.mode != "None" {
                            if let Some(cf) = current_frame.as_ref() {
                                if cf.content_transparency_depth > 0 {
                                    // Content-only shadow — skip.
                                } else if cf.stroke_transparency_depth > 0 {
                                    // Stroke-only shadow — captured for
                                    // conditional emission by the
                                    // renderer.
                                    match cf.kind {
                                        CurrentFrameKind::Text(i) => {
                                            out.text_frames[i].stroke_drop_shadow = Some(setting);
                                        }
                                        CurrentFrameKind::Rect(i) => {
                                            out.rectangles[i].stroke_drop_shadow = Some(setting);
                                        }
                                        CurrentFrameKind::Oval(i) => {
                                            out.ovals[i].stroke_drop_shadow = Some(setting);
                                        }
                                        CurrentFrameKind::Line(_)
                                        | CurrentFrameKind::Polygon(_) => {
                                            // GraphicLine + Polygon have
                                            // no shadow fields today;
                                            // ignore.
                                        }
                                    }
                                } else {
                                    match cf.kind {
                                        CurrentFrameKind::Text(i) => {
                                            out.text_frames[i].drop_shadow = Some(setting);
                                        }
                                        CurrentFrameKind::Rect(i) => {
                                            out.rectangles[i].drop_shadow = Some(setting);
                                        }
                                        CurrentFrameKind::Oval(i) => {
                                            out.ovals[i].drop_shadow = Some(setting);
                                        }
                                        CurrentFrameKind::Line(_)
                                        | CurrentFrameKind::Polygon(_) => {
                                            // GraphicLine + Polygon have
                                            // no drop_shadow field today;
                                            // ignore.
                                        }
                                    }
                                }
                            } else if let Some(b) = group_builders.last_mut() {
                                // No frame is open but a `<Group>`
                                // is — route the shadow to the
                                // innermost group's transparency
                                // block. Stroke-/content-only
                                // wrappers around a group don't
                                // map onto our model and are
                                // skipped.
                                if b.content_transparency_depth == 0
                                    && b.stroke_transparency_depth == 0
                                {
                                    b.transparency.drop_shadow = Some(setting);
                                }
                            }
                        }
                    }
                }
                b"AnchoredObjectSetting" => {
                    // Mark the current frame as an anchored object.
                    // Renderer-side text-flow integration is
                    // queued; today the flag is informational.
                    if let Some(cf) = current_frame.as_ref() {
                        match cf.kind {
                            CurrentFrameKind::Text(i) => {
                                out.text_frames[i].is_anchored = true;
                            }
                            CurrentFrameKind::Rect(i) => {
                                out.rectangles[i].is_anchored = true;
                            }
                            _ => {}
                        }
                    }
                }
                b"InnerShadowSetting"
                | b"OuterGlowSetting"
                | b"InnerGlowSetting"
                | b"BevelAndEmbossSetting"
                | b"SatinSetting"
                | b"FeatherSetting"
                | b"DirectionalFeatherSetting"
                | b"GradientFeatherSetting" => {
                    // Surface each effect's parameters onto the
                    // current shape's effects bag, gated on the
                    // `Applied="true"` flag — `Applied="false"` (or
                    // absent) means the user disabled the effect
                    // even though IDML still serialises the settings
                    // for round-trip preservation. Q-04: extended
                    // from Rectangle-only to all five shape kinds.
                    if let Some(kind) = current_frame.as_ref().map(|cf| cf.kind) {
                        let applied = attr(&e, b"Applied")
                            .and_then(|s| s.parse::<bool>().ok())
                            .unwrap_or(false);
                        if !applied {
                            // Effect is present but disabled; skip
                            // the parameter capture entirely so the
                            // renderer doesn't accidentally emit it.
                            continue;
                        }
                        let Some(bag_slot) = effects_slot_mut(&mut out, kind) else {
                            continue;
                        };
                        let bag = bag_slot.get_or_insert_with(Default::default);
                        match e.name().as_ref() {
                            b"InnerShadowSetting" => {
                                bag.inner_shadow = Some(InnerShadowParams {
                                    x_offset: parse_f(&e, b"XOffset"),
                                    y_offset: parse_f(&e, b"YOffset"),
                                    size: parse_f(&e, b"Size"),
                                    opacity_pct: parse_f(&e, b"Opacity"),
                                    effect_color: attr(&e, b"EffectColor"),
                                    angle_deg: parse_f(&e, b"Angle"),
                                    distance: parse_f(&e, b"Distance"),
                                    choke_pct: parse_f(&e, b"ChokeAmount"),
                                    blend_mode: attr(&e, b"BlendMode"),
                                    noise_pct: parse_f(&e, b"Noise"),
                                });
                            }
                            b"OuterGlowSetting" => {
                                bag.outer_glow = Some(OuterGlowParams {
                                    size: parse_f(&e, b"Size"),
                                    opacity_pct: parse_f(&e, b"Opacity"),
                                    effect_color: attr(&e, b"EffectColor"),
                                    spread_pct: parse_f(&e, b"Spread"),
                                    blend_mode: attr(&e, b"BlendMode"),
                                    noise_pct: parse_f(&e, b"Noise"),
                                });
                            }
                            b"InnerGlowSetting" => {
                                bag.inner_glow = Some(InnerGlowParams {
                                    size: parse_f(&e, b"Size"),
                                    opacity_pct: parse_f(&e, b"Opacity"),
                                    effect_color: attr(&e, b"EffectColor"),
                                    choke_pct: parse_f(&e, b"ChokeAmount"),
                                    blend_mode: attr(&e, b"BlendMode"),
                                    source: attr(&e, b"Source"),
                                    noise_pct: parse_f(&e, b"Noise"),
                                });
                            }
                            b"BevelAndEmbossSetting" => {
                                bag.bevel = Some(BevelEmbossParams {
                                    depth_pct: parse_f(&e, b"Depth"),
                                    size: parse_f(&e, b"Size"),
                                    angle_deg: parse_f(&e, b"Angle"),
                                    altitude_deg: parse_f(&e, b"Altitude"),
                                    highlight_color: attr(&e, b"HighlightColor"),
                                    shadow_color: attr(&e, b"ShadowColor"),
                                    highlight_opacity_pct: parse_f(&e, b"HighlightOpacity"),
                                    shadow_opacity_pct: parse_f(&e, b"ShadowOpacity"),
                                    style: attr(&e, b"Style"),
                                    direction: attr(&e, b"Direction"),
                                    technique: attr(&e, b"Technique"),
                                    soften: parse_f(&e, b"Soften"),
                                });
                            }
                            b"SatinSetting" => {
                                bag.satin = Some(SatinParams {
                                    size: parse_f(&e, b"Size"),
                                    angle_deg: parse_f(&e, b"Angle"),
                                    distance: parse_f(&e, b"Distance"),
                                    effect_color: attr(&e, b"EffectColor"),
                                    opacity_pct: parse_f(&e, b"Opacity"),
                                    blend_mode: attr(&e, b"BlendMode"),
                                    invert: attr(&e, b"Invert")
                                        .and_then(|s| s.parse::<bool>().ok()),
                                });
                            }
                            b"FeatherSetting" => {
                                bag.feather = Some(FeatherParams {
                                    width: parse_f(&e, b"Width"),
                                    corner_type: attr(&e, b"CornerType"),
                                    noise_pct: parse_f(&e, b"Noise"),
                                    choke_pct: parse_f(&e, b"ChokeAmount"),
                                });
                            }
                            b"DirectionalFeatherSetting" => {
                                bag.directional_feather = Some(DirectionalFeatherParams {
                                    left_width: parse_f(&e, b"LeftWidth"),
                                    right_width: parse_f(&e, b"RightWidth"),
                                    top_width: parse_f(&e, b"TopWidth"),
                                    bottom_width: parse_f(&e, b"BottomWidth"),
                                    angle_deg: parse_f(&e, b"Angle"),
                                    noise_pct: parse_f(&e, b"NoiseAmount"),
                                    choke_pct: parse_f(&e, b"ChokeAmount"),
                                    corner_type: attr(&e, b"CornerType"),
                                });
                            }
                            b"GradientFeatherSetting" => {
                                // InDesign uses `GradientStart`
                                // (an "x y" pair) + `Length` +
                                // `HiliteAngle` to describe the
                                // gradient direction; the IDML
                                // spec also accepts an explicit
                                // `GradientEnd` pair. We accept
                                // both shapes — the parser
                                // computes the end point from
                                // start + (Length × Angle) when
                                // GradientEnd is missing so the
                                // renderer sees one canonical
                                // pair regardless of the source.
                                let start_point = attr(&e, b"GradientStart")
                                    .as_deref()
                                    .and_then(parse_xy_pair);
                                let end_point = attr(&e, b"GradientEnd")
                                    .as_deref()
                                    .and_then(parse_xy_pair)
                                    .or_else(|| {
                                        // `HiliteAngle` is the *highlight*
                                        // ramp orientation, not the
                                        // gradient axis direction —
                                        // InDesign uses it for the
                                        // radial-feather hilite preview
                                        // and leaves the gradient axis
                                        // horizontal (0°) when no
                                        // dedicated angle attribute is
                                        // serialised. Tied to the visible
                                        // page-5 yellow→white feather in
                                        // `manual-sample.idml`, where
                                        // `HiliteAngle="-62.2"` paints a
                                        // diagonal smudge instead of the
                                        // expected left→right fade.
                                        let s = start_point?;
                                        let length = parse_f(&e, b"Length")?;
                                        let angle = parse_f(&e, b"GradientAngle")
                                            .or_else(|| parse_f(&e, b"Angle"))
                                            .unwrap_or(0.0);
                                        let rad = angle.to_radians();
                                        let (sin, cos) = rad.sin_cos();
                                        Some((s.0 + length * cos, s.1 - length * sin))
                                    });
                                bag.gradient_feather = Some(GradientFeatherParams {
                                    gradient_type: attr(&e, b"Type"),
                                    start_point,
                                    end_point,
                                    angle_deg: parse_f(&e, b"GradientAngle")
                                        .or_else(|| parse_f(&e, b"Angle")),
                                    stops: Vec::new(),
                                });
                                // Mark the current frame's gradient
                                // feather as the open target so
                                // nested `<GradientStop>` /
                                // `<OpacityGradientStop>` children
                                // can append to it. Cleared on the
                                // close tag below. Q-04: tracks
                                // CurrentFrameKind (not just rect
                                // index) so non-Rectangle shapes
                                // can host gradient feathers too.
                                current_gradient_feather = Some(kind);
                            }
                            _ => {}
                        }
                    }
                }
                b"GradientStop" | b"OpacityGradientStop" => {
                    // Children of an open `<GradientFeatherSetting>`
                    // define the alpha falloff. InDesign serialises
                    // them as `<OpacityGradientStop Opacity="..."
                    // Location="..." Midpoint="...">`; the IDML
                    // spec also documents a `<GradientStop StopColor
                    // ="..." Alpha="..." Location="..."
                    // GradientStopMidpoint="...">` form. Both are
                    // accepted — the alpha lands in `alpha_pct`
                    // regardless of which attribute the IDML
                    // actually used.
                    //
                    // `<GradientStop>` is also a child of
                    // `<Gradient>` swatches in graphic.rs; that's
                    // a separate parser file, so the routing here
                    // only fires when a gradient-feather block is
                    // actually open in the spread parser.
                    if let Some(kind) = current_gradient_feather {
                        if let Some(bag) = effects_slot_mut(&mut out, kind).and_then(|s| s.as_mut())
                        {
                            if let Some(gf) = bag.gradient_feather.as_mut() {
                                let location_pct = parse_f(&e, b"Location").unwrap_or(0.0);
                                // `Opacity` (OpacityGradientStop)
                                // takes precedence; `Alpha`
                                // (GradientStop spec form) falls
                                // back; default 100 (fully opaque)
                                // when neither is set.
                                let alpha_pct = parse_f(&e, b"Opacity")
                                    .or_else(|| parse_f(&e, b"Alpha"))
                                    .unwrap_or(100.0);
                                let midpoint_pct = parse_f(&e, b"GradientStopMidpoint")
                                    .or_else(|| parse_f(&e, b"Midpoint"))
                                    .unwrap_or(50.0);
                                gf.stops.push(GradientFeatherStop {
                                    stop_color: attr(&e, b"StopColor"),
                                    location_pct,
                                    alpha_pct,
                                    midpoint_pct,
                                });
                            }
                        }
                    }
                }
                b"BlendingSetting" => {
                    // Nested under <TransparencySetting>; we don't
                    // track the wrapper because no other element
                    // shares this name. Opacity is 0..=100;
                    // BlendMode is a string (Normal / Multiply /
                    // Screen / etc).
                    let opacity = attr(&e, b"Opacity").and_then(|s| s.parse::<f32>().ok());
                    let mode = attr(&e, b"BlendMode");
                    if let Some(cf) = current_frame.as_ref() {
                        match cf.kind {
                            CurrentFrameKind::Rect(i) => {
                                if opacity.is_some() {
                                    out.rectangles[i].opacity = opacity;
                                }
                                if mode.is_some() {
                                    out.rectangles[i].blend_mode = mode;
                                }
                            }
                            CurrentFrameKind::Text(i) => {
                                if opacity.is_some() {
                                    out.text_frames[i].opacity = opacity;
                                }
                                if mode.is_some() {
                                    out.text_frames[i].blend_mode = mode;
                                }
                            }
                            CurrentFrameKind::Oval(i) => {
                                if opacity.is_some() {
                                    out.ovals[i].opacity = opacity;
                                }
                                if mode.is_some() {
                                    out.ovals[i].blend_mode = mode;
                                }
                            }
                            CurrentFrameKind::Polygon(i) => {
                                if opacity.is_some() {
                                    out.polygons[i].opacity = opacity;
                                }
                                if mode.is_some() {
                                    out.polygons[i].blend_mode = mode;
                                }
                            }
                            _ => {
                                // GraphicLines don't yet surface
                                // opacity / blend_mode;
                                // ignore until they do.
                            }
                        }
                    } else if let Some(b) = group_builders.last_mut() {
                        // No frame is open but a `<Group>` is —
                        // route the BlendingSetting to the
                        // innermost group's transparency block so
                        // the renderer can bracket the group's
                        // member range with a single opacity /
                        // blend mode.
                        if opacity.is_some() {
                            b.transparency.opacity = opacity;
                        }
                        if mode.is_some() {
                            b.transparency.blend_mode = mode;
                        }
                    }
                }
                b"GeometryPathType" => {
                    // Record the start of a new subpath. IDML's
                    // `<PathGeometry>` may host multiple
                    // `<GeometryPathType>` children to form a
                    // compound path (e.g. a square with a hole);
                    // capturing the boundary lets the renderer
                    // emit one MoveTo/Close per contour rather
                    // than joining them with a straight segment.
                    // We only track this for shapes that retain
                    // anchors (text frames / graphic lines /
                    // polygons); for the others the field is
                    // unused. The companion `PathOpen` flag lifts
                    // here too so the renderer can skip auto-close
                    // on open paths (P-15).
                    if let Some(cf) = current_frame.as_mut() {
                        if cf.in_clipping_path {
                            // W1.21: a `<GeometryPathType>` under
                            // `<ClippingPathSettings>` begins a clip
                            // subpath. Compound clips (a star with a
                            // punched centre) keep their holes via
                            // these boundaries, exactly like frame
                            // compound paths.
                            if let Some(clip) = cf.clip.as_mut() {
                                clip.clip_subpath_starts.push(clip.clip_anchors.len());
                                let open = attr(&e, b"PathOpen")
                                    .and_then(|s| s.parse::<bool>().ok())
                                    .unwrap_or(false);
                                clip.clip_subpath_open.push(open);
                            }
                        } else if cf.in_image_depth == 0 && cf.keep_anchors {
                            // The image's own `<PathGeometry>` (when
                            // `in_image_depth > 0`) describes the
                            // placed picture's native box, not the
                            // frame's silhouette — skip it so a
                            // Polygon host's outline isn't polluted.
                            cf.subpath_starts.push(cf.anchors.len());
                            let open = attr(&e, b"PathOpen")
                                .and_then(|s| s.parse::<bool>().ok())
                                .unwrap_or(false);
                            cf.subpath_open.push(open);
                        }
                    }
                }
                b"KeyValuePair" => {
                    // `Properties/Label` entry on the current page
                    // item — the plugin-metadata carrier. Attach to
                    // the OPEN frame's Self id (nested anchored
                    // frames are skipped by this parser, so their
                    // labels are ignored rather than mis-attached
                    // to the host).
                    if let Some(cf) = current_frame.as_ref() {
                        let key = crate::util::attr_unescaped(&e, b"Key");
                        let value = crate::util::attr_unescaped(&e, b"Value");
                        if let (Some(key), Some(value)) = (key, value) {
                            let self_id = match cf.kind {
                                CurrentFrameKind::Text(i) => {
                                    out.text_frames.get(i).and_then(|f| f.self_id.clone())
                                }
                                CurrentFrameKind::Rect(i) => {
                                    out.rectangles.get(i).and_then(|f| f.self_id.clone())
                                }
                                CurrentFrameKind::Oval(i) => {
                                    out.ovals.get(i).and_then(|f| f.self_id.clone())
                                }
                                CurrentFrameKind::Line(i) => {
                                    out.graphic_lines.get(i).and_then(|f| f.self_id.clone())
                                }
                                CurrentFrameKind::Polygon(i) => {
                                    out.polygons.get(i).and_then(|f| f.self_id.clone())
                                }
                            };
                            if let Some(self_id) = self_id {
                                let entries = out.labels.entry(self_id).or_default();
                                match entries.iter_mut().find(|(k, _)| *k == key) {
                                    Some(slot) => slot.1 = value,
                                    None => entries.push((key, value)),
                                }
                            }
                        }
                    }
                }
                b"PathPointType" => {
                    // Accumulate path-anchor points so the close
                    // tag can derive bounds when no
                    // GeometricBounds attribute was present, and
                    // so polygon rasterisation has the actual
                    // Bezier control points to work with. Real-
                    // world InDesign exports always serialise
                    // geometry this way.
                    if let Some(cf) = current_frame.as_mut() {
                        if cf.in_clipping_path {
                            // W1.21: route clip-path anchors into the
                            // pending `ClippingPathSettings`. Same
                            // anchor/handle shape as frame geometry,
                            // but in the image's pixel space.
                            if let Some(clip) = cf.clip.as_mut() {
                                if let Some(a) = attr(&e, b"Anchor").and_then(|s| parse_xy_pair(&s))
                                {
                                    let left = attr(&e, b"LeftDirection")
                                        .and_then(|s| parse_xy_pair(&s))
                                        .unwrap_or(a);
                                    let right = attr(&e, b"RightDirection")
                                        .and_then(|s| parse_xy_pair(&s))
                                        .unwrap_or(a);
                                    clip.clip_anchors.push(PathAnchor {
                                        anchor: a,
                                        left,
                                        right,
                                    });
                                }
                            }
                        } else if cf.in_image_depth == 0 && (cf.needs_bounds || cf.keep_anchors) {
                            let anchor = attr(&e, b"Anchor").and_then(|s| parse_xy_pair(&s));
                            if let Some(a) = anchor {
                                let left = attr(&e, b"LeftDirection")
                                    .and_then(|s| parse_xy_pair(&s))
                                    .unwrap_or(a);
                                let right = attr(&e, b"RightDirection")
                                    .and_then(|s| parse_xy_pair(&s))
                                    .unwrap_or(a);
                                cf.anchors.push(PathAnchor {
                                    anchor: a,
                                    left,
                                    right,
                                });
                            }
                        }
                    }
                }
                b"TextWrapPreference" => {
                    // The wrap rect itself comes from the
                    // enclosing shape's geometry; we just record
                    // mode + offsets here. Offsets serialise as
                    // a `TextWrapOffset` child element rather
                    // than attributes, so the actual numbers
                    // arrive a few events later (handled below).
                    if let Some(cf) = current_frame.as_mut() {
                        let mode = attr(&e, b"TextWrapMode")
                            .as_deref()
                            .map(TextWrapMode::from_idml)
                            .unwrap_or(TextWrapMode::None);
                        let invert = attr(&e, b"Inverse")
                            .or_else(|| attr(&e, b"Inverted"))
                            .and_then(|s| s.parse::<bool>().ok());
                        let kind = cf.kind;
                        let prior_offsets = current_text_wrap_offsets(&out, kind);
                        // W2.5 — preserve any contour info already
                        // folded in (a `<ContourOption>` child can be
                        // emitted before OR after — InDesign emits it
                        // after, but the read is order-robust).
                        let (prior_contour, prior_inside) = current_text_wrap_contour(&out, kind);
                        apply_text_wrap(
                            &mut out,
                            kind,
                            Some(TextWrap {
                                mode,
                                offsets: prior_offsets,
                                invert,
                                contour_type: prior_contour,
                                include_inside_edges: prior_inside,
                            }),
                        );
                        cf.in_text_wrap = true;
                    }
                }
                b"TextWrapOffset" => {
                    if let Some(cf) = current_frame.as_ref() {
                        if cf.in_text_wrap {
                            let offsets = [
                                attr(&e, b"Top").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                                attr(&e, b"Left")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0),
                                attr(&e, b"Bottom")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0),
                                attr(&e, b"Right")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0),
                            ];
                            set_text_wrap_offsets(&mut out, cf.kind, offsets);
                        }
                    }
                }
                // W2.5 — `<ContourOption>` child of
                // `<TextWrapPreference>`: the contour source +
                // include-inside-edges for `ContourTextWrap`.
                b"ContourOption" => {
                    if let Some(cf) = current_frame.as_ref() {
                        if cf.in_text_wrap {
                            let contour_type = attr(&e, b"ContourType")
                                .as_deref()
                                .map(ContourOptionType::from_idml);
                            let include_inside = attr(&e, b"IncludeInsideEdges")
                                .and_then(|s| s.parse::<bool>().ok());
                            set_text_wrap_contour(&mut out, cf.kind, contour_type, include_inside);
                        }
                    }
                }
                b"TextFramePreference" => {
                    if let Some(CurrentFrameKind::Text(i)) =
                        current_frame.as_ref().map(|cf| cf.kind)
                    {
                        let f = &mut out.text_frames[i];
                        if let Some(vj) = attr(&e, b"VerticalJustification")
                            .as_deref()
                            .and_then(VerticalJustification::from_idml)
                        {
                            f.vertical_justification = Some(vj);
                        }
                        if let Some(fbo) = attr(&e, b"FirstBaselineOffset")
                            .as_deref()
                            .and_then(FirstBaselineOffset::from_idml)
                        {
                            f.first_baseline_offset = Some(fbo);
                        }
                        if let Some(min_fbo) = attr(&e, b"MinimumFirstBaselineOffset")
                            .and_then(|s| s.parse::<f32>().ok())
                        {
                            f.minimum_first_baseline_offset = Some(min_fbo);
                        }
                        if let Some(insets) =
                            attr(&e, b"InsetSpacing").and_then(|s| parse_insets(&s))
                        {
                            f.inset_spacing = Some(insets);
                        }
                        if let Some(at) = attr(&e, b"AutoSizingType")
                            .as_deref()
                            .and_then(AutoSizingType::from_idml)
                        {
                            f.auto_sizing = Some(at);
                        }
                        if let Some(rp) = attr(&e, b"AutoSizingReferencePoint")
                            .as_deref()
                            .and_then(AutoSizingReferencePoint::from_idml)
                        {
                            f.auto_sizing_reference_point = Some(rp);
                        }
                        if let Some(min_w) = attr(&e, b"MinimumWidthForAutoSizing")
                            .and_then(|s| s.parse::<f32>().ok())
                        {
                            f.minimum_width_for_auto_sizing = Some(min_w);
                        }
                        if let Some(min_h) = attr(&e, b"MinimumHeightForAutoSizing")
                            .and_then(|s| s.parse::<f32>().ok())
                        {
                            f.minimum_height_for_auto_sizing = Some(min_h);
                        }
                        if let Some(use_min_h) = attr(&e, b"UseMinimumHeightForAutoSizing")
                            .and_then(|s| s.parse::<bool>().ok())
                        {
                            f.use_minimum_height_for_auto_sizing = Some(use_min_h);
                        }
                        if let Some(cc) =
                            attr(&e, b"TextColumnCount").and_then(|s| s.parse::<u32>().ok())
                        {
                            f.column_count = Some(cc);
                        }
                        if let Some(cg) =
                            attr(&e, b"TextColumnGutter").and_then(|s| s.parse::<f32>().ok())
                        {
                            f.column_gutter = Some(cg);
                        }
                        if let Some(cb) =
                            attr(&e, b"VerticalBalanceColumns").and_then(|s| s.parse::<bool>().ok())
                        {
                            f.column_balance = Some(cb);
                        }
                    }
                }
                b"Image" | b"EPSImage" | b"PDF" | b"ImportedPage" | b"Link" => {
                    // IDML's image-bearing frame nests an
                    // <Image> with a LinkResourceURI on the
                    // element itself or on its <Link> child.
                    // Both Rectangle and Polygon may host placed
                    // images; routing here dispatches on the
                    // open frame's kind.
                    //
                    // The image-element tags (Image / EPSImage /
                    // PDF / ImportedPage) also flip
                    // `has_image_element` so the renderer can
                    // distinguish a plain colour swatch from an
                    // image frame whose link failed to resolve
                    // (Envato template placeholders) and stamp
                    // InDesign's missing-image placeholder
                    // instead of falling back to raw fill.
                    let is_image_element = !matches!(e.name().as_ref(), b"Link");
                    let is_pdf_element = matches!(e.name().as_ref(), b"PDF");
                    // W1.21: entering an image container — its own
                    // `<PathGeometry>` is the picture box, not the
                    // frame outline, so suppress frame-anchor
                    // accumulation until the matching `</Image>`.
                    // Only `Start` (a container) bumps the depth;
                    // a self-closed `<Link/>` / `<Image/>` does not.
                    if is_image_element && event_is_start {
                        if let Some(cf) = current_frame.as_mut() {
                            cf.in_image_depth += 1;
                        }
                    }
                    let element_uri = attr(&e, b"LinkResourceURI").or_else(|| attr(&e, b"href"));
                    // Q-06: a `<PDF>` element with no link URI carries
                    // its content as inline `<Contents>` CDATA we can't
                    // decode. Flag it so the renderer renders the
                    // frame's intrinsic FillColor instead of the
                    // missing-image grey-X placeholder.
                    let inline_pdf = is_pdf_element && element_uri.is_none();
                    match current_frame.as_ref().map(|cf| cf.kind) {
                        Some(CurrentFrameKind::Rect(i)) => {
                            if is_image_element {
                                out.rectangles[i].has_image_element = true;
                            }
                            if inline_pdf {
                                out.rectangles[i].has_inline_pdf = true;
                            }
                            if let Some(uri) = element_uri {
                                // First-write-wins so the outer <Image>
                                // attribute beats the inner <Link>'s.
                                if out.rectangles[i].image_link.is_none() {
                                    out.rectangles[i].image_link = Some(uri);
                                }
                            }
                            if e.name().as_ref() == b"Image" {
                                if let Some(m) =
                                    attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s))
                                {
                                    if out.rectangles[i].image_item_transform.is_none() {
                                        out.rectangles[i].image_item_transform = Some(m);
                                    }
                                }
                                let host = out.rectangles[i].self_id.clone();
                                record_image_metadata(&mut out, host, &e);
                            }
                        }
                        Some(CurrentFrameKind::Polygon(i)) => {
                            if is_image_element {
                                out.polygons[i].has_image_element = true;
                            }
                            if inline_pdf {
                                out.polygons[i].has_inline_pdf = true;
                            }
                            if let Some(uri) = element_uri {
                                if out.polygons[i].image_link.is_none() {
                                    out.polygons[i].image_link = Some(uri);
                                }
                            }
                            if e.name().as_ref() == b"Image" {
                                if let Some(m) =
                                    attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s))
                                {
                                    if out.polygons[i].image_item_transform.is_none() {
                                        out.polygons[i].image_item_transform = Some(m);
                                    }
                                }
                                let host = out.polygons[i].self_id.clone();
                                record_image_metadata(&mut out, host, &e);
                            }
                        }
                        Some(CurrentFrameKind::Oval(i)) => {
                            if is_image_element {
                                out.ovals[i].has_image_element = true;
                            }
                            if inline_pdf {
                                out.ovals[i].has_inline_pdf = true;
                            }
                            if let Some(uri) = element_uri {
                                if out.ovals[i].image_link.is_none() {
                                    out.ovals[i].image_link = Some(uri);
                                }
                            }
                            if e.name().as_ref() == b"Image" {
                                if let Some(m) =
                                    attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s))
                                {
                                    if out.ovals[i].image_item_transform.is_none() {
                                        out.ovals[i].image_item_transform = Some(m);
                                    }
                                }
                                let host = out.ovals[i].self_id.clone();
                                record_image_metadata(&mut out, host, &e);
                            }
                        }
                        _ => {}
                    }
                }
                b"ClippingPathSettings" => {
                    // W1.21: `<ClippingPathSettings>` nests inside the
                    // open `<Image>`. Parse the type + knobs onto the
                    // current frame's pending clip; any
                    // `<PathGeometry>` child (UserModifiedPath) then
                    // feeds `clip_anchors` while `in_clipping_path`
                    // holds. A self-closed element (ClippingType="None"
                    // with no geometry) still records the type so the
                    // renderer knows there's no clip.
                    if let Some(cf) = current_frame.as_mut() {
                        let clipping_type =
                            attr(&e, b"ClippingType").map(|s| ClippingType::from_idml(&s));
                        let invert_path = attr(&e, b"InvertPath")
                            .and_then(|s| s.parse::<bool>().ok())
                            .unwrap_or(false);
                        let include_inside_edges = attr(&e, b"IncludeInsideEdges")
                            .and_then(|s| s.parse::<bool>().ok())
                            .unwrap_or(false);
                        let applied_path_name =
                            attr(&e, b"AppliedPathName").filter(|s| !s.is_empty() && s != "$ID/");
                        let threshold = attr(&e, b"Threshold").and_then(|s| s.parse::<f32>().ok());
                        let tolerance = attr(&e, b"Tolerance").and_then(|s| s.parse::<f32>().ok());
                        cf.clip = Some(ClippingPathSettings {
                            clipping_type,
                            invert_path,
                            include_inside_edges,
                            applied_path_name,
                            threshold,
                            tolerance,
                            clip_anchors: Vec::new(),
                            clip_subpath_starts: Vec::new(),
                            clip_subpath_open: Vec::new(),
                        });
                        // Only a container (Start) hosts geometry; a
                        // self-closed settings element has none.
                        cf.in_clipping_path = event_is_start;
                    }
                }
                b"Contents" => {
                    // Q-03: enter the inline-image base64 capture
                    // path when we're nested inside a frame.
                    // `<Contents>` only appears under image-bearing
                    // tags in spread.xml so this branch is safe
                    // without a parent-tag filter.
                    if let Some(kind) = current_frame.as_ref().map(|cf| cf.kind) {
                        current_image_contents_target = Some(kind);
                        current_contents_buf.clear();
                    }
                }
                b"FrameFittingOption" => {
                    // Attaches to the current Rectangle. Crops are
                    // signed pt offsets — negative values grow the
                    // image past the frame edge for FillProportionally
                    // fits.
                    if let Some(CurrentFrameKind::Rect(i)) =
                        current_frame.as_ref().map(|cf| cf.kind)
                    {
                        out.rectangles[i].frame_fitting = Some(FrameFittingOption {
                            left_crop: attr(&e, b"LeftCrop").and_then(|s| s.parse().ok()),
                            top_crop: attr(&e, b"TopCrop").and_then(|s| s.parse().ok()),
                            right_crop: attr(&e, b"RightCrop").and_then(|s| s.parse().ok()),
                            bottom_crop: attr(&e, b"BottomCrop").and_then(|s| s.parse().ok()),
                            fitting_on_empty_frame: attr(&e, b"FittingOnEmptyFrame"),
                            reference_point: attr(&e, b"FittingAlignment"),
                            auto_fit: attr(&e, b"AutoFit").and_then(|s| s.parse::<bool>().ok()),
                        });
                    }
                }
                b"TextPath" => {
                    // `<TextPath>` attaches a story to the current
                    // shape's path (Polygon / Rectangle /
                    // GraphicLine). The shape's own
                    // `<PathGeometry>` provides the curve geometry;
                    // we only record the story reference plus a
                    // few alignment knobs here.
                    if let (Some(cf), Some(parent_story)) =
                        (current_frame.as_ref(), attr(&e, b"ParentStory"))
                    {
                        let tp = TextPath {
                            self_id: attr(&e, b"Self"),
                            parent_story,
                            path_alignment: attr(&e, b"PathAlignment"),
                            path_type_alignment: attr(&e, b"PathTypeAlignment"),
                            path_effect: attr(&e, b"PathEffect"),
                            flip_path_effect: attr(&e, b"FlipPathEffect"),
                            start_bracket: attr(&e, b"StartBracket").and_then(|s| s.parse().ok()),
                            end_bracket: attr(&e, b"EndBracket").and_then(|s| s.parse().ok()),
                        };
                        match cf.kind {
                            CurrentFrameKind::Polygon(i) => {
                                out.polygons[i].text_paths.push(tp);
                            }
                            CurrentFrameKind::Rect(i) => {
                                out.rectangles[i].text_paths.push(tp);
                            }
                            CurrentFrameKind::Line(i) => {
                                out.graphic_lines[i].text_paths.push(tp);
                            }
                            // Oval / TextFrame don't host TextPath
                            // in the IDML schema; ignore if seen.
                            _ => {}
                        }
                    }
                }
                b"GraphicLine" => {
                    let bounds_attr = attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                    let common = read_common_attrs(&e);
                    let item_transform =
                        effective_item_transform(&group_transforms, common.item_transform);
                    out.graphic_lines.push(GraphicLine {
                        self_id: common.self_id,
                        bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                        item_transform,
                        stroke_color: common.stroke_color,
                        stroke_weight: common.stroke_weight,
                        stroke_type: common.stroke_type,
                        end_join: attr(&e, b"EndJoin"),
                        miter_limit: attr(&e, b"MiterLimit").and_then(|s| s.parse().ok()),
                        stroke_gap_color: common.stroke_gap_color,
                        stroke_gap_tint: common.stroke_gap_tint,
                        stroke_dash: common.stroke_dash,
                        applied_object_style: common.applied_object_style,
                        text_wrap: None,
                        item_layer: common.item_layer,
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        text_paths: Vec::new(),
                        effects: None,
                        overprint_stroke: common.overprint_stroke,
                        nonprinting: common.nonprinting,
                        visible: common.visible,
                        locked: common.locked,
                        start_arrow: attr(&e, b"LeftLineEnd")
                            .map(|s| ArrowheadType::from_idml(&s))
                            .unwrap_or(ArrowheadType::None),
                        end_arrow: attr(&e, b"RightLineEnd")
                            .map(|s| ArrowheadType::from_idml(&s))
                            .unwrap_or(ArrowheadType::None),
                        start_arrow_scale: attr(&e, b"LeftArrowHeadScale")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(100.0),
                        end_arrow_scale: attr(&e, b"RightArrowHeadScale")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(100.0),
                    });
                    let idx = out.graphic_lines.len() - 1;
                    register_with_group(&mut out, &mut group_builders, FrameRef::GraphicLine(idx));
                    current_frame = Some(CurrentFrame {
                        kind: CurrentFrameKind::Line(idx),
                        needs_bounds: bounds_attr.is_none(),
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        // Always retain Bezier path anchors for
                        // graphic lines so a child <TextPath> can
                        // flow text along the actual stroke.
                        keep_anchors: true,
                        in_text_wrap: false,
                        stroke_transparency_depth: 0,
                        content_transparency_depth: 0,
                        in_image_depth: 0,
                        clip: None,
                        in_clipping_path: false,
                    });
                }
                b"Polygon" => {
                    let bounds_attr = attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                    let common = read_common_attrs(&e);
                    let item_transform =
                        effective_item_transform(&group_transforms, common.item_transform);
                    out.polygons.push(Polygon {
                        self_id: common.self_id,
                        bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                        item_transform,
                        fill_color: common.fill_color,
                        fill_tint: common.fill_tint,
                        stroke_color: common.stroke_color,
                        stroke_weight: common.stroke_weight,
                        stroke_type: common.stroke_type,
                        stroke_alignment: attr(&e, b"StrokeAlignment"),
                        end_join: attr(&e, b"EndJoin"),
                        miter_limit: attr(&e, b"MiterLimit").and_then(|s| s.parse().ok()),
                        stroke_gap_color: common.stroke_gap_color,
                        stroke_gap_tint: common.stroke_gap_tint,
                        stroke_dash: common.stroke_dash,
                        applied_object_style: common.applied_object_style,
                        text_wrap: None,
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        item_layer: common.item_layer,
                        gradient_fill_angle: common.gradient_fill_angle,
                        gradient_fill_length: common.gradient_fill_length,
                        gradient_stroke_angle: common.gradient_stroke_angle,
                        gradient_stroke_length: common.gradient_stroke_length,
                        opacity: None,
                        blend_mode: None,
                        text_paths: Vec::new(),
                        image_link: None,
                        image_bytes: None,
                        image_clip: None,
                        has_image_element: false,
                        has_inline_pdf: false,
                        image_item_transform: None,
                        effects: None,
                        overprint_fill: common.overprint_fill,
                        overprint_stroke: common.overprint_stroke,
                        nonprinting: common.nonprinting,
                        visible: common.visible,
                        locked: common.locked,
                    });
                    let idx = out.polygons.len() - 1;
                    register_with_group(&mut out, &mut group_builders, FrameRef::Polygon(idx));
                    current_frame = Some(CurrentFrame {
                        kind: CurrentFrameKind::Polygon(idx),
                        needs_bounds: bounds_attr.is_none(),
                        anchors: Vec::new(),
                        subpath_starts: Vec::new(),
                        subpath_open: Vec::new(),
                        // Always retain Bezier path anchors for
                        // polygons so the renderer can emit a
                        // FillPath instead of a bbox FillRect.
                        keep_anchors: true,
                        in_text_wrap: false,
                        stroke_transparency_depth: 0,
                        content_transparency_depth: 0,
                        in_image_depth: 0,
                        clip: None,
                        in_clipping_path: false,
                    });
                }
                _ => {}
            },
            Event::End(e) => match e.name().as_ref() {
                b"Group" if !group_transforms.is_empty() => {
                    group_transforms.pop();
                    if let Some(builder) = group_builders.pop() {
                        let group = Group {
                            self_id: builder.self_id,
                            item_transform: builder.item_transform,
                            members: builder.members,
                            transparency: builder.transparency,
                        };
                        let group_idx = out.groups.len();
                        out.groups.push(group);
                        // Register this sub-group with the
                        // enclosing group, if any, so the
                        // outer's `members` list captures
                        // sub-groups in document order. Top-level
                        // groups (no outer) surface in
                        // `frames_in_order` so the renderer's
                        // cross-shape z-sort sees them once at
                        // their XML position.
                        if let Some(outer) = group_builders.last_mut() {
                            outer.members.push(FrameRef::Group(group_idx));
                        } else {
                            out.frames_in_order.push(FrameRef::Group(group_idx));
                        }
                    }
                }
                b"TextFrame" | b"Rectangle" | b"Oval" | b"GraphicLine" | b"Polygon" => {
                    // Finalize bounds from accumulated path
                    // anchors when no GeometricBounds attribute
                    // was present. If neither source produced
                    // geometry, drop the placeholder frame so
                    // downstream code never sees a zero-rect
                    // ghost (matches the previous behaviour of
                    // skipping bounds-less shapes).
                    if let Some(cf) = current_frame.take() {
                        // W1.21: flush the placed image's clipping
                        // path onto the host shape. Written before the
                        // bounds/drop logic — if the frame is dropped
                        // (bounds-less ghost) the clip rides along into
                        // the pop. Only Rectangle / Oval / Polygon host
                        // images, so the other kinds carry no field.
                        if let Some(clip) = cf.clip.clone() {
                            match cf.kind {
                                CurrentFrameKind::Rect(i) if i < out.rectangles.len() => {
                                    out.rectangles[i].image_clip = Some(clip);
                                }
                                CurrentFrameKind::Oval(i) if i < out.ovals.len() => {
                                    out.ovals[i].image_clip = Some(clip);
                                }
                                CurrentFrameKind::Polygon(i) if i < out.polygons.len() => {
                                    out.polygons[i].image_clip = Some(clip);
                                }
                                _ => {}
                            }
                        }
                        if cf.needs_bounds {
                            if cf.anchors.is_empty() {
                                drop_pending(&mut out, cf.kind);
                                // The frame was registered with
                                // the open group at open time;
                                // unregister now that it has been
                                // discarded so the group's member
                                // list never points to a stale
                                // frame index.
                                let frame_ref = match cf.kind {
                                    CurrentFrameKind::Text(i) => FrameRef::TextFrame(i),
                                    CurrentFrameKind::Rect(i) => FrameRef::Rectangle(i),
                                    CurrentFrameKind::Oval(i) => FrameRef::Oval(i),
                                    CurrentFrameKind::Line(i) => FrameRef::GraphicLine(i),
                                    CurrentFrameKind::Polygon(i) => FrameRef::Polygon(i),
                                };
                                unregister_last_in_group(&mut out, &mut group_builders, frame_ref);
                            } else {
                                set_pending_bounds(
                                    &mut out,
                                    cf.kind,
                                    bounds_from_anchors(&cf.anchors),
                                );
                            }
                        }
                        // Polygons keep the curved-path data
                        // even when GeometricBounds was set, so
                        // the renderer can rasterise the actual
                        // outline. GraphicLines keep them too so a
                        // child <TextPath> can flow text along the
                        // actual stroke (curved or multi-segment).
                        if cf.keep_anchors && !cf.anchors.is_empty() {
                            // Drop spurious subpath markers — a
                            // subpath start at the very end of
                            // the anchor list points to nothing,
                            // and the canonical single-contour
                            // case is encoded as `[]` (so callers
                            // can keep using the slice as-is).
                            // `subpath_open` stays parallel to
                            // `subpath_starts`, so when we either
                            // empty or shorten the latter we mirror
                            // the truncation here (P-15).
                            let (subpath_starts, subpath_open) = {
                                let mut starts = cf.subpath_starts.clone();
                                let mut opens = cf.subpath_open.clone();
                                // Keep the indices that point at a
                                // real anchor; trim the parallel
                                // open flags by index so the two
                                // arrays stay in step.
                                let mut keep = vec![true; starts.len()];
                                for (k, &s) in starts.iter().enumerate() {
                                    if s >= cf.anchors.len() {
                                        keep[k] = false;
                                    }
                                }
                                let mut filtered_starts = Vec::with_capacity(starts.len());
                                let mut filtered_open = Vec::with_capacity(opens.len());
                                for k in 0..starts.len() {
                                    if keep[k] {
                                        filtered_starts.push(starts[k]);
                                        filtered_open.push(opens.get(k).copied().unwrap_or(false));
                                    }
                                }
                                starts = filtered_starts;
                                opens = filtered_open;
                                if starts.len() <= 1 {
                                    // The legacy canonical form for
                                    // a single contour. Surface the
                                    // open flag onto a 1-element vec
                                    // so the renderer can still see
                                    // an open single contour.
                                    let lone_open = opens.first().copied().unwrap_or(false);
                                    if lone_open {
                                        (Vec::new(), vec![true])
                                    } else {
                                        (Vec::new(), Vec::new())
                                    }
                                } else {
                                    (starts, opens)
                                }
                            };
                            match cf.kind {
                                CurrentFrameKind::Polygon(i) if i < out.polygons.len() => {
                                    out.polygons[i].anchors = cf.anchors;
                                    out.polygons[i].subpath_starts = subpath_starts;
                                    out.polygons[i].subpath_open = subpath_open;
                                }
                                CurrentFrameKind::Line(i) if i < out.graphic_lines.len() => {
                                    out.graphic_lines[i].anchors = cf.anchors;
                                    out.graphic_lines[i].subpath_starts = subpath_starts;
                                    out.graphic_lines[i].subpath_open = subpath_open;
                                }
                                CurrentFrameKind::Text(i) if i < out.text_frames.len() => {
                                    out.text_frames[i].anchors = cf.anchors;
                                    out.text_frames[i].subpath_starts = subpath_starts;
                                    out.text_frames[i].subpath_open = subpath_open;
                                }
                                CurrentFrameKind::Rect(i) if i < out.rectangles.len() => {
                                    // Q-11: only stash when the
                                    // outline is non-rectangular
                                    // (>4 anchors). A plain 4-corner
                                    // AABB is the existing default
                                    // and skipping the stash here
                                    // keeps `from_rectangle`'s
                                    // legacy `Geometry::Rect` path.
                                    if cf.anchors.len() > 4 {
                                        out.rectangles[i].anchors = cf.anchors;
                                        out.rectangles[i].subpath_starts = subpath_starts;
                                        out.rectangles[i].subpath_open = subpath_open;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                b"TextWrapPreference" => {
                    if let Some(cf) = current_frame.as_mut() {
                        cf.in_text_wrap = false;
                    }
                }
                b"ClippingPathSettings" => {
                    // W1.21: clip geometry capture ends here; the
                    // pending `clip` is flushed onto the host shape at
                    // frame close.
                    if let Some(cf) = current_frame.as_mut() {
                        cf.in_clipping_path = false;
                    }
                }
                b"Image" | b"EPSImage" | b"ImportedPage" | b"PDF" => {
                    // W1.21: leaving the image container restores
                    // frame-anchor accumulation. Saturating so a
                    // malformed/mismatched stream can't underflow.
                    if let Some(cf) = current_frame.as_mut() {
                        cf.in_image_depth = cf.in_image_depth.saturating_sub(1);
                    }
                }
                b"StrokeTransparencySetting" => {
                    if let Some(cf) = current_frame.as_mut() {
                        if cf.stroke_transparency_depth > 0 {
                            cf.stroke_transparency_depth -= 1;
                        }
                    } else if let Some(b) = group_builders.last_mut() {
                        if b.stroke_transparency_depth > 0 {
                            b.stroke_transparency_depth -= 1;
                        }
                    }
                }
                b"ContentTransparencySetting" => {
                    if let Some(cf) = current_frame.as_mut() {
                        if cf.content_transparency_depth > 0 {
                            cf.content_transparency_depth -= 1;
                        }
                    } else if let Some(b) = group_builders.last_mut() {
                        if b.content_transparency_depth > 0 {
                            b.content_transparency_depth -= 1;
                        }
                    }
                }
                b"GradientFeatherSetting" => {
                    // Close the gradient-feather scope so any
                    // later `<GradientStop>` (e.g. inside a
                    // `<Gradient>` swatch parsed in graphic.rs
                    // — different file, but defensive here)
                    // doesn't accidentally route to this rect.
                    current_gradient_feather = None;
                }
                b"Contents" => {
                    // Q-03: close the inline-image base64 capture.
                    // Decode and stash on the parent shape; clear
                    // state so a later sibling can't accidentally
                    // route into the same buffer.
                    if let Some(kind) = current_image_contents_target.take() {
                        let decoded = decode_image_contents_base64(&current_contents_buf);
                        current_contents_buf.clear();
                        if let Some(bytes) = decoded {
                            set_image_bytes(&mut out, kind, bytes);
                        }
                    }
                }
                _ => {}
            },
            Event::Text(t) if current_image_contents_target.is_some() => {
                // base64 CDATA can also arrive as Text events
                // (whitespace-padded between tags). Trim during
                // decode rather than at capture time.
                current_contents_buf.extend_from_slice(t.as_ref());
            }
            Event::CData(t) if current_image_contents_target.is_some() => {
                current_contents_buf.extend_from_slice(t.as_ref());
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn parse_bounds(s: &str) -> Option<Bounds> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Bounds {
        top: parts[0],
        left: parts[1],
        bottom: parts[2],
        right: parts[3],
    })
}

/// Parse an "x y" pair from an IDML attribute (Anchor /
/// LeftDirection / RightDirection / etc.). IDML serialises 2D
/// coordinates as two whitespace-separated f32s.
fn parse_xy_pair(s: &str) -> Option<(f32, f32)> {
    let mut it = s.split_whitespace();
    let x: f32 = it.next()?.parse().ok()?;
    let y: f32 = it.next()?.parse().ok()?;
    Some((x, y))
}

fn parse_drop_shadow(e: &quick_xml::events::BytesStart) -> Option<DropShadowSetting> {
    // IDML defaults — see §IDML Defaults Table 84 in the spec:
    // Mode=None, BlendMode=Multiply, Opacity=75, XOffset=7, YOffset=7,
    // Size=5, EffectColor="n" (Black). When `Mode="Drop"` is the only
    // attribute on the element, these are the values InDesign uses for
    // the unspecified ones. Earlier behaviour treated missing offsets
    // / size as zero, which produced a solid black stamp behind the
    // frame instead of a real drop shadow.
    Some(DropShadowSetting {
        mode: attr(e, b"Mode").unwrap_or_else(|| "Drop".to_string()),
        x_offset: attr(e, b"XOffset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(7.0),
        y_offset: attr(e, b"YOffset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(7.0),
        size: attr(e, b"Size").and_then(|s| s.parse().ok()).unwrap_or(5.0),
        opacity_pct: attr(e, b"Opacity")
            .and_then(|s| s.parse().ok())
            .unwrap_or(75.0),
        effect_color: attr(e, b"EffectColor"),
    })
}

/// IDML's `InsetSpacing` is four whitespace-separated numbers in
/// pt — top, left, bottom, right. Returns `None` if the count is
/// off; the renderer falls back to zero insets.
fn parse_insets(s: &str) -> Option<[f32; 4]> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    (parts.len() == 4).then(|| [parts[0], parts[1], parts[2], parts[3]])
}

/// Q-03: decode the base64 CDATA payload of `<Image><Properties>
/// <Contents>` into the original image bytes. The CDATA is standard
/// RFC 4648 base64 with arbitrary whitespace (newlines, spaces) so
/// strip those before decoding. Returns `None` on malformed input
/// rather than panicking — the caller falls back to "no inline
/// bytes" and the renderer's missing-image path takes over.
fn decode_image_contents_base64(raw: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine;
    // Strip whitespace in place into a scratch buffer. The XML
    // serializer pretty-prints the base64 payload across many lines
    // (typically 76-char wraps); base64's STANDARD engine rejects
    // any whitespace, so we have to clean first.
    let mut cleaned: Vec<u8> = Vec::with_capacity(raw.len());
    for &b in raw {
        if !matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
            cleaned.push(b);
        }
    }
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .ok()
}

/// Q-04: borrow the effects bag slot for any frame kind. Returns
/// `None` only when the kind's index is out of bounds (defensive —
/// the parser shouldn't reach this state). Centralises the per-shape
/// dispatch so the effect-routing block doesn't fan into five copies.
fn effects_slot_mut(out: &mut Spread, kind: CurrentFrameKind) -> Option<&mut Option<FrameEffects>> {
    match kind {
        CurrentFrameKind::Text(i) => out.text_frames.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Rect(i) => out.rectangles.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Oval(i) => out.ovals.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Line(i) => out.graphic_lines.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Polygon(i) => out.polygons.get_mut(i).map(|f| &mut f.effects),
    }
}

/// Q-03: stash decoded image bytes on the frame the just-closed
/// `<Contents>` element was nested under. Centralised here so the
/// per-shape match doesn't clutter the parser's main loop.
fn set_image_bytes(out: &mut Spread, kind: CurrentFrameKind, bytes: Vec<u8>) {
    match kind {
        CurrentFrameKind::Rect(i) if i < out.rectangles.len() => {
            out.rectangles[i].image_bytes = Some(bytes);
        }
        CurrentFrameKind::Oval(i) if i < out.ovals.len() => {
            out.ovals[i].image_bytes = Some(bytes);
        }
        CurrentFrameKind::Polygon(i) if i < out.polygons.len() => {
            out.polygons[i].image_bytes = Some(bytes);
        }
        // TextFrame / GraphicLine don't carry image_bytes — IDML
        // doesn't put `<Image>` children under them.
        _ => {}
    }
}

fn parse_matrix(s: &str) -> Option<[f32; 6]> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 6 {
        return None;
    }
    Some([parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]])
}

/// Parse the x-component of an IDML ppi tuple. InDesign writes
/// `ActualPpi`/`EffectivePpi` as `"(x y)"` (parenthesised, two
/// space-separated numbers); square pixels are near-universal so the
/// x-resolution is representative. Also tolerates a bare `"x"` for
/// synthetic inputs. `None` when nothing parses.
fn parse_ppi_x(s: &str) -> Option<f32> {
    s.trim_matches(|c| c == '(' || c == ')' || c == ' ')
        .split_whitespace()
        .next()
        .and_then(|p| p.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// Build [`ImageMetadata`] from a nested `<Image>` element's
/// `Space` / `ActualPpi` / `EffectivePpi` attributes. Returns `None`
/// when the element carries none of them (so a plain placement
/// doesn't allocate an empty record). `Space` is stripped of its
/// `$ID/` namespace prefix.
fn read_image_metadata(e: &quick_xml::events::BytesStart) -> Option<ImageMetadata> {
    let space = attr(e, b"Space").map(|s| s.strip_prefix("$ID/").unwrap_or(&s).to_string());
    let actual_ppi = attr(e, b"ActualPpi").as_deref().and_then(parse_ppi_x);
    let effective_ppi = attr(e, b"EffectivePpi").as_deref().and_then(parse_ppi_x);
    if space.is_none() && actual_ppi.is_none() && effective_ppi.is_none() {
        return None;
    }
    Some(ImageMetadata {
        space,
        actual_ppi,
        effective_ppi,
    })
}

/// Record placed-image metadata for the host frame `self_id` into the
/// spread's side map. First-write-wins (matching `image_link`) and a
/// no-op when the frame has no `Self` id or the `<Image>` carries no
/// colour-space / ppi attributes.
fn record_image_metadata(
    out: &mut Spread,
    host_self_id: Option<String>,
    e: &quick_xml::events::BytesStart,
) {
    let Some(host) = host_self_id else { return };
    if out.image_metadata.contains_key(&host) {
        return;
    }
    if let Some(meta) = read_image_metadata(e) {
        out.image_metadata.insert(host, meta);
    }
}

/// Compose two affine matrices `a ∘ b`: applying the result to a
/// point is equivalent to applying `b` first then `a`. Matches
/// `paged_compose::Transform::compose` so the parser and the
/// renderer agree on composition order.
fn compose_matrix(a: &[f32; 6], b: &[f32; 6]) -> [f32; 6] {
    let [a1, b1, c1, d1, tx1, ty1] = *a;
    let [a2, b2, c2, d2, tx2, ty2] = *b;
    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * tx2 + c1 * ty2 + tx1,
        b1 * tx2 + d1 * ty2 + ty1,
    ]
}

/// Resolve the effective `ItemTransform` for a frame nested inside
/// zero or more groups: outer groups apply first, then inner groups,
/// then the frame's own ItemTransform. `None` for every input
/// short-circuits to `None` so axis-aligned frames keep an empty
/// transform field.
fn effective_item_transform(
    group_stack: &[Option<[f32; 6]>],
    own: Option<[f32; 6]>,
) -> Option<[f32; 6]> {
    let mut acc: Option<[f32; 6]> = None;
    for g in group_stack {
        match (acc, g) {
            (None, Some(m)) => acc = Some(*m),
            (Some(a), Some(m)) => acc = Some(compose_matrix(&a, m)),
            (acc_, None) => acc = acc_,
        }
    }
    match (acc, own) {
        (None, x) => x,
        (Some(a), None) => Some(a),
        (Some(a), Some(o)) => Some(compose_matrix(&a, &o)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_PAGE_SPREAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="spread1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Page Self="p2" GeometricBounds="0 612 792 1224"/>
    <TextFrame Self="frame1" ParentStory="u10"
               GeometricBounds="72 72 720 540"
               ItemTransform="1 0 0 1 0 0"/>
    <TextFrame Self="frame2" ParentStory="u20"
               GeometricBounds="100 700 300 1100"/>
  </Spread>
</idPkg:Spread>"#;

    #[test]
    fn parses_graphic_line_line_ends() {
        // v43 — `LeftLineEnd` / `RightLineEnd` carry InDesign's
        // `ArrowHead` enumeration tokens; the scales ride alongside.
        // Unknown-but-present names become `Other`; absent = `None`.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="spread1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <GraphicLine Self="gl1" GeometricBounds="10 10 110 210" StrokeWeight="2"
                 LeftLineEnd="CircleSolidArrowHead" RightLineEnd="TriangleArrowHead"
                 LeftArrowHeadScale="150" RightArrowHeadScale="75"/>
    <GraphicLine Self="gl2" GeometricBounds="10 220 110 420"
                 RightLineEnd="NotARealHead"/>
    <GraphicLine Self="gl3" GeometricBounds="10 430 110 630"/>
  </Spread>
</idPkg:Spread>"#;
        let s = parse_spread(xml.as_bytes()).unwrap();
        assert_eq!(s.graphic_lines.len(), 3);
        let gl1 = &s.graphic_lines[0];
        assert_eq!(gl1.start_arrow, ArrowheadType::CircleSolid);
        assert_eq!(gl1.end_arrow, ArrowheadType::Triangle);
        assert!((gl1.start_arrow_scale - 150.0).abs() < 1e-3);
        assert!((gl1.end_arrow_scale - 75.0).abs() < 1e-3);
        // The typed value round-trips to the canonical token.
        assert_eq!(gl1.end_arrow.as_idml(), "TriangleArrowHead");
        assert_eq!(
            ArrowheadType::from_idml(gl1.start_arrow.as_idml()),
            gl1.start_arrow
        );
        let gl2 = &s.graphic_lines[1];
        assert_eq!(gl2.start_arrow, ArrowheadType::None);
        assert_eq!(gl2.end_arrow, ArrowheadType::Other);
        let gl3 = &s.graphic_lines[2];
        assert_eq!(gl3.start_arrow, ArrowheadType::None);
        assert_eq!(gl3.end_arrow, ArrowheadType::None);
    }

    /// Every drawable variant's canonical token must survive
    /// `as_idml` → `from_idml` unchanged — paged-write and the mutate
    /// inverses rely on the bijection.
    #[test]
    fn arrowhead_vocabulary_round_trips() {
        use ArrowheadType as A;
        for t in [
            A::None,
            A::Simple,
            A::SimpleWide,
            A::Triangle,
            A::TriangleWide,
            A::Barbed,
            A::Curved,
            A::Circle,
            A::CircleSolid,
            A::Square,
            A::SquareSolid,
            A::Bar,
        ] {
            assert_eq!(A::from_idml(t.as_idml()), t, "{t:?}");
        }
        // `Other` is the one non-representable variant.
        assert_eq!(A::Other.as_idml(), "");
    }

    #[test]
    fn parses_ruler_guides() {
        // Mix of vertical + horizontal guides on a 2-page spread.
        // Both `<Guide>` and self-closing variants accepted.
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="spread1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Guide Self="g1" Orientation="Vertical" Location="120.5" PageIndex="0"/>
    <Guide Self="g2" Orientation="Horizontal" Location="240" PageIndex="1"/>
    <Guide Self="g3" Orientation="Bogus" Location="50"/>
  </Spread>
</idPkg:Spread>"#;
        let s = parse_spread(xml.as_bytes()).unwrap();
        assert_eq!(s.guides.len(), 2, "bogus orientation should be dropped");
        assert!(matches!(
            s.guides[0].orientation,
            GuideOrientation::Vertical
        ));
        assert!((s.guides[0].location - 120.5).abs() < 1e-3);
        assert_eq!(s.guides[0].page_index, 0);
        assert!(matches!(
            s.guides[1].orientation,
            GuideOrientation::Horizontal
        ));
        assert!((s.guides[1].location - 240.0).abs() < 1e-3);
        assert_eq!(s.guides[1].page_index, 1);
    }

    #[test]
    fn parses_labels_into_the_side_map() {
        // `Properties/Label` KeyValuePairs — the plugin-metadata
        // carrier. Attribute values are XML-unescaped; duplicate keys
        // collapse last-write-wins; items without labels stay out of
        // the map.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="spread1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Rectangle Self="u100" GeometricBounds="10 10 110 210" ItemTransform="1 0 0 1 0 0">
      <Properties>
        <Label>
          <KeyValuePair Key="x-paged:web" Value="{&quot;v&quot;:1,&quot;data&quot;:{}}"/>
          <KeyValuePair Key="vendor" Value="acme"/>
          <KeyValuePair Key="vendor" Value="acme2"/>
        </Label>
      </Properties>
    </Rectangle>
    <Rectangle Self="u200" GeometricBounds="10 220 110 420" ItemTransform="1 0 0 1 0 0"/>
  </Spread>
</idPkg:Spread>"#;
        let s = parse_spread(xml.as_bytes()).unwrap();
        let labels = s.labels.get("u100").expect("u100 labelled");
        assert_eq!(
            labels,
            &vec![
                (
                    "x-paged:web".to_string(),
                    "{\"v\":1,\"data\":{}}".to_string()
                ),
                ("vendor".to_string(), "acme2".to_string()),
            ]
        );
        assert!(!s.labels.contains_key("u200"));
    }

    #[test]
    fn parses_pages_and_frames() {
        let s = parse_spread(TWO_PAGE_SPREAD).unwrap();
        assert_eq!(s.self_id.as_deref(), Some("spread1"));
        assert_eq!(s.pages.len(), 2);
        assert_eq!(s.pages[0].self_id.as_deref(), Some("p1"));
        assert_eq!(s.pages[0].bounds.width(), 612.0);
        assert_eq!(s.pages[0].bounds.height(), 792.0);

        assert_eq!(s.text_frames.len(), 2);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("frame1"));
        assert_eq!(s.text_frames[0].parent_story.as_deref(), Some("u10"));
        assert_eq!(s.text_frames[0].bounds.width(), 468.0);
        assert_eq!(s.text_frames[0].bounds.height(), 648.0);
        assert_eq!(
            s.text_frames[0].item_transform,
            Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        );
        assert_eq!(s.text_frames[1].item_transform, None);
    }

    #[test]
    fn parses_show_master_items_flag() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Page Self="p1" GeometricBounds="0 0 792 612" ShowMasterItems="false"/>
            <Page Self="p2" GeometricBounds="0 0 792 612" ShowMasterItems="true"/>
            <Page Self="p3" GeometricBounds="0 0 792 612"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.pages.len(), 3);
        assert_eq!(s.pages[0].show_master_items, Some(false));
        assert_eq!(s.pages[1].show_master_items, Some(true));
        assert_eq!(
            s.pages[2].show_master_items, None,
            "absent ⇒ stamp as usual"
        );
    }

    #[test]
    fn lifts_frames_out_of_groups_with_composed_transform() {
        // Two levels of nesting: outer group translates by (10, 20),
        // inner group translates by (3, 4), inner frame has its own
        // ItemTransform translating by (100, 200). Expected effective
        // transform: outer ∘ inner ∘ frame = translate(113, 224).
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="top" ParentStory="u1" GeometricBounds="0 0 100 200"/>
            <Group ItemTransform="1 0 0 1 10 20">
              <Group ItemTransform="1 0 0 1 3 4">
                <TextFrame Self="inner" ParentStory="u2"
                           GeometricBounds="0 0 50 50"
                           ItemTransform="1 0 0 1 100 200"/>
              </Group>
            </Group>
            <TextFrame Self="after" ParentStory="u3" GeometricBounds="0 0 100 200"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 3, "all frames lifted out of groups");
        assert_eq!(s.skipped_nested_frames, 0);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("top"));
        assert_eq!(s.text_frames[1].self_id.as_deref(), Some("inner"));
        assert_eq!(s.text_frames[2].self_id.as_deref(), Some("after"));
        // outer translation (10, 20) + inner translation (3, 4) +
        // frame's own (100, 200) = translation (113, 224); the linear
        // part stays identity since every transform is pure
        // translation.
        let m = s.text_frames[1].item_transform.expect("composed");
        assert!((m[0] - 1.0).abs() < 1e-4 && (m[3] - 1.0).abs() < 1e-4);
        assert!(m[1].abs() < 1e-4 && m[2].abs() < 1e-4);
        assert!((m[4] - 113.0).abs() < 1e-4, "tx = {}", m[4]);
        assert!((m[5] - 224.0).abs() < 1e-4, "ty = {}", m[5]);
    }

    #[test]
    fn parses_text_frame_preference_inset_and_first_baseline() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 200 300">
              <Properties/>
              <TextFramePreference VerticalJustification="CenterAlign"
                                   FirstBaselineOffset="CapHeight"
                                   MinimumFirstBaselineOffset="14"
                                   InsetSpacing="6 8 10 12"/>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let f = &s.text_frames[0];
        assert_eq!(
            f.vertical_justification,
            Some(VerticalJustification::Center)
        );
        assert_eq!(
            f.first_baseline_offset,
            Some(FirstBaselineOffset::CapHeight)
        );
        assert_eq!(f.minimum_first_baseline_offset, Some(14.0));
        assert_eq!(f.inset_spacing, Some([6.0, 8.0, 10.0, 12.0]));
    }

    // W0.3 — text-frame column prefs + balance.
    #[test]
    fn w03_parses_text_frame_columns() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 200 300">
              <TextFramePreference TextColumnCount="3" TextColumnGutter="14"
                                   VerticalBalanceColumns="true"/>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let f = &s.text_frames[0];
        assert_eq!(f.column_count, Some(3));
        assert_eq!(f.column_gutter, Some(14.0));
        assert_eq!(f.column_balance, Some(true));
    }

    // W0.3 — stroke gap colour/tint + text-wrap invert + frame-fitting
    // alignment/auto-fit + overprint, on a Rectangle.
    #[test]
    fn w03_parses_stroke_gap_wrap_invert_and_fitting() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r" GeometricBounds="0 0 100 200"
                       StrokeType="StrokeStyle/$ID/Dashed"
                       GapColor="Color/Cyan" GapTint="60"
                       OverprintFill="true" OverprintStroke="true">
              <TextWrapPreference TextWrapMode="ContourTextWrap" Inverse="true">
                <TextWrapOffset Top="1" Left="2" Bottom="3" Right="4"/>
              </TextWrapPreference>
              <FrameFittingOption LeftCrop="-5" FittingOnEmptyFrame="FillProportionally"
                                  FittingAlignment="CenterPoint" AutoFit="true"/>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let r = &s.rectangles[0];
        assert_eq!(r.stroke_gap_color.as_deref(), Some("Color/Cyan"));
        assert_eq!(r.stroke_gap_tint, Some(60.0));
        assert!(r.overprint_fill);
        assert!(r.overprint_stroke);
        let tw = r.text_wrap.expect("text wrap parsed");
        assert_eq!(tw.invert, Some(true));
        assert_eq!(tw.offsets, [1.0, 2.0, 3.0, 4.0]);
        let ff = r.frame_fitting.as_ref().expect("frame fitting parsed");
        assert_eq!(ff.reference_point.as_deref(), Some("CenterPoint"));
        assert_eq!(ff.auto_fit, Some(true));
    }

    #[test]
    fn q16_parses_per_corner_options() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r" GeometricBounds="0 0 100 200"
                       CornerOption="RoundedCorner" CornerRadius="0"
                       TopLeftCornerOption="None" TopLeftCornerRadius="0"
                       TopRightCornerOption="None" TopRightCornerRadius="0"
                       BottomRightCornerOption="None" BottomRightCornerRadius="0"
                       BottomLeftCornerOption="RoundedCorner" BottomLeftCornerRadius="19.84"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let r = &s.rectangles[0];
        // Top-left + top-right + bottom-right squared off explicitly.
        assert_eq!(r.corners[0].option, Some(CornerOption::None));
        assert_eq!(r.corners[1].option, Some(CornerOption::None));
        assert_eq!(r.corners[2].option, Some(CornerOption::None));
        // Bottom-left rounded with explicit radius.
        assert_eq!(r.corners[3].option, Some(CornerOption::Rounded));
        assert_eq!(r.corners[3].radius, Some(19.84));
        assert!(r.corners[3].option.unwrap().rounds());
    }

    #[test]
    fn q03_parses_inline_image_contents_base64() {
        // "Hello, IDML!" base64-encoded → the bytes round-trip.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 50 50">
              <Image>
                <Properties>
                  <Contents><![CDATA[SGVsbG8sIElETUwh]]></Contents>
                </Properties>
              </Image>
            </Rectangle>
            <Rectangle Self="r2" GeometricBounds="0 0 50 50">
              <Image LinkResourceURI="file:///link.jpg"/>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.rectangles.len(), 2);
        let r1 = &s.rectangles[0];
        assert_eq!(
            r1.image_bytes.as_deref(),
            Some(b"Hello, IDML!" as &[u8]),
            "inline CDATA should base64-decode and stash on the rect",
        );
        assert!(
            r1.has_image_element,
            "rect should still flag has_image_element"
        );
        let r2 = &s.rectangles[1];
        assert!(
            r2.image_bytes.is_none(),
            "link-only rect carries no inline bytes"
        );
        assert_eq!(r2.image_link.as_deref(), Some("file:///link.jpg"));
    }

    #[test]
    fn q03_decodes_whitespace_padded_base64() {
        // InDesign's serializer wraps base64 at ~76 chars with
        // surrounding whitespace; verify the decoder strips it.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r" GeometricBounds="0 0 1 1">
              <Image><Properties><Contents><![CDATA[
                SGVsbG8s
                IElETUwh
              ]]></Contents></Properties></Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(
            s.rectangles[0].image_bytes.as_deref(),
            Some(b"Hello, IDML!" as &[u8]),
        );
    }

    #[test]
    fn q02_parses_text_frame_preference_auto_sizing() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 50 30">
              <Properties/>
              <TextFramePreference AutoSizingType="WidthOnly"
                                   AutoSizingReferencePoint="TopLeftPoint"
                                   MinimumWidthForAutoSizing="40"
                                   MinimumHeightForAutoSizing="20"
                                   UseMinimumHeightForAutoSizing="true"/>
            </TextFrame>
            <TextFrame Self="frameB" ParentStory="u2" GeometricBounds="0 0 100 100">
              <Properties/>
              <TextFramePreference AutoSizingType="HeightAndWidth"/>
            </TextFrame>
            <TextFrame Self="frameC" ParentStory="u3" GeometricBounds="0 0 100 100">
              <Properties/>
              <TextFramePreference VerticalJustification="TopAlign"/>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let a = &s.text_frames[0];
        assert_eq!(a.auto_sizing, Some(AutoSizingType::WidthOnly));
        assert!(a.auto_sizing.unwrap().grows_width());
        assert!(!a.auto_sizing.unwrap().grows_height());
        assert_eq!(
            a.auto_sizing_reference_point,
            Some(AutoSizingReferencePoint::TopLeftPoint)
        );
        assert_eq!(a.minimum_width_for_auto_sizing, Some(40.0));
        assert_eq!(a.minimum_height_for_auto_sizing, Some(20.0));
        assert_eq!(a.use_minimum_height_for_auto_sizing, Some(true));

        let b = &s.text_frames[1];
        assert_eq!(b.auto_sizing, Some(AutoSizingType::HeightAndWidth));
        assert!(b.auto_sizing.unwrap().grows_width());
        assert!(b.auto_sizing.unwrap().grows_height());

        let c = &s.text_frames[2];
        assert!(c.auto_sizing.is_none(), "frameC has no AutoSizingType");
    }

    #[test]
    fn parses_applied_toc_style_on_text_frame() {
        // TOC-host TextFrames carry `AppliedTOCStyle="TOCStyle/<id>"`
        // so the renderer can swap the unresolved story's paragraphs
        // for the resolver's output.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 100 200"
                       AppliedTOCStyle="TOCStyle/Main"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(
            s.text_frames[0].applied_toc_style.as_deref(),
            Some("TOCStyle/Main")
        );
    }

    #[test]
    fn parses_next_text_frame_link() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1"
                       GeometricBounds="0 0 100 100"
                       NextTextFrame="frameB"/>
            <TextFrame Self="frameB" ParentStory="u1"
                       GeometricBounds="120 0 220 100"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 2);
        assert_eq!(s.text_frames[0].next_text_frame.as_deref(), Some("frameB"));
        assert!(s.text_frames[1].next_text_frame.is_none());
    }

    #[test]
    fn group_without_item_transform_passes_child_through() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group>
              <TextFrame Self="inner" ParentStory="u1" GeometricBounds="0 0 50 50"/>
            </Group>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        assert!(
            s.text_frames[0].item_transform.is_none(),
            "no group transform + no own transform → None"
        );
    }

    #[test]
    fn parses_rectangles_alongside_text_frames() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="t1" ParentStory="u1" GeometricBounds="0 0 100 200"/>
            <Rectangle Self="r1" GeometricBounds="10 10 90 190"
                       FillColor="Color/Blue" StrokeColor="Color/Black"
                       StrokeWeight="1.5"/>
            <Rectangle Self="r2" GeometricBounds="200 200 300 300"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        assert_eq!(s.rectangles.len(), 2);
        assert_eq!(s.rectangles[0].self_id.as_deref(), Some("r1"));
        assert_eq!(s.rectangles[0].fill_color.as_deref(), Some("Color/Blue"));
        assert_eq!(s.rectangles[0].stroke_weight, Some(1.5));
        assert_eq!(s.rectangles[1].fill_color, None);
    }

    #[test]
    fn parses_gradient_fill_and_stroke_angle_length() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 200"
                       FillColor="Gradient/Sky" StrokeColor="Gradient/Sun"
                       GradientFillAngle="45" GradientFillLength="120"
                       GradientStrokeAngle="-30" GradientStrokeLength="80"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let r = &s.rectangles[0];
        assert_eq!(r.gradient_fill_angle, Some(45.0));
        assert_eq!(r.gradient_fill_length, Some(120.0));
        assert_eq!(r.gradient_stroke_angle, Some(-30.0));
        assert_eq!(r.gradient_stroke_length, Some(80.0));
    }

    #[test]
    fn parses_drop_shadow_inside_text_frame_properties() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <TransparencySetting>
                  <DropShadowSetting Mode="Drop" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50" EffectColor="Color/Black"/>
                </TransparencySetting>
              </Properties>
            </TextFrame>
            <Rectangle Self="rect1" GeometricBounds="0 0 50 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        let shadow = s.text_frames[0]
            .drop_shadow
            .as_ref()
            .expect("drop shadow parsed");
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 3.0);
        assert_eq!(shadow.y_offset, 3.0);
        assert_eq!(shadow.size, 6.0);
        assert_eq!(shadow.opacity_pct, 50.0);
        assert_eq!(shadow.effect_color.as_deref(), Some("Color/Black"));
        // Plain rectangle without shadow stays None.
        assert_eq!(s.rectangles.len(), 1);
        assert!(s.rectangles[0].drop_shadow.is_none());
    }

    #[test]
    fn drop_shadow_under_stroke_transparency_lands_in_stroke_field() {
        // <StrokeTransparencySetting><DropShadowSetting/> → captured
        // as `stroke_drop_shadow`, not `drop_shadow`. Renderer gates
        // emission on stroke visibility downstream.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <StrokeTransparencySetting>
                  <DropShadowSetting Mode="Drop" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50" EffectColor="Color/Black"/>
                </StrokeTransparencySetting>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert!(s.text_frames[0].drop_shadow.is_none());
        let shadow = s.text_frames[0]
            .stroke_drop_shadow
            .as_ref()
            .expect("stroke drop shadow parsed");
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 3.0);
    }

    #[test]
    fn drop_shadow_under_content_transparency_is_skipped() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <ContentTransparencySetting>
                  <DropShadowSetting Mode="Drop" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50" EffectColor="Color/Black"/>
                </ContentTransparencySetting>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert!(s.text_frames[0].drop_shadow.is_none());
        assert!(s.text_frames[0].stroke_drop_shadow.is_none());
    }

    #[test]
    fn drop_shadow_with_mode_none_is_skipped() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="f1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <TransparencySetting>
                  <DropShadowSetting Mode="None" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50"/>
                </TransparencySetting>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert!(s.text_frames[0].drop_shadow.is_none());
    }

    #[test]
    fn ignores_malformed_bounds() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Page Self="bad" GeometricBounds="0 0 bogus"/>
            <Page Self="good" GeometricBounds="0 0 100 200"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.pages.len(), 1);
        assert_eq!(s.pages[0].self_id.as_deref(), Some("good"));
    }

    /// Real-world IDMLs almost never serialise `GeometricBounds` on
    /// shape elements; geometry lives in `<Properties><PathGeometry>`
    /// instead. The parser must derive the bounds from the path
    /// anchors so InDesign exports populate frames at all.
    #[test]
    fn text_frame_bounds_come_from_path_geometry_when_attribute_absent() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1"
                       ItemTransform="1 0 0 1 0 0">
              <Properties>
                <PathGeometry>
                  <GeometryPathType PathOpen="false">
                    <PathPointArray>
                      <PathPointType Anchor="-100 -50"
                                     LeftDirection="-100 -50"
                                     RightDirection="-100 -50"/>
                      <PathPointType Anchor="-100  150"
                                     LeftDirection="-100  150"
                                     RightDirection="-100  150"/>
                      <PathPointType Anchor=" 200  150"
                                     LeftDirection=" 200  150"
                                     RightDirection=" 200  150"/>
                      <PathPointType Anchor=" 200 -50"
                                     LeftDirection=" 200 -50"
                                     RightDirection=" 200 -50"/>
                    </PathPointArray>
                  </GeometryPathType>
                </PathGeometry>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1, "frame must survive without GB");
        let f = &s.text_frames[0];
        // Bounding box of (-100,-50) and (200,150) → top=-50, left=-100,
        // bottom=150, right=200.
        assert_eq!(f.bounds.top, -50.0);
        assert_eq!(f.bounds.left, -100.0);
        assert_eq!(f.bounds.bottom, 150.0);
        assert_eq!(f.bounds.right, 200.0);
        assert_eq!(f.bounds.width(), 300.0);
        assert_eq!(f.bounds.height(), 200.0);
    }

    #[test]
    fn rectangle_oval_and_graphic_line_also_derive_bounds_from_path() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="0 0"/>
                  <PathPointType Anchor="40 60"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </Rectangle>
            <Oval Self="o1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="-5 -5"/>
                  <PathPointType Anchor="15 25"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </Oval>
            <GraphicLine Self="l1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="0 100"/>
                  <PathPointType Anchor="200 100"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </GraphicLine>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.rectangles.len(), 1);
        assert_eq!(s.rectangles[0].bounds.width(), 40.0);
        assert_eq!(s.rectangles[0].bounds.height(), 60.0);
        assert_eq!(s.ovals.len(), 1);
        assert_eq!(s.ovals[0].bounds.width(), 20.0);
        assert_eq!(s.ovals[0].bounds.height(), 30.0);
        assert_eq!(s.graphic_lines.len(), 1);
        assert_eq!(s.graphic_lines[0].bounds.width(), 200.0);
        // Degenerate-height line still produces a frame so downstream
        // can render it as a stroke between the two anchors.
        assert_eq!(s.graphic_lines[0].bounds.height(), 0.0);
    }

    #[test]
    fn frame_with_neither_bounds_attribute_nor_path_is_dropped() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="lost" ParentStory="u1">
              <Properties/>
            </TextFrame>
            <TextFrame Self="kept" ParentStory="u2" GeometricBounds="0 0 50 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("kept"));
    }

    /// The CS5+ multi-page-size feature places each `<Page>` in the
    /// spread via its own ItemTransform. Previously we ignored the
    /// attribute, which made every real-world IDML page route frames
    /// to (0, 0) of spread coords and miss every page after the
    /// first. Capture both the attribute extraction and the
    /// translation-only common case here.
    #[test]
    fn page_carries_item_transform_for_multi_page_spreads() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Page Self="left"
                  GeometricBounds="0 0 792 612"
                  ItemTransform="1 0 0 1 -612 -396"/>
            <Page Self="right"
                  GeometricBounds="0 0 792 612"
                  ItemTransform="1 0 0 1 0 -396"/>
            <Page Self="legacy" GeometricBounds="0 0 792 612"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.pages.len(), 3);
        assert_eq!(
            s.pages[0].item_transform,
            Some([1.0, 0.0, 0.0, 1.0, -612.0, -396.0]),
            "left page's ItemTransform translates by (-612, -396)",
        );
        assert_eq!(
            s.pages[1].item_transform,
            Some([1.0, 0.0, 0.0, 1.0, 0.0, -396.0]),
            "right page's ItemTransform translates by (0, -396)",
        );
        assert_eq!(
            s.pages[2].item_transform, None,
            "legacy page without the attribute reads as identity",
        );
    }

    #[test]
    fn geometric_bounds_attribute_wins_over_path_geometry_when_both_present() {
        // Defensive: when both shapes carry geometry, the attribute
        // is the authoritative source (it's what InDesign writes when
        // emitting a synthetic element). PathGeometry should not
        // overwrite it.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame" ParentStory="u1"
                       GeometricBounds="0 0 100 200">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="999 999"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.text_frames[0].bounds.right, 200.0);
        assert_eq!(s.text_frames[0].bounds.bottom, 100.0);
    }

    #[test]
    fn polygon_text_path_attaches_to_parent_polygon() {
        // Real-world IDML serialises text-on-path as a `<TextPath>`
        // child of the host shape, referencing a story via
        // `ParentStory`. The host's own `<PathGeometry>` provides the
        // curve geometry — we just need the story link plus a few
        // alignment knobs.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="0 0"/>
                  <PathPointType Anchor="100 0"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
              <TextPath Self="tp1" ParentStory="story_u1"
                        PathAlignment="CenterPathAlignment"
                        PathTypeAlignment="CenterPathType"
                        PathEffect="RainbowPathEffect"
                        FlipPathEffect="NotFlipped"
                        StartBracket="0" EndBracket="100"/>
            </Polygon>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.polygons[0].text_paths.len(), 1);
        let tp = &s.polygons[0].text_paths[0];
        assert_eq!(tp.parent_story, "story_u1");
        assert_eq!(tp.self_id.as_deref(), Some("tp1"));
        assert_eq!(tp.path_alignment.as_deref(), Some("CenterPathAlignment"));
        assert_eq!(tp.path_type_alignment.as_deref(), Some("CenterPathType"));
        assert_eq!(tp.path_effect.as_deref(), Some("RainbowPathEffect"));
        assert_eq!(tp.start_bracket, Some(0.0));
        assert_eq!(tp.end_bracket, Some(100.0));
    }

    #[test]
    fn polygon_hosts_image_link_and_item_transform() {
        // A `<Polygon>` may host a placed image just like a Rectangle.
        // The nested `<Image>`'s `LinkResourceURI` (or its `<Link>`
        // child's `LinkResourceURI`) populates `image_link`; the
        // `<Image>`'s `ItemTransform` populates `image_item_transform`.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1" GeometricBounds="0 0 100 100">
              <Properties/>
              <Image Self="img1" ItemTransform="0.5 0 0 0.5 10 20">
                <Link Self="link1" LinkResourceURI="file:///tmp/photo.jpg"/>
              </Image>
            </Polygon>
            <Polygon Self="poly2" GeometricBounds="0 0 50 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.polygons.len(), 2);
        let p = &s.polygons[0];
        assert_eq!(p.image_link.as_deref(), Some("file:///tmp/photo.jpg"));
        assert_eq!(
            p.image_item_transform,
            Some([0.5, 0.0, 0.0, 0.5, 10.0, 20.0])
        );
        // Plain polygon without image stays None.
        assert!(s.polygons[1].image_link.is_none());
        assert!(s.polygons[1].image_item_transform.is_none());
        // Rectangles in the same spread keep working.
        assert_eq!(s.rectangles.len(), 0);
    }

    #[test]
    fn group_records_members_and_transparency_block() {
        // A `<Group>` wrapping two rectangles with its own
        // `<TransparencySetting>` / `<BlendingSetting>` /
        // `<DropShadowSetting>` block. The group entry should carry
        // the blend mode + opacity + drop shadow; member FrameRefs
        // should match the rectangles' indices in document order.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group Self="grp1" ItemTransform="1 0 0 1 5 7">
              <Properties>
                <TransparencySetting>
                  <BlendingSetting Opacity="60" BlendMode="Multiply"/>
                  <DropShadowSetting Mode="Drop" XOffset="2" YOffset="3" Size="5"
                                     Opacity="80" EffectColor="Color/Black"/>
                </TransparencySetting>
              </Properties>
              <Rectangle Self="r1" GeometricBounds="0 0 50 50"/>
              <Rectangle Self="r2" GeometricBounds="0 60 50 110"/>
            </Group>
            <Rectangle Self="r3" GeometricBounds="100 0 150 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.rectangles.len(), 3);
        assert_eq!(s.groups.len(), 1);
        let g = &s.groups[0];
        assert_eq!(g.self_id.as_deref(), Some("grp1"));
        assert_eq!(g.item_transform, Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]));
        assert_eq!(g.transparency.blend_mode.as_deref(), Some("Multiply"));
        assert_eq!(g.transparency.opacity, Some(60.0));
        let shadow = g
            .transparency
            .drop_shadow
            .as_ref()
            .expect("drop shadow on group");
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 2.0);
        assert_eq!(shadow.opacity_pct, 80.0);
        // Members are the two grouped rectangles in document order;
        // r3 sits outside and is NOT a member.
        assert_eq!(
            g.members,
            vec![FrameRef::Rectangle(0), FrameRef::Rectangle(1)]
        );
        // Top-level surface: the group as a single entry + the
        // ungrouped r3. Grouped rectangles do NOT appear here.
        assert_eq!(
            s.frames_in_order,
            vec![FrameRef::Group(0), FrameRef::Rectangle(2)]
        );
    }

    #[test]
    fn nested_groups_register_subgroup_members() {
        // Outer group contains a sub-group + a TextFrame. The
        // sub-group contains two Polygons. Outer's members should
        // list TextFrame(0), Group(0). Inner's members should list
        // both polygons.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group Self="outer">
              <TextFrame Self="t1" ParentStory="u1" GeometricBounds="0 0 10 10"/>
              <Group Self="inner">
                <Polygon Self="p1" GeometricBounds="0 0 5 5"/>
                <Polygon Self="p2" GeometricBounds="0 0 6 6"/>
              </Group>
            </Group>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.groups.len(), 2);
        // Inner group closes first → at index 0.
        let inner = &s.groups[0];
        assert_eq!(inner.self_id.as_deref(), Some("inner"));
        assert_eq!(
            inner.members,
            vec![FrameRef::Polygon(0), FrameRef::Polygon(1)]
        );
        let outer = &s.groups[1];
        assert_eq!(outer.self_id.as_deref(), Some("outer"));
        assert_eq!(
            outer.members,
            vec![FrameRef::TextFrame(0), FrameRef::Group(0)]
        );
        // Group transparency defaults to all-None when absent.
        assert!(outer.transparency.blend_mode.is_none());
        assert!(outer.transparency.opacity.is_none());
        assert!(outer.transparency.drop_shadow.is_none());
        // Outer is the only top-level item; inner stays buried in
        // outer.members and does NOT surface in frames_in_order.
        assert_eq!(s.frames_in_order, vec![FrameRef::Group(1)]);
    }

    #[test]
    fn group_blending_setting_does_not_leak_into_inner_frame() {
        // BlendingSetting attached to the Group must update the
        // group's transparency, not the inner frames' opacity. The
        // current_frame check in the BlendingSetting arm already
        // disambiguates; this test pins the contract.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group Self="grp">
              <Properties>
                <TransparencySetting>
                  <BlendingSetting Opacity="40" BlendMode="Screen"/>
                </TransparencySetting>
              </Properties>
              <Rectangle Self="r1" GeometricBounds="0 0 50 50"/>
            </Group>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert!(s.rectangles[0].opacity.is_none());
        assert!(s.rectangles[0].blend_mode.is_none());
        assert_eq!(s.groups.len(), 1);
        assert_eq!(s.groups[0].transparency.opacity, Some(40.0));
        assert_eq!(
            s.groups[0].transparency.blend_mode.as_deref(),
            Some("Screen")
        );
    }

    #[test]
    fn polygon_image_link_falls_through_to_outer_image_attribute() {
        // When the `<Image>` element itself carries a
        // `LinkResourceURI` (no nested `<Link>`), the polygon still
        // picks it up. Mirrors the Rectangle behaviour.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1" GeometricBounds="0 0 100 100">
              <Image Self="img1" LinkResourceURI="file:///tmp/cat.png"
                     ItemTransform="1 0 0 1 0 0"/>
            </Polygon>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(
            s.polygons[0].image_link.as_deref(),
            Some("file:///tmp/cat.png")
        );
        assert_eq!(
            s.polygons[0].image_item_transform,
            Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        );
    }

    #[test]
    fn parses_directional_feather_setting() {
        // Per-edge widths land in `directional_feather`; the bool
        // sentinel from the previous parser is gone.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Properties>
                <TransparencySetting>
                  <DirectionalFeatherSetting Applied="true"
                    LeftWidth="2" RightWidth="3" TopWidth="4" BottomWidth="5"
                    Angle="90" NoiseAmount="10" ChokeAmount="20"
                    CornerType="Rounded"/>
                </TransparencySetting>
              </Properties>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let bag = s.rectangles[0].effects.as_ref().expect("effects bag");
        let dir = bag
            .directional_feather
            .as_ref()
            .expect("directional feather parsed");
        assert_eq!(dir.left_width, Some(2.0));
        assert_eq!(dir.right_width, Some(3.0));
        assert_eq!(dir.top_width, Some(4.0));
        assert_eq!(dir.bottom_width, Some(5.0));
        assert_eq!(dir.angle_deg, Some(90.0));
        assert_eq!(dir.noise_pct, Some(10.0));
        assert_eq!(dir.choke_pct, Some(20.0));
        assert_eq!(dir.corner_type.as_deref(), Some("Rounded"));
    }

    #[test]
    fn directional_feather_disabled_when_applied_false() {
        // `Applied="false"` short-circuits the whole block —
        // directional_feather stays None.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Properties>
                <TransparencySetting>
                  <DirectionalFeatherSetting Applied="false"
                    LeftWidth="2" RightWidth="3" TopWidth="4" BottomWidth="5"/>
                </TransparencySetting>
              </Properties>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        // The effects bag may be absent entirely or have no
        // directional_feather; both are acceptable.
        let dir_present = s.rectangles[0]
            .effects
            .as_ref()
            .and_then(|e| e.directional_feather.as_ref())
            .is_some();
        assert!(
            !dir_present,
            "Applied=false should leave directional_feather=None"
        );
    }

    #[test]
    fn parses_gradient_feather_setting_with_stops() {
        // Linear gradient feather with two stops; `<GradientStop>`
        // children are nested inside `<GradientFeatherSetting>` and
        // get appended to `gradient_feather.stops`.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Properties>
                <TransparencySetting>
                  <GradientFeatherSetting Applied="true" Type="Linear"
                                          GradientAngle="45">
                    <GradientStop StopColor="Color/Black" Location="0"
                                  Alpha="100" GradientStopMidpoint="50"/>
                    <GradientStop StopColor="Color/Black" Location="100"
                                  Alpha="0" GradientStopMidpoint="50"/>
                  </GradientFeatherSetting>
                </TransparencySetting>
              </Properties>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let bag = s.rectangles[0].effects.as_ref().expect("effects bag");
        let gf = bag
            .gradient_feather
            .as_ref()
            .expect("gradient feather parsed");
        assert_eq!(gf.gradient_type.as_deref(), Some("Linear"));
        assert_eq!(gf.angle_deg, Some(45.0));
        assert_eq!(gf.stops.len(), 2);
        assert_eq!(gf.stops[0].location_pct, 0.0);
        assert_eq!(gf.stops[0].alpha_pct, 100.0);
        assert_eq!(gf.stops[0].stop_color.as_deref(), Some("Color/Black"));
        assert_eq!(gf.stops[1].location_pct, 100.0);
        assert_eq!(gf.stops[1].alpha_pct, 0.0);
    }

    /// IDML compound paths (e.g. `<Polygon>` with two
    /// `<GeometryPathType>` children — square + hole) must surface
    /// the contour boundaries via `subpath_starts` so the renderer
    /// can lift them into separate MoveTo/Close subpaths. Without
    /// this, the renderer silently joins the two contours into one
    /// broken polyline (the geometry-groups page-6 visual regression).
    #[test]
    fn polygon_compound_path_records_subpath_starts() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="200 0"/>
                          <PathPointType Anchor="200 200"/>
                          <PathPointType Anchor="0 200"/>
                        </PathPointArray>
                      </GeometryPathType>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="60 60"/>
                          <PathPointType Anchor="60 140"/>
                          <PathPointType Anchor="140 140"/>
                          <PathPointType Anchor="140 60"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        let p = &s.polygons[0];
        assert_eq!(p.anchors.len(), 8, "both contours' anchors are stored");
        assert_eq!(
            p.subpath_starts,
            vec![0, 4],
            "compound path → two contour starts at indices 0 and 4"
        );
    }

    /// Single-contour polygons (the InDesign-export shape every plain
    /// rectangle / polygon uses) leave `subpath_starts` empty so the
    /// renderer's legacy single-MoveTo path keeps firing.
    #[test]
    fn polygon_path_open_lifts_to_subpath_open_flag() {
        // P-15: `<GeometryPathType PathOpen="true">` should lift onto
        // the polygon's `subpath_open` slice so the renderer can skip
        // the auto-close. Single open contour: `subpath_starts` stays
        // empty (legacy canonical form for one contour) but
        // `subpath_open` carries `[true]` so the renderer can branch.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="true">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="100 0"/>
                          <PathPointType Anchor="50 50"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.polygons[0].subpath_open, vec![true]);
    }

    #[test]
    fn polygon_compound_path_open_records_per_contour_flags() {
        // P-15: two contours, one open and one closed; the flags need
        // to come out in declaration order parallel to `subpath_starts`.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="true">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="40 40"/>
                        </PathPointArray>
                      </GeometryPathType>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="100 0"/>
                          <PathPointType Anchor="200 0"/>
                          <PathPointType Anchor="200 100"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.polygons[0].subpath_starts, vec![0, 2]);
        assert_eq!(s.polygons[0].subpath_open, vec![true, false]);
    }

    #[test]
    fn polygon_single_contour_leaves_subpath_starts_empty() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="100 0"/>
                          <PathPointType Anchor="100 100"/>
                          <PathPointType Anchor="0 100"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert!(
            s.polygons[0].subpath_starts.is_empty(),
            "single contour → no markers (legacy path stays hot)"
        );
    }

    #[test]
    fn overprint_attributes_round_trip_through_every_shape() {
        // Pin that `OverprintFill` / `OverprintStroke` lift off the
        // outer tag for every page-item kind (Rectangle / Oval /
        // TextFrame / Polygon / GraphicLine). Absent attributes
        // default to `false` (the IDML default).
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
            <idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s1">
                <Rectangle Self="r1"
                           GeometricBounds="0 0 10 10"
                           FillColor="Color/Black"
                           StrokeColor="Color/None"
                           OverprintFill="true"
                           OverprintStroke="true"/>
                <Rectangle Self="r2"
                           GeometricBounds="0 0 10 10"
                           FillColor="Color/Cyan"/>
                <Oval Self="o1"
                      GeometricBounds="0 0 10 10"
                      FillColor="Color/Black"
                      OverprintFill="true"/>
                <TextFrame Self="t1"
                           ParentStory="u10"
                           GeometricBounds="0 0 10 10"
                           OverprintFill="true"/>
                <Polygon Self="p1"
                         GeometricBounds="0 0 10 10"
                         FillColor="Color/Black"
                         OverprintFill="true"
                         OverprintStroke="false">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType>
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="10 0"/>
                          <PathPointType Anchor="10 10"/>
                          <PathPointType Anchor="0 10"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
                <GraphicLine Self="l1"
                             GeometricBounds="0 0 10 10"
                             StrokeColor="Color/Black"
                             OverprintStroke="true"/>
              </Spread>
            </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        // Rect r1: both flags true; r2: both false (defaults).
        assert!(s.rectangles[0].overprint_fill);
        assert!(s.rectangles[0].overprint_stroke);
        assert!(!s.rectangles[1].overprint_fill);
        assert!(!s.rectangles[1].overprint_stroke);
        // Oval: fill flag picked up.
        assert!(s.ovals[0].overprint_fill);
        assert!(!s.ovals[0].overprint_stroke);
        // TextFrame: fill flag picked up.
        assert!(s.text_frames[0].overprint_fill);
        // Polygon: fill true, stroke explicitly false.
        assert!(s.polygons[0].overprint_fill);
        assert!(!s.polygons[0].overprint_stroke);
        // GraphicLine: only stroke is meaningful.
        assert!(s.graphic_lines[0].overprint_stroke);
    }

    #[test]
    fn parses_placed_image_colorspace_and_ppi() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Image Self="img1" Space="$ID/CMYK"
                     ActualPpi="(300 300)" EffectivePpi="(225 225)"
                     LinkResourceURI="file:///photo.tif"/>
            </Rectangle>
            <Rectangle Self="r2" GeometricBounds="0 0 50 50" FillColor="Color/Red"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let meta = s.image_metadata.get("r1").expect("metadata for r1");
        assert_eq!(meta.space.as_deref(), Some("CMYK"), "$ID/ prefix stripped");
        assert!((meta.actual_ppi.unwrap() - 300.0).abs() < 1e-3);
        assert!((meta.effective_ppi.unwrap() - 225.0).abs() < 1e-3);
        // Plain colour-swatch rectangle has no image metadata.
        assert!(!s.image_metadata.contains_key("r2"));
    }

    #[test]
    fn parses_page_margins_and_columns() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Page Self="p1" GeometricBounds="0 0 792 612">
              <MarginPreference Top="36" Bottom="48" Left="54" Right="54"
                                ColumnCount="3" ColumnGutter="12"/>
            </Page>
            <Page Self="p2" GeometricBounds="0 0 792 612"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let m = s.page_margins.get("p1").expect("margins for p1");
        assert!((m.top - 36.0).abs() < 1e-3);
        assert!((m.bottom - 48.0).abs() < 1e-3);
        assert!((m.left - 54.0).abs() < 1e-3);
        assert!((m.right - 54.0).abs() < 1e-3);
        assert_eq!(m.column_count, 3);
        assert!((m.column_gutter - 12.0).abs() < 1e-3);
        // p2 declared no MarginPreference.
        assert!(!s.page_margins.contains_key("p2"));
    }

    // ── W1.21: image clipping paths ──────────────────────────────────

    /// `UserModifiedPath` with inline `<PathGeometry>` ⇒ clip anchors
    /// captured into `image_clip.clip_anchors`, NOT into the host
    /// rectangle's outline anchors (the clip lives in image space).
    #[test]
    fn parses_user_modified_clipping_path_geometry() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:p.png">
                <Properties>
                  <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
                    <PathPointType Anchor="0 0"/>
                    <PathPointType Anchor="100 0"/>
                    <PathPointType Anchor="100 100"/>
                    <PathPointType Anchor="0 100"/>
                  </PathPointArray></GeometryPathType></PathGeometry>
                </Properties>
                <ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="false"
                                      IncludeInsideEdges="false">
                  <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
                    <PathPointType Anchor="10 10"/>
                    <PathPointType Anchor="90 10"/>
                    <PathPointType Anchor="50 90"/>
                  </PathPointArray></GeometryPathType></PathGeometry>
                </ClippingPathSettings>
                <Link LinkResourceURI="file:p.png"/>
              </Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let clip = s.rectangles[0]
            .image_clip
            .as_ref()
            .expect("clipping path settings parsed");
        assert_eq!(clip.clipping_type, Some(ClippingType::UserModifiedPath));
        assert!(!clip.invert_path);
        assert!(!clip.include_inside_edges);
        // Three triangle anchors captured into the clip, in image space.
        assert_eq!(clip.clip_anchors.len(), 3);
        assert_eq!(clip.clip_anchors[0].anchor, (10.0, 10.0));
        assert_eq!(clip.clip_anchors[2].anchor, (50.0, 90.0));
        assert!(clip.has_renderable_geometry());
        assert!(!clip.is_deferred_clip());
        // The image's own PathGeometry (the picture box) must NOT have
        // leaked into the rectangle's outline anchors.
        assert!(
            s.rectangles[0].anchors.is_empty(),
            "image PathGeometry must not pollute the frame outline"
        );
    }

    /// Compound clip (two `<GeometryPathType>` contours) with
    /// `IncludeInsideEdges="true"` records `clip_subpath_starts` so the
    /// hole survives downstream.
    #[test]
    fn parses_compound_clipping_path_subpath_starts() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:p.png">
                <ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="false"
                                      IncludeInsideEdges="true">
                  <PathGeometry>
                    <GeometryPathType PathOpen="false"><PathPointArray>
                      <PathPointType Anchor="0 0"/>
                      <PathPointType Anchor="100 0"/>
                      <PathPointType Anchor="100 100"/>
                      <PathPointType Anchor="0 100"/>
                    </PathPointArray></GeometryPathType>
                    <GeometryPathType PathOpen="false"><PathPointArray>
                      <PathPointType Anchor="40 40"/>
                      <PathPointType Anchor="60 40"/>
                      <PathPointType Anchor="60 60"/>
                      <PathPointType Anchor="40 60"/>
                    </PathPointArray></GeometryPathType>
                  </PathGeometry>
                </ClippingPathSettings>
                <Link LinkResourceURI="file:p.png"/>
              </Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let clip = s.rectangles[0].image_clip.as_ref().expect("clip parsed");
        assert!(clip.include_inside_edges);
        assert_eq!(clip.clip_anchors.len(), 8);
        // One subpath-start per <GeometryPathType>: 0 and 4.
        assert_eq!(clip.clip_subpath_starts, vec![0, 4]);
        assert_eq!(clip.clip_subpath_open, vec![false, false]);
        assert!(clip.has_renderable_geometry());
    }

    /// `InvertPath="true"` lifts onto the parsed settings.
    #[test]
    fn parses_invert_clipping_path_flag() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:p.png">
                <ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="true"
                                      IncludeInsideEdges="false">
                  <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
                    <PathPointType Anchor="20 20"/>
                    <PathPointType Anchor="80 20"/>
                    <PathPointType Anchor="80 80"/>
                    <PathPointType Anchor="20 80"/>
                  </PathPointArray></GeometryPathType></PathGeometry>
                </ClippingPathSettings>
              </Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let clip = s.rectangles[0].image_clip.as_ref().expect("clip parsed");
        assert!(clip.invert_path);
        assert!(clip.has_renderable_geometry());
    }

    /// `ClippingType="PhotoshopPath"` with a named path but NO inline
    /// geometry ⇒ the defer case: no clip anchors, `is_deferred_clip()`
    /// true, `AppliedPathName` captured for the diagnostic.
    #[test]
    fn parses_photoshop_clipping_path_as_deferred() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:p.png">
                <ClippingPathSettings ClippingType="PhotoshopPath" InvertPath="false"
                                      IncludeInsideEdges="false" AppliedPathName="Path 1"
                                      Threshold="25" Tolerance="2"/>
                <Link LinkResourceURI="file:p.png"/>
              </Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let clip = s.rectangles[0].image_clip.as_ref().expect("clip parsed");
        assert_eq!(clip.clipping_type, Some(ClippingType::PhotoshopPath));
        assert!(clip.clip_anchors.is_empty());
        assert!(clip.is_deferred_clip());
        assert!(!clip.has_renderable_geometry());
        assert_eq!(clip.applied_path_name.as_deref(), Some("Path 1"));
        assert_eq!(clip.threshold, Some(25.0));
        assert_eq!(clip.tolerance, Some(2.0));
    }

    /// `ClippingType="None"` (or absent settings) is not a deferred
    /// clip and carries no geometry.
    #[test]
    fn clipping_type_none_is_not_deferred() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:p.png">
                <ClippingPathSettings ClippingType="None" InvertPath="false"
                                      IncludeInsideEdges="false"/>
                <Link LinkResourceURI="file:p.png"/>
              </Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let clip = s.rectangles[0].image_clip.as_ref().expect("clip parsed");
        assert_eq!(clip.clipping_type, Some(ClippingType::None));
        assert!(!clip.is_deferred_clip());
        assert!(!clip.has_renderable_geometry());
    }

    /// A polygon-hosted image's own `<PathGeometry>` must not pollute
    /// the polygon's outline anchors (the `in_image_depth` guard).
    #[test]
    fn image_path_geometry_does_not_pollute_polygon_outline() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1">
              <Properties>
                <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
                  <PathPointType Anchor="0 0"/>
                  <PathPointType Anchor="50 0"/>
                  <PathPointType Anchor="25 50"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
              <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:p.png">
                <Properties>
                  <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
                    <PathPointType Anchor="0 0"/>
                    <PathPointType Anchor="100 0"/>
                    <PathPointType Anchor="100 100"/>
                    <PathPointType Anchor="0 100"/>
                  </PathPointArray></GeometryPathType></PathGeometry>
                </Properties>
                <Link LinkResourceURI="file:p.png"/>
              </Image>
            </Polygon>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        // The polygon keeps its own triangle (3 anchors), not the
        // image's 4-corner box appended after it.
        assert_eq!(
            s.polygons[0].anchors.len(),
            3,
            "image box must not be absorbed into the polygon outline"
        );
    }

    #[test]
    fn parses_text_wrap_contour_option() {
        // W2.5 — a `<ContourOption>` child of `<TextWrapPreference>`
        // folds its ContourType + IncludeInsideEdges into the shape's
        // TextWrap, preserving the mode.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="rect1" GeometricBounds="0 0 100 200">
              <TextWrapPreference TextWrapMode="ContourTextWrap" Inverse="false">
                <ContourOption ContourType="DetectEdges" IncludeInsideEdges="true"/>
              </TextWrapPreference>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let tw = s.rectangles[0].text_wrap.expect("text_wrap parsed");
        assert_eq!(tw.mode, TextWrapMode::ContourTextWrap);
        assert_eq!(tw.contour_type, Some(ContourOptionType::DetectEdges));
        assert_eq!(tw.include_inside_edges, Some(true));
    }

    #[test]
    fn parses_element_visible_locked() {
        // W2.5 — element-level Visible / Locked attributes on a page
        // item. Defaults: Visible=true, Locked=false.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="hidden" Visible="false" Locked="true"/>
            <Rectangle Self="default"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = parse_spread(xml).unwrap();
        let hidden = s
            .rectangles
            .iter()
            .find(|r| r.self_id.as_deref() == Some("hidden"))
            .unwrap();
        assert!(!hidden.visible);
        assert!(hidden.locked);
        let default = s
            .rectangles
            .iter()
            .find(|r| r.self_id.as_deref() == Some("default"))
            .unwrap();
        assert!(default.visible, "absent Visible ⇒ visible");
        assert!(!default.locked, "absent Locked ⇒ unlocked");
    }
}
