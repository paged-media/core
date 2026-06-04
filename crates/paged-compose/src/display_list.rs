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

//! Display-list primitives.
//!
//! A flat command stream plus a path buffer. The command stream is the
//! handoff format between the CPU-side compositor and the GPU backend;
//! the path buffer lets repeated shapes (especially glyphs) share
//! tessellated data.

use std::hash::{Hash, Hasher};

/// Linear-RGB colour. All compositing happens in linear light; gamma
/// conversion is the GPU backend's responsibility.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

/// Paint describes how a path is filled. Solid colour and linear
/// gradient cover most IDML fills today; radial / image / pattern
/// fills land with §10.3.
///
/// `Paint` is `Copy`, so gradients are stored once in
/// `DisplayList::gradients` and referenced by id rather than embedded.
///
/// `Cmyk` carries native CMYK channels (0.0..=1.0 each) all the way
/// through to the rasterizer — necessary for true per-channel CMYK
/// overprint compositing (Phase 3 Tier 3 #14 Stage A). For ordinary
/// draws the rasterizer uses the `rgb` cached field (the compose stage
/// pre-baked the ICC-converted display colour) so the visible result
/// stays bit-identical to a `Paint::Solid` of the same swatch. Only
/// the overprint path consumes the C/M/Y/K channels separately.
///
/// Stage C: a `Cmyk` paint can optionally carry a [`SpotInkId`] that
/// identifies a named-ink swatch (e.g. PANTONE 286). The rasterizer
/// routes spot draws to a dedicated per-spot-ink plane in addition to
/// painting the cached `rgb` into the framebuffer; overprint of two
/// runs of the SAME spot ink composes via per-pixel `max(top, bot)`
/// in the spot plane. Different-named spots overprint as independent
/// inks. The CMYK channels on the paint carry the spot's CMYK alternate
/// (with any swatch-level `TintValue` already folded in by the parser
/// via `ColorEntry::effective_cmyk`) — preserved so the legacy CMYK
/// overprint composite can still operate when a spot overprints over
/// a non-spot CMYK ink.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Solid(Color),
    LinearGradient(GradientId),
    /// Radial gradient — `center → center + radius` in unit-rect
    /// coords. Same id-space as linear gradients but resolved against
    /// `DisplayList::radial_gradients` instead of `gradients`.
    RadialGradient(GradientId),
    /// Native CMYK paint preserved end to end. Channels are 0.0..=1.0
    /// (NOT percentages — the renderer scales IDML's 0..100 percent
    /// values to the unit range at compose time). `rgb` is the
    /// already-ICC-resolved linear-RGB colour the rasterizer would
    /// have used pre-Stage-A — keeping it on the paint lets ordinary
    /// (non-overprint) draws render bit-identically to the prior
    /// `Paint::Solid` path without re-running ICC at raster time.
    ///
    /// `spot = Some(id)` marks the paint as a named-ink spot colour
    /// resolved via the `DisplayList::spot_inks` table. The CMYK
    /// channels remain populated with the spot's CMYK alternate; the
    /// `id` lets the rasterizer route the per-pixel ink into a
    /// dedicated spot plane rather than collapsing immediately into
    /// the C/M/Y/K planes. `spot = None` is a plain process CMYK.
    Cmyk {
        c: f32,
        m: f32,
        y: f32,
        k: f32,
        rgb: Color,
        spot: Option<SpotInkId>,
    },
}

/// Index into `DisplayList::gradients` *or* `DisplayList::radial_gradients`,
/// depending on the [`Paint`] variant carrying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GradientId(pub u32);

/// Index into `DisplayList::spot_inks`. Identifies a named spot ink
/// (e.g. PANTONE 286 C) so the rasterizer can track per-pixel tints
/// for that ink on its own plane and composite spot-on-same-spot
/// overprints correctly. Stage C of the CMYK pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpotInkId(pub u32);

/// A named-ink spot colour registered on the display list. `name` is
/// the IDML `<Color Self="...">` id (e.g. `"Color/Pantone286"`) —
/// stable across draws so two paints with the same ink intern to the
/// same `SpotInkId`. `cmyk_alternate` holds the spot's CMYK alternate
/// in 8-bit space (0..=255 per channel) with the swatch-level `TintValue`
/// already folded in — that's what the final flush composites into the
/// CMYK planes for spot-over-spot or spot-over-CMYK pixels where the
/// late-bound preview needs to converge to a single visible colour.
#[derive(Debug, Clone, PartialEq)]
pub struct SpotInk {
    pub name: String,
    pub cmyk_alternate: [u8; 4],
}

/// One stop in a gradient: a colour at a normalised offset (0..=1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// Linear gradient definition. Endpoints are unit coordinates
/// (`(0, 0)..(1, 0)` is left → right; `(0, 0)..(0, 1)` is
/// top → bottom). The path's transform maps the unit square to its
/// final geometry, so the same gradient reused on N rectangles
/// renders correctly on each.
#[derive(Debug, Clone)]
pub struct LinearGradient {
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub stops: Vec<GradientStop>,
}

/// Radial gradient definition. `center` is in unit-rect coords
/// (`(0.5, 0.5)` is the centre of the path's local rect); `radius`
/// is in the same coord space (`0.5` covers half the unit rect).
/// IDML's `GradientFillStart` + `GradientFillLength` translate to
/// page-space center + half-length, but the renderer currently
/// places the radial gradient at the unit-rect centre with full
/// radius — that matches the common case (Oval-with-radial fills).
#[derive(Debug, Clone)]
pub struct RadialGradient {
    pub center: (f32, f32),
    pub radius: f32,
    pub stops: Vec<GradientStop>,
}

