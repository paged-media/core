//! Shaping helper: wraps `rustybuzz::shape` and scales to point units.
//!
//! Every homogeneous character run (single font, size, feature set) goes
//! through this function. Output is a `ShapedRun` whose advances are in
//! 1/64 pt so downstream layout can stay in integer arithmetic.

use rustybuzz::{Face, UnicodeBuffer};

/// Advance precision: 1/64 pt, matching the composer spike.
pub const ADVANCE_PRECISION: f32 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub glyph_id: u32,
    /// Byte offset within the shaped input that produced this glyph.
    pub cluster: u32,
    /// Horizontal advance, 1/64 pt.
    pub x_advance: i32,
    /// Vertical offset applied at render time, 1/64 pt.
    pub y_offset: i32,
    /// Horizontal offset applied at render time, 1/64 pt.
    pub x_offset: i32,
}

#[derive(Debug, Clone)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    /// Sum of all x_advance values; convenience for line-break width.
    pub total_advance: i32,
}

/// Shape a text run with the given face and point size.
///
/// `text` must already be a single homogeneous run (one font, one size,
/// one language, one direction); the caller is responsible for segmenting
/// paragraphs into such runs.
pub fn shape_run(face: &Face, text: &str, point_size: f32) -> ShapedRun {
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    let shaped = rustybuzz::shape(face, &[], buf);

    let units_per_em = face.units_per_em() as f32;
    let scale = point_size * ADVANCE_PRECISION / units_per_em;
    let to_fp64 = |u: i32| -> i32 { ((u as f32) * scale).round() as i32 };

    let infos = shaped.glyph_infos();
    let positions = shaped.glyph_positions();
    debug_assert_eq!(infos.len(), positions.len());

    let mut glyphs = Vec::with_capacity(infos.len());
    let mut total = 0i32;
    for (info, pos) in infos.iter().zip(positions.iter()) {
        let adv = to_fp64(pos.x_advance);
        total += adv;
        glyphs.push(ShapedGlyph {
            glyph_id: info.glyph_id,
            cluster: info.cluster,
            x_advance: adv,
            y_offset: to_fp64(pos.y_offset),
            x_offset: to_fp64(pos.x_offset),
        });
    }

    ShapedRun {
        glyphs,
        total_advance: total,
    }
}
