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
}

/// Index into `DisplayList::gradients`.
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

/// One command in the display list.
#[derive(Debug, Clone)]
pub enum DisplayCommand {
    /// Fill a path with a paint, positioned by `transform`.
    FillPath {
        path_id: PathId,
        paint: Paint,
        transform: Transform,
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
    // DrawImage, PushLayer, PopLayer, PushClip, PopClip land with
    // §10.3 / §10.4.
}

/// Stroke parameters. Widths are in pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
}

impl Stroke {
    /// Minimal defaults: `width` set by caller, butt caps, miter
    /// joins, miter_limit=4.0 (PDF default).
    pub fn new(width: f32) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
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