/// 2×3 affine transform stored as `[a b c d tx ty]` —
/// `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform(pub [f32; 6]);

impl Transform {
    pub const IDENTITY: Transform = Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(tx: f32, ty: f32) -> Self {
        Transform([1.0, 0.0, 0.0, 1.0, tx, ty])
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Transform([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    /// Rotation by `deg` degrees clockwise about the origin (y grows
    /// downward, so a positive angle turns clockwise on screen).
    pub fn rotate_deg(deg: f32) -> Self {
        let r = deg.to_radians();
        let (s, c) = r.sin_cos();
        Transform([c, s, -s, c, 0.0, 0.0])
    }

    /// Apply to a point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, tx, ty] = self.0;
        (a * x + c * y + tx, b * x + d * y + ty)
    }

    /// Build the transform that maps the unit rect `[0,0,1,1]` onto
    /// `rect` in some local space, with `outer` applied on top:
    /// `result = outer ∘ scale(rect.w, rect.h) ∘ translate(rect.x, rect.y)`.
    /// Centralises the unit-rect-to-page mapping shared by every
    /// `emit_*_transformed` helper in `primitives` and the image
    /// emitter — keeps that math in one place.
    pub fn for_rect_in(rect: Rect, outer: Transform) -> Transform {
        outer.compose(&Transform([rect.w, 0.0, 0.0, rect.h, rect.x, rect.y]))
    }

    /// Compose `self` with `inner`: the result applies `inner` first,
    /// then `self`. I.e. `self.compose(inner).apply(p) == self.apply(inner.apply(p))`.
    pub fn compose(&self, inner: &Transform) -> Transform {
        let [a1, b1, c1, d1, tx1, ty1] = self.0;
        let [a2, b2, c2, d2, tx2, ty2] = inner.0;
        Transform([
            a1 * a2 + c1 * b2,
            b1 * a2 + d1 * b2,
            a1 * c2 + c1 * d2,
            b1 * c2 + d1 * d2,
            a1 * tx2 + c1 * ty2 + tx1,
            b1 * tx2 + d1 * ty2 + ty1,
        ])
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One segment of a bezier path in local path coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSegment {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    QuadTo {
        cx: f32,
        cy: f32,
        x: f32,
        y: f32,
    },
    CubicTo {
        cx1: f32,
        cy1: f32,
        cx2: f32,
        cy2: f32,
        x: f32,
        y: f32,
    },
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct PathData {
    pub segments: Vec<PathSegment>,
}

impl PathData {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// Opaque index into a [`PathBuffer`]. Stable within a [`DisplayList`]
/// but not across lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathId(pub u32);

/// Owns all path-segment data and hands out `PathId`s. Uses a caller-
/// supplied cache key for interning, so (glyph_id, font_id, size)
/// combinations share outlines across the command stream.
#[derive(Debug, Default)]
pub struct PathBuffer {
    paths: Vec<PathData>,
    /// Cache key → PathId. Callers are responsible for making the key
    /// unique for their domain (glyph caches use `GlyphCacheKey`).
    cache: std::collections::HashMap<u64, PathId>,
}

impl PathBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `path` under `key`. Returns the existing id if `key` has
    /// been seen before; otherwise stores `path` and returns a fresh
    /// id. The second return value is true when the path was freshly
    /// stored.
    pub fn intern(&mut self, key: u64, path: PathData) -> (PathId, bool) {
        if let Some(id) = self.cache.get(&key) {
            return (*id, false);
        }
        let id = PathId(self.paths.len() as u32);
        self.paths.push(path);
        self.cache.insert(key, id);
        (id, true)
    }

    /// Probe for an existing interned id without inserting. Useful
    /// when producing the `PathData` is expensive and should be
    /// skipped on a cache hit.
    pub fn find_by_key(&self, key: u64) -> Option<PathId> {
        self.cache.get(&key).copied()
    }

    /// Store `path` without interning. Useful for one-off shapes.
    pub fn push_anon(&mut self, path: PathData) -> PathId {
        let id = PathId(self.paths.len() as u32);
        self.paths.push(path);
        id
    }

    pub fn get(&self, id: PathId) -> Option<&PathData> {
        self.paths.get(id.0 as usize)
    }

    /// Extract a slice of the underlying path vec for caching /
    /// snapshotting (Perf-MasterText). Returns the path-buffer's
    /// raw storage between `[start, end)`. Read-only access; the
    /// caller is responsible for not feeding stale indices.
    pub fn slice(&self, start: usize, end: usize) -> &[PathData] {
        &self.paths[start.min(self.paths.len())..end.min(self.paths.len())]
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Cache key for a glyph outline. Hashed to give `PathBuffer::intern`
/// a `u64`. Designers note: `font_id` is a user-space integer; callers
/// are responsible for making it stable across a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphCacheKey {
    pub font_id: u32,
    pub glyph_id: u32,
}

impl GlyphCacheKey {
    pub fn to_u64(self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// Drop-shadow parameters. All measurements are in pt; `color` is
/// linear RGB; `opacity` is multiplied into the shadow alpha.
///
/// `blur_radius` is honoured by the CPU rasterizer as σ in pt for a
/// separable Gaussian convolution over a padded offscreen stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub color: Color,
    pub opacity: f32,
}

impl DropShadow {
    /// Sensible default: 4 pt offset down-right, 4 pt blur radius
    /// (currently ignored), 30% black.
    pub fn default_soft() -> Self {
        Self {
            offset_x: 4.0,
            offset_y: 4.0,
            blur_radius: 4.0,
            color: Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 0.3,
        }
    }
}

/// Inner-shadow parameters. Same units as [`DropShadow`] (pt, linear
/// RGB, 0..1 opacity), but the rasterizer paints the soft stamp on
/// the *inside* of the path: the path's interior is darkened where
/// the offset/blurred shadow stamp falls inside it. `choke` is in pt
/// — it expands the shadow's hard edge before blurring (mapped to a
/// dilation in the rasterizer's stamp pass; `0.0` is the common case).
/// `blend_mode` is reserved for future fidelity work (most IDML inner
/// shadows use `Multiply`); the rasterizer renders the stamp with
/// straight alpha-over today.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InnerShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub color: Color,
    pub opacity: f32,
    pub choke: f32,
    pub blend_mode: BlendMode,
}

impl InnerShadow {
    /// Sensible default: 3 pt offset down-right, 6 pt blur, 50%
    /// black, no choke, Multiply.
    pub fn default_soft() -> Self {
        Self {
            offset_x: 3.0,
            offset_y: 3.0,
            blur_radius: 6.0,
            color: Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 0.5,
            choke: 0.0,
            blend_mode: BlendMode::Multiply,
        }
    }
}

/// Outer-glow parameters. The rasterizer paints a soft halo *outside*
/// the path's interior: blur the filled stamp, then composite it
/// behind / around the path. `spread` is in pt — it grows the hard
/// stamp before blurring so glows can extend beyond a thin path.
/// `blend_mode` is most commonly `Screen` for InDesign-style glows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuterGlow {
    pub blur_radius: f32,
    pub color: Color,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub spread: f32,
}

impl OuterGlow {
    pub fn default_soft() -> Self {
        Self {
            blur_radius: 6.0,
            color: Color::rgba(1.0, 1.0, 0.5, 1.0),
            opacity: 0.75,
            blend_mode: BlendMode::Screen,
            spread: 0.0,
        }
    }
}

/// Inner-glow parameters. Same shape as [`InnerShadow`] without the
/// directional offset: paint a soft glow on the *inside* of the
/// path's interior. `choke` is in pt (same dilation knob as
/// `InnerShadow::choke`). InDesign's default uses `Screen` blend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InnerGlow {
    pub blur_radius: f32,
    pub color: Color,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub choke: f32,
}

impl InnerGlow {
    pub fn default_soft() -> Self {
        Self {
            blur_radius: 6.0,
            color: Color::rgba(1.0, 1.0, 0.5, 1.0),
            opacity: 0.75,
            blend_mode: BlendMode::Screen,
            choke: 0.0,
        }
    }
}

/// Bevel-and-emboss parameters. `depth` is the relative bump
/// strength (0..=1; 1.0 is "100% depth" in InDesign's slider);
/// `size` is the bevel width in pt. `angle_deg` is the light's
/// azimuth in screen space (0° = light from the right, 90° = light
/// from below); `altitude_deg` is the light's elevation (0 =
/// grazing, 90 = top-down). Highlight + shadow colour separately so
/// a coloured bevel can be expressed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BevelEmboss {
    pub depth: f32,
    pub size: f32,
    pub angle_deg: f32,
    pub altitude_deg: f32,
    pub highlight_color: Color,
    pub shadow_color: Color,
    pub highlight_opacity: f32,
    pub shadow_opacity: f32,
}

impl BevelEmboss {
    pub fn default_soft() -> Self {
        Self {
            depth: 1.0,
            size: 5.0,
            angle_deg: 120.0,
            altitude_deg: 30.0,
            highlight_color: Color::rgba(1.0, 1.0, 1.0, 1.0),
            shadow_color: Color::rgba(0.0, 0.0, 0.0, 1.0),
            highlight_opacity: 0.75,
            shadow_opacity: 0.75,
        }
    }
}

/// Satin parameters. The rasterizer offsets two blurred path stamps
/// in opposite directions (along `angle_deg`, separated by `distance`
/// pt) and uses their difference as a "wave" mask painted onto the
/// path's interior. `blend_mode` is most commonly `Multiply` for
/// dark satin and `Screen` for bright satin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Satin {
    pub blur_radius: f32,
    pub angle_deg: f32,
    pub distance: f32,
    pub color: Color,
    pub opacity: f32,
    pub blend_mode: BlendMode,
}

impl Satin {
    pub fn default_soft() -> Self {
        Self {
            blur_radius: 7.0,
            angle_deg: 19.0,
            distance: 11.0,
            color: Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 0.5,
            blend_mode: BlendMode::Multiply,
        }
    }
}

/// Feather corner shape. IDML's `<FeatherSetting CornerType="...">`
/// selects between three options; the rasterizer uses this to pick a
/// distance-field shape for the feather edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeatherCornerType {
    /// Sharp 90° corners — feather follows the path's outline
    /// faithfully.
    #[default]
    Sharp,
    /// Rounded corners — slight softening of sharp turns; default
    /// in InDesign.
    Rounded,
    /// Diffusion (noise-modulated alpha falloff). The rasterizer
    /// approximates this with a slightly randomised falloff weight.
    Diffusion,
}

/// Feather parameters. `width` is the soft-edge width in pt; `noise`
/// (0..=1) modulates the alpha falloff for the diffusion variant
/// (and is roughly ignored for `Sharp` / `Rounded`). `choke`
/// (0..=1) shifts the half-alpha point inward (0.0 centred on the
/// path edge, positive choke pulls the feather inward).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feather {
    pub width: f32,
    pub corner_type: FeatherCornerType,
    pub noise: f32,
    pub choke: f32,
}

