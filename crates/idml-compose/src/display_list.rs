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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Solid(Color),
    LinearGradient(GradientId),
    /// Radial gradient — `center → center + radius` in unit-rect
    /// coords. Same id-space as linear gradients but resolved against
    /// `DisplayList::radial_gradients` instead of `gradients`.
    RadialGradient(GradientId),
}

/// Index into `DisplayList::gradients` *or* `DisplayList::radial_gradients`,
/// depending on the [`Paint`] variant carrying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GradientId(pub u32);

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
    // PushLayer, PopLayer land with §10.4.
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
            | DisplayCommand::Feather { transform, .. } => transform,
        }
    }
}

/// Index into [`DisplayList::images`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// One placed image's decoded RGBA8 pixels. The pipeline decodes
/// once per (uri, dpi) and stores the result here so repeat
/// placements share the buffer.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 (4 bytes per pixel, row-major). Length
    /// must equal `width * height * 4`.
    pub rgba: Vec<u8>,
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

#[derive(Debug, Default)]
pub struct DisplayList {
    pub paths: PathBuffer,
    pub commands: Vec<DisplayCommand>,
    pub gradients: Vec<LinearGradient>,
    pub radial_gradients: Vec<RadialGradient>,
    pub images: Vec<DecodedImage>,
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
