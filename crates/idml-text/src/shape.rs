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

    // Drop control-char glyphs (.notdef "tofu" boxes from ASCII LF /
    // CR / ZWSP / line / paragraph separators). IDML's <Br/> lands
    // as `\n` in run text; rustybuzz emits a notdef rectangle for
    // it. Cluster byte offsets stay valid because we only filter
    // by the *source* byte, not by reordering.
    let bytes = text.as_bytes();
    let is_control_at = |cluster: u32| -> bool {
        let i = cluster as usize;
        if i >= bytes.len() {
            return false;
        }
        match bytes[i] {
            b'\n' | b'\r' | 0x0B | 0x0C => true,
            // U+2028 LINE SEP and U+2029 PARA SEP are 3-byte UTF-8
            // starting with 0xE2 0x80 0xA8 / 0xA9. Cheap prefix
            // check without allocating a chars iterator.
            0xE2 if i + 2 < bytes.len() && bytes[i + 1] == 0x80 => {
                matches!(bytes[i + 2], 0xA8 | 0xA9)
            }
            _ => false,
        }
    };

    let mut glyphs = Vec::with_capacity(infos.len());
    let mut total = 0i32;
    for (info, pos) in infos.iter().zip(positions.iter()) {
        if is_control_at(info.cluster) {
            continue;
        }
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

/// Apply InDesign-style letter-spacing (Tracking) to an already-shaped
/// run. Tracking is in 1/1000 em units (the IDML convention) — at
/// `point_size` pt and 1/64 pt advance precision, every glyph's
/// x_advance gets `tracking * point_size * 64 / 1000` added.
///
/// Tracking is a post-shape adjustment: it doesn't change shaping
/// decisions (kerning, ligatures), only the per-glyph advances. The
/// composer's column fit therefore still measures with tracking
/// applied — `total_advance` is updated in lockstep.
pub fn apply_tracking(run: &mut ShapedRun, tracking_thousandths_em: f32, point_size: f32) {
    if tracking_thousandths_em == 0.0 {
        return;
    }
    let extra = (tracking_thousandths_em * point_size * ADVANCE_PRECISION / 1000.0).round() as i32;
    if extra == 0 {
        return;
    }
    let mut total = 0i32;
    for glyph in &mut run.glyphs {
        glyph.x_advance += extra;
        total += glyph.x_advance;
    }
    run.total_advance = total;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(advances: &[i32]) -> ShapedRun {
        let glyphs: Vec<ShapedGlyph> = advances
            .iter()
            .enumerate()
            .map(|(i, &x_advance)| ShapedGlyph {
                glyph_id: i as u32,
                cluster: i as u32,
                x_advance,
                y_offset: 0,
                x_offset: 0,
            })
            .collect();
        ShapedRun {
            glyphs,
            total_advance: advances.iter().sum(),
        }
    }

    #[test]
    fn zero_tracking_is_a_noop() {
        let mut r = run(&[100, 80, 120]);
        let original = r.total_advance;
        apply_tracking(&mut r, 0.0, 12.0);
        assert_eq!(r.total_advance, original);
        assert_eq!(r.glyphs[0].x_advance, 100);
    }

    #[test]
    fn positive_tracking_widens_every_advance() {
        // 100/1000 em at 12pt → 1.2 pt × 64 = 76.8 → 77 per glyph.
        let mut r = run(&[100, 80, 120]);
        apply_tracking(&mut r, 100.0, 12.0);
        assert_eq!(r.glyphs[0].x_advance, 100 + 77);
        assert_eq!(r.glyphs[1].x_advance, 80 + 77);
        assert_eq!(r.glyphs[2].x_advance, 120 + 77);
        assert_eq!(r.total_advance, 100 + 80 + 120 + 3 * 77);
    }

    #[test]
    fn negative_tracking_tightens_advance() {
        let mut r = run(&[200, 200]);
        apply_tracking(&mut r, -50.0, 12.0);
        assert!(r.glyphs[0].x_advance < 200);
        assert_eq!(r.total_advance, 2 * r.glyphs[0].x_advance);
    }
}