impl Feather {
    pub fn default_soft() -> Self {
        Self {
            width: 9.0,
            corner_type: FeatherCornerType::Sharp,
            noise: 0.0,
            choke: 0.0,
        }
    }
}

/// Directional feather parameters. Mirrors IDML's
/// `<DirectionalFeatherSetting>` — each side of the path gets its
/// own soft-edge width in pt; `angle_deg` rotates the per-edge
/// directions. `noise` / `choke` and `corner_type` mirror the plain
/// [`Feather`] semantics.
///
/// The CPU rasterizer modulates alpha per side (left / right / top /
/// bottom) using the path's page-pt bounding box. The IDML `Angle`
/// attribute is captured so the rasterizer can lift to the rotated
/// per-side path later without a parser/compose round-trip; today
/// it's unused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalFeather {
    pub left_width: f32,
    pub right_width: f32,
    pub top_width: f32,
    pub bottom_width: f32,
    pub angle_deg: f32,
    pub noise: f32,
    pub choke: f32,
    pub corner_type: FeatherCornerType,
}

/// Gradient feather kind — mirrors IDML's `Type` attribute. `Linear`
/// projects each pixel onto the `(start, end)` axis to derive the
/// alpha; `Radial` uses `distance / radius` from the start point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GradientFeatherKind {
    #[default]
    Linear,
    Radial,
}

/// One stop of a [`GradientFeather`]'s alpha gradient. `location`
/// is in `[0, 1]` (0 = start, 1 = end); `alpha` is the opacity at
/// that location, also `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientFeatherStop {
    pub location: f32,
    pub alpha: f32,
}

/// Gradient feather parameters. The path's interior alpha is
/// modulated by a 1-D gradient defined by `stops` (sorted by
/// location); the gradient runs along
/// `(start_x, start_y) → (end_x, end_y)` for `Linear` and radially
/// out from `start` for `Radial` (`end` defines the radius). All
/// coords are in the path's local space — same coords as the
/// `Transform` that places the path.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientFeather {
    pub kind: GradientFeatherKind,
    pub start_x: f32,
    pub start_y: f32,
    pub end_x: f32,
    pub end_y: f32,
    pub stops: Vec<GradientFeatherStop>,
}

/// Pixel-space transform applied to an offscreen layer after the
/// commands inside it have rasterised, before it composites back onto
/// the parent target. The blur variant runs a separable Gaussian over
/// the layer's premultiplied RGBA buffer; `None` is a pure
/// transparency group (semantically identical to a `BeginBlendGroup`
/// pair, but emitted through the generic `PushLayer`/`PopLayer`
/// plumbing that future per-layer effects will share).
///
/// `GaussianBlur::sigma_pt` is in page-space pt — the CPU rasterizer
/// scales by `dpi / 72` to derive pixel σ, matching the existing
/// `DropShadow::blur_radius` semantic. The pipeline can build a
/// blur-only soft drop shadow with
/// `PushLayer { effect: LayerEffect::GaussianBlur { sigma_pt } } +
/// FillPath(shadow stamp) + PopLayer`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerEffect {
    /// No post-effect — equivalent to a plain transparency group.
    /// Carries `opacity` so a single command can express both grouping
    /// + a uniform fade (matching `BeginBlendGroup` semantics).
    None,
    /// Convolve the layer with a separable Gaussian of radius σ
    /// (in pt) before compositing. Edges clamp to transparent (the
    /// layer's bounds are padded by 3σ at composite time).
    GaussianBlur { sigma_pt: f32 },
}

impl LayerEffect {
    /// Effective σ in pt — `0.0` for non-blur variants. Lets callers
    /// pad the layer bounds uniformly.
    pub fn sigma_pt(&self) -> f32 {
        match self {
            LayerEffect::None => 0.0,
            LayerEffect::GaussianBlur { sigma_pt } => sigma_pt.max(0.0),
        }
    }
}

/// IDML compositing blend mode. `Normal` (the default, source-over
/// alpha composite) keeps the existing fast path; everything else
/// requires the rasterizer to render the fill into an offscreen
/// pixmap and composite onto the page with the named blend mode.
/// Names match Adobe / Skia conventions; map straight to
/// `tiny_skia::BlendMode` in the rasterizer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

/// One command in the display list.
#[derive(Debug, Clone)]
pub enum DisplayCommand {
    /// Fill a path with a paint, positioned by `transform`.
    FillPath {
        path_id: PathId,
        paint: Paint,
        transform: Transform,
    },
    /// Same as `FillPath` but composites onto the page with a
    /// non-Normal blend mode. Rasterizer routes this through an
    /// offscreen pixmap so the blend reads the page contents below.
    /// The fast `FillPath` path stays untouched for the common
    /// Normal case.
    FillPathBlend {
        path_id: PathId,
        paint: Paint,
        transform: Transform,
        blend_mode: BlendMode,
    },
    /// Stroke a path with a paint + stroke parameters, positioned by
    /// `transform`. Stroke width is in pt, *after* `transform` is
    /// applied — rasterizers pick up the document-space width from
    /// `stroke.width` rather than a scaled derivation of the path
    /// points.
    StrokePath {
        path_id: PathId,
        paint: Paint,
        stroke: Stroke,
        transform: Transform,
    },
    /// Drop-shadow stamp: render the path filled with `shadow.color`
    /// at `(offset_x, offset_y)` from the path's natural position.
    /// Conventionally emitted *before* the matching FillPath/StrokePath
    /// so the shadow lands behind the painted shape.
    DropShadow {
        path_id: PathId,
        transform: Transform,
        shadow: DropShadow,
    },
    /// Glyph- / arbitrary-path shadow stamp. Identical render
    /// semantics to [`DisplayCommand::DropShadow`] (fill `path_id`
    /// at `(offset_x, offset_y)` with a soft Gaussian-blurred
    /// stamp), separated as its own variant so the glyph-shadow
    /// post-pass can splice these in front of glyph `FillPath`
    /// commands without disturbing the rect-stamp variant emitted
    /// at frame-body time. The `transform` is the glyph's natural
    /// page-space transform — the rasterizer adds `shadow.offset_*`
    /// internally.
    PathShadow {
        path_id: PathId,
        transform: Transform,
        shadow: DropShadow,
    },
    /// Place a decoded RGBA8 image. The unit-rect at the source
    /// pixmap's pixel grid maps to page coordinates via `transform` —
    /// `(0, 0)` of the source pixmap lands at `transform.apply(0, 0)`,
    /// `(width, height)` lands at `transform.apply(width, height)`.
    /// Subsampling, filtering, and alpha blending live in the
    /// rasterizer.
    Image {
        image_id: ImageId,
        transform: Transform,
    },
    /// Push a clip path onto the rasterizer's clip stack. Subsequent
    /// drawing commands are masked to the *intersection* of every
    /// pushed clip until a matching `PopClip` lands. Paths are filled
    /// with `FillRule::NonZero` (matching IDML's path-geometry
    /// convention); the clip is anti-aliased.
    ///
    /// The transform maps `path_id` from its local space into page
    /// coordinates, exactly like `FillPath::transform`. The
    /// rasterizer multiplies in its page-to-pixel scale on top.
    ///
    /// Today only the CPU rasterizer enforces clips; the Vello
    /// backend currently no-ops them (matching its existing
    /// "unsupported feature ⇒ skip" behaviour for `Image` and
    /// `DropShadow`).
    PushClip { path_id: PathId, transform: Transform },
    /// Pop the most-recently-pushed clip. Mismatched Push/Pop pairs
    /// are tolerated — a stray `PopClip` drops back to the base
    /// (un-clipped) state. The contained transform is unused; it
    /// only exists so [`DisplayCommand::transform_mut`] can keep
    /// returning `&mut Transform` for every variant. Existing
    /// callers (vertical-justification etc.) walk command ranges
    /// that never include clip pairs.
    PopClip(Transform),
    /// Begin a transparency group. Subsequent drawing commands emit
    /// into an offscreen buffer sized to `bounds` (in page coords)
    /// instead of the page; a matching `EndBlendGroup` composites the
    /// buffer back onto the page (or the next-outer group) using
    /// `blend_mode` and `opacity`. This is the structurally correct
    /// PDF transparency-group semantic — non-Normal blend / partial
    /// opacity gets applied at the group composite, not per fill.
    ///
    /// `transform` is a stub (mirrors `PopClip`'s scheme) so
    /// [`DisplayCommand::transform_mut`] keeps returning a non-None
    /// reference. The field is initialised to identity by emitters
    /// and not consumed by the rasterizer.
    BeginBlendGroup {
        bounds: Rect,
        blend_mode: BlendMode,
        opacity: f32,
        transform: Transform,
    },
    /// End the most-recently-pushed transparency group. Mismatched
    /// pairs are tolerated — a stray `EndBlendGroup` is a no-op.
    /// The contained transform is unused; same rationale as
    /// [`DisplayCommand::PopClip`].
    EndBlendGroup(Transform),
    /// Inner-shadow stamp. Paints a soft, offset shadow on the
    /// *inside* of the path: blur the offset path's complement, mask
    /// to the path interior, composite over the page. Conventionally
    /// emitted *after* the matching FillPath so it sits on top of the
    /// fill (mirrors Photoshop's layer-effect order).
    InnerShadow {
        path_id: PathId,
        transform: Transform,
        params: InnerShadow,
    },
    /// Outer-glow stamp. Like a centred [`DisplayCommand::DropShadow`]
    /// (no offset) carved against the path's exterior. Conventionally
    /// emitted *before* the matching FillPath so the halo lands behind
    /// the fill.
    OuterGlow {
        path_id: PathId,
        transform: Transform,
        params: OuterGlow,
    },
    /// Inner-glow stamp. Same shape as [`DisplayCommand::InnerShadow`]
    /// but with no offset and a glow colour. Conventionally emitted
    /// *after* the matching FillPath.
    InnerGlow {
        path_id: PathId,
        transform: Transform,
        params: InnerGlow,
    },
    /// Bevel / emboss stamp. The rasterizer builds a height map from
    /// the path's alpha mask, derives a normal field, and composites
    /// per-pixel highlight + shadow tints onto the path's interior.
    /// Conventionally emitted *after* the matching FillPath.
    BevelEmboss {
        path_id: PathId,
        transform: Transform,
        params: BevelEmboss,
    },
    /// Satin stamp. Two offset blurred path stamps subtracted to
    /// produce a "wave" mask, tinted with `params.color` and
    /// composited onto the path's interior. Conventionally emitted
    /// *after* the matching FillPath.
    Satin {
        path_id: PathId,
        transform: Transform,
        params: Satin,
    },
    /// Feather stamp. Replaces the path's hard edge with a soft alpha
    /// falloff `params.width` pt wide. Conventionally emitted *in
    /// place of* the matching FillPath (the feather pass is the
    /// fill); a dedicated variant lets the rasterizer skip the
    /// expensive distance-field pass when no feathering is requested.
    Feather {
        path_id: PathId,
        transform: Transform,
        params: Feather,
    },
    /// Directional feather stamp. Same render contract as
    /// [`DisplayCommand::Feather`] but with per-edge widths instead
    /// of a uniform width — the rasterizer modulates alpha by the
    /// distance to each side independently.
    DirectionalFeather {
        path_id: PathId,
        transform: Transform,
        params: DirectionalFeather,
    },
    /// Gradient feather stamp. Same render contract as
    /// [`DisplayCommand::Feather`] but the alpha falloff comes from
    /// a 1-D gradient (`Linear` or `Radial`) sampled along
    /// `(start, end)` in the path's local space.
    GradientFeather {
        path_id: PathId,
        transform: Transform,
        params: GradientFeather,
    },
    /// Push a generic transparency / effect layer. Subsequent drawing
    /// commands emit into an offscreen buffer sized to `bounds` (page
    /// coords, padded internally by `3σ + 1px` so a Gaussian-blur
    /// kernel doesn't clip its tails); the matching [`PopLayer`]
    /// applies `effect` to the buffer and composites it onto the
    /// parent target with `blend_mode` and `opacity`.
    ///
    /// `PushLayer` is the structural successor to
    /// [`BeginBlendGroup`](DisplayCommand::BeginBlendGroup): the
    /// blend-group variant predates `LayerEffect` and is kept for the
    /// per-frame paper-backdrop-bypass path that the orchestrator
    /// already relies on. `PushLayer` is the right primitive for
    /// effect-driven layers (soft drop shadows, future glow refactors)
    /// because the effect runs *after* the contents are buffered, so
    /// the blur kernel sees the layer's full alpha (overlapping
    /// stamps included) instead of the parent target's alpha.
    ///
    /// `transform` is a stub (matches `BeginBlendGroup`'s scheme) so
    /// [`DisplayCommand::transform_mut`] keeps returning a non-None
    /// reference. Emitters initialise it to identity; the rasterizer
    /// never consumes it.
    PushLayer {
        bounds: Rect,
        effect: LayerEffect,
        blend_mode: BlendMode,
        opacity: f32,
        transform: Transform,
    },
    /// Pop the most-recently-pushed [`PushLayer`]. Mismatched pairs
    /// are tolerated (a stray `PopLayer` is a no-op), matching the
    /// `BeginBlendGroup` / `EndBlendGroup` policy. The contained
    /// transform is unused; same rationale as
    /// [`DisplayCommand::PopClip`].
    PopLayer(Transform),
    /// Fill a path with overprint semantics. Identical wire-format to
    /// [`DisplayCommand::FillPath`] (path / paint / transform), but the
    /// rasterizer composites the result with a per-channel
    /// `min(top, bottom)` darken instead of standard alpha blending.
    /// This is a CMYK-overprint *approximation* done in RGB: it's
    /// visibly correct for dark ink on lighter background and
    /// black-on-tints (the common real-world overprint cases) but not
    /// a true per-channel CMYK composite — that's deferred until the
    /// rasterizer carries separations end to end (see Phase 3 Tier 3
    /// #14, Stage 4).
    ///
    /// Lifted to a distinct variant rather than a flag on `FillPath`
    /// so the common knock-out path stays one variant + one rasterizer
    /// arm with no per-command branch, and so existing construction
    /// sites for `FillPath` need no churn.
    FillPathOverprint {
        path_id: PathId,
        paint: Paint,
        transform: Transform,
    },
    /// Stroke a path with overprint semantics. See
    /// [`DisplayCommand::FillPathOverprint`] for the rasterizer
    /// approximation.
    StrokePathOverprint {
        path_id: PathId,
        paint: Paint,
        stroke: Stroke,
        transform: Transform,
    },
}

impl DisplayCommand {
    /// Mutable accessor for the command's placement transform.
    /// Used by post-emit passes (vertical justification, future
    /// layered effects) that need to translate / re-anchor a range
    /// of commands without inspecting variants individually.
    pub fn transform_mut(&mut self) -> &mut Transform {
        match self {
            DisplayCommand::FillPath { transform, .. }
            | DisplayCommand::FillPathBlend { transform, .. }
            | DisplayCommand::StrokePath { transform, .. }
            | DisplayCommand::DropShadow { transform, .. }
            | DisplayCommand::PathShadow { transform, .. }
            | DisplayCommand::Image { transform, .. }
            | DisplayCommand::PushClip { transform, .. }
            | DisplayCommand::PopClip(transform)
            | DisplayCommand::BeginBlendGroup { transform, .. }
            | DisplayCommand::EndBlendGroup(transform)
            | DisplayCommand::InnerShadow { transform, .. }
            | DisplayCommand::OuterGlow { transform, .. }
            | DisplayCommand::InnerGlow { transform, .. }
            | DisplayCommand::BevelEmboss { transform, .. }
            | DisplayCommand::Satin { transform, .. }
            | DisplayCommand::Feather { transform, .. }
            | DisplayCommand::DirectionalFeather { transform, .. }
            | DisplayCommand::GradientFeather { transform, .. }
            | DisplayCommand::PushLayer { transform, .. }
            | DisplayCommand::PopLayer(transform)
            | DisplayCommand::FillPathOverprint { transform, .. }
            | DisplayCommand::StrokePathOverprint { transform, .. } => transform,
        }
    }
}

/// Index into [`DisplayList::images`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// One placed image's decoded RGBA8 pixels. The pipeline decodes
/// once per (uri, dpi) and stores the result here so repeat
/// placements share the buffer.
///
/// Cloning is cheap (both `encoded` and `rgba` are `Bytes` — refcount
/// bumps, not memcpys), so the renderer can dedup an image across
/// many `Image` commands without bloating the heap.
///
/// Lazy decoding: when `rgba.is_empty()` the rasterizer must decode
/// `encoded` on demand and discard the result after composition. This
/// is the path the wasm32 build takes for envato megapacks (~50MB
/// embedded images) where decoded RGBA would blow the 4GB
/// address-space cap. `encoded` may be empty for synthetic images
/// built directly from RGBA — in that case the rasterizer falls
/// through to the pre-decoded `rgba`.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// Original encoded bytes (PNG / JPEG / WebP / …). Held so a
    /// lazy rasterizer can decode + drop on demand. Empty when the
    /// image was constructed directly from a pre-decoded RGBA8 buffer
    /// (e.g. synthetic placeholders, tests).
    pub encoded: bytes::Bytes,
    /// Tightly packed RGBA8 (4 bytes per pixel, row-major). Length
    /// must equal `width * height * 4` when populated. Empty when
    /// the rasterizer is expected to decode `encoded` lazily.
    pub rgba: bytes::Bytes,
    /// Concept 3 — the image's embedded ICC profile (JPEG APP2 /
    /// PNG iCCP) when the decoder retained it. The PDF exporter
    /// tags the image XObject with it so placed assets keep their
    /// colour space (concept E7); rasterizers ignore it. `None`
    /// default — every existing constructor stays valid.
    pub icc: Option<bytes::Bytes>,
}

/// Stroke parameters. Widths are in pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// Optional dash pattern in pt: alternating on/off lengths. Empty
    /// means solid. The rasterizer is responsible for cycling through
    /// the array per stroked path.
    pub dash: DashPattern,
}

/// Up to four on/off pairs (eight slots) cover IDML's preset stroke
/// styles (Solid, Dashed, Dotted, Dashed3-2, etc.) without
/// allocating. Anything richer falls back to `Solid` until the
/// parser learns custom `<StrokeStyle>` definitions.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DashPattern {
    /// Number of valid entries in `pattern`.
    pub len: u8,
    /// Up to 8 entries; only `pattern[..len]` is meaningful. An empty
    /// pattern means solid.
    pub pattern: [f32; 8],
}

impl DashPattern {
    pub fn from_slice(values: &[f32]) -> Self {
        let mut out = Self::default();
        for (slot, v) in out.pattern.iter_mut().zip(values.iter()) {
            *slot = *v;
        }
        out.len = values.len().min(8) as u8;
        out
    }

    pub fn is_solid(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.pattern[..self.len as usize]
    }
}

impl Stroke {
    /// Minimal defaults: `width` set by caller, butt caps, miter
    /// joins, miter_limit=4.0 (PDF default), solid dash.
    pub fn new(width: f32) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dash: DashPattern::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

/// Concept 3 (PDF export) — one captured glyph: the side-channel
/// record paralleling the outline `FillPath`/`StrokePath` command at
/// `command_index`. Captured ONLY when
/// `PipelineOptions::collect_glyph_runs` is set (the canvas default
/// is off — the live render path never pays for this); the PDF
/// backend skips the outline command and emits real text (`Tf`/`Tm`/
/// `TJ`) instead, with the unicode codepoint feeding `/ToUnicode`.
#[derive(Debug, Clone)]
pub struct GlyphRunEntry {
    /// Index of the parallel outline command in
    /// `DisplayList::commands`.
    pub command_index: u32,
    /// The renderer's font-table id (resolves to face bytes).
    pub font_id: u32,
    pub glyph_id: u32,
    pub font_size: f32,
    /// The EXACT affine the outline command received — reused
    /// verbatim as the PDF text matrix so text lands
    /// pixel-identical to the outline it replaces.
    pub transform: Transform,
    pub paint: Paint,
    /// The character(s) this glyph represents, for `/ToUnicode`.
    /// Single char covers the common case; ligatures carry the
    /// first char v1 (multi-char mapping is a refinement).
    pub unicode: Option<char>,
    pub is_stroke: bool,
}

/// Concept 3 — the per-list glyph capture (see [`GlyphRunEntry`]).
#[derive(Debug, Default)]
pub struct GlyphRunTable {
    pub entries: Vec<GlyphRunEntry>,
}

impl GlyphRunTable {
    pub fn push(&mut self, entry: GlyphRunEntry) {
        self.entries.push(entry);
    }
}

#[derive(Debug, Default)]
pub struct DisplayList {
    pub paths: PathBuffer,
    pub commands: Vec<DisplayCommand>,
    pub gradients: Vec<LinearGradient>,
    pub radial_gradients: Vec<RadialGradient>,
    pub images: Vec<DecodedImage>,
    /// Named spot inks the document references. Indexed by
    /// [`SpotInkId`]. Two `Paint::Cmyk` paints carrying the same spot
    /// name intern to the same id so the rasterizer can identify
    /// spot-on-same-spot overprints (which compose per-pixel `max`) vs.
    /// different-named-ink overprints (which accumulate independently).
    pub spot_inks: Vec<SpotInk>,
    /// Concept 3 — glyph-run side-channel for the PDF exporter.
    /// `None` unless the build ran with
    /// `PipelineOptions::collect_glyph_runs`; rasterizers never read
    /// it.
    pub glyph_runs: Option<GlyphRunTable>,
}

impl DisplayList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, cmd: DisplayCommand) {
        self.commands.push(cmd);
    }

    /// Append a linear gradient and return its id.
    pub fn push_linear_gradient(&mut self, g: LinearGradient) -> GradientId {
        let id = GradientId(self.gradients.len() as u32);
        self.gradients.push(g);
        id
    }

    pub fn linear_gradient(&self, id: GradientId) -> Option<&LinearGradient> {
        self.gradients.get(id.0 as usize)
    }

    /// Append a radial gradient and return its id.
    pub fn push_radial_gradient(&mut self, g: RadialGradient) -> GradientId {
        let id = GradientId(self.radial_gradients.len() as u32);
        self.radial_gradients.push(g);
        id
    }

    pub fn radial_gradient(&self, id: GradientId) -> Option<&RadialGradient> {
        self.radial_gradients.get(id.0 as usize)
    }

    /// Append a decoded image and return its id. Callers are expected
    /// to dedupe before calling — the buffer is a Vec, not a hash
    /// map, since image bytes don't have a cheap hash.
    pub fn push_image(&mut self, img: DecodedImage) -> ImageId {
        let id = ImageId(self.images.len() as u32);
        self.images.push(img);
        id
    }

    pub fn image(&self, id: ImageId) -> Option<&DecodedImage> {
        self.images.get(id.0 as usize)
    }

    /// Intern a spot ink name. Returns the existing id if the document
    /// already registered an ink with that `name`; otherwise pushes the
    /// `ink` and returns the freshly minted id. Dedup keeps the spot
    /// plane count proportional to the document's distinct named inks
    /// rather than to the number of `Paint::Cmyk` constructions.
    pub fn push_spot_ink(&mut self, ink: SpotInk) -> SpotInkId {
        if let Some((i, _)) = self
            .spot_inks
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == ink.name)
        {
            return SpotInkId(i as u32);
        }
        let id = SpotInkId(self.spot_inks.len() as u32);
        self.spot_inks.push(ink);
        id
    }

    pub fn spot_ink(&self, id: SpotInkId) -> Option<&SpotInk> {
        self.spot_inks.get(id.0 as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_translate_applied_to_point() {
        let t = Transform::translate(3.0, 4.0);
        assert_eq!(t.apply(1.0, 2.0), (4.0, 6.0));
    }

    #[test]
    fn transform_scale_applied_to_point() {
        let t = Transform::scale(2.0, 3.0);
        assert_eq!(t.apply(5.0, 7.0), (10.0, 21.0));
    }

    #[test]
    fn transform_compose_applies_outer_after_inner() {
        // inner first scales by 2x, outer translates by (10, 20).
        let inner = Transform::scale(2.0, 2.0);
        let outer = Transform::translate(10.0, 20.0);
        let composed = outer.compose(&inner);
        // Point (3, 4) → inner → (6, 8) → outer → (16, 28).
        assert_eq!(composed.apply(3.0, 4.0), (16.0, 28.0));
    }

    #[test]
    fn transform_compose_with_identity_is_a_noop() {
        let t = Transform([2.0, 0.5, -0.5, 2.0, 7.0, 11.0]);
        assert_eq!(Transform::IDENTITY.compose(&t).0, t.0);
        assert_eq!(t.compose(&Transform::IDENTITY).0, t.0);
    }

    #[test]
    fn path_buffer_interns_by_key() {
        let mut pb = PathBuffer::new();
        let key = 42u64;
        let (id1, fresh1) = pb.intern(
            key,
            PathData {
                segments: vec![PathSegment::MoveTo { x: 0.0, y: 0.0 }],
            },
        );
        assert!(fresh1);
        let (id2, fresh2) = pb.intern(key, PathData::default());
        assert!(!fresh2, "second intern under same key should not store");
        assert_eq!(id1, id2);
        assert_eq!(pb.len(), 1);
    }

    #[test]
    fn path_buffer_anon_does_not_collide() {
        let mut pb = PathBuffer::new();
        let a = pb.push_anon(PathData::default());
        let b = pb.push_anon(PathData::default());
        assert_ne!(a, b);
        assert_eq!(pb.len(), 2);
    }
}
