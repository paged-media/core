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
///
/// Equivalent to [`shape_run_with_features`] with `features` =
/// [`ShapingFeatures::default()`] (rustybuzz's defaults: kerning + standard
/// ligatures on, discretionary ligatures off — the same set the OpenType
/// spec says fonts opt into by default). Kept as a no-features wrapper
/// because the calibration spike, the optical-margin pass, and several
/// tests want the simplest possible call site.
pub fn shape_run(face: &Face, text: &str, point_size: f32) -> ShapedRun {
    shape_run_with_features(face, text, point_size, ShapingFeatures::default())
}

/// Phase 4 typography — OpenType feature toggles. Currently exposes
/// the two that ship in InDesign's character-style UI today: standard
/// ligatures (`liga` / `clig`) and kerning (`kern`). Other features
/// (discretionary ligatures, contextual alternates, swashes,
/// stylistic sets) can be added as needed.
///
/// The default is "shape exactly like the bare `shape_run` did before
/// Phase 4 landed" so existing call sites change behaviour only when
/// they explicitly opt in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapingFeatures {
    /// `LigaturesOn`. When false, standard + contextual ligatures
    /// (`liga`, `clig`) are disabled. Discretionary ligatures stay
    /// off either way (separate IDML attribute, not yet wired).
    pub ligatures_on: bool,
    /// `KerningMethod`. When `Off`, the `kern` OpenType feature is
    /// disabled and shape advances reflect the font's bare metrics.
    /// `Metrics` (default) lets rustybuzz apply OpenType kerning;
    /// `Optical` falls through to Metrics for now — InDesign's
    /// optical kerning would need a separate pass over glyph
    /// outlines and is queued.
    pub kerning: KerningMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KerningMethod {
    /// OpenType `kern` feature on. InDesign default.
    #[default]
    Metrics,
    /// Optical kerning — fall back to Metrics until the outline-
    /// driven pass lands.
    Optical,
    /// Disable kerning entirely.
    Off,
}

impl Default for ShapingFeatures {
    fn default() -> Self {
        Self {
            ligatures_on: true,
            kerning: KerningMethod::Metrics,
        }
    }
}

impl ShapingFeatures {
    fn to_rustybuzz(self) -> Vec<rustybuzz::Feature> {
        let mut out: Vec<rustybuzz::Feature> = Vec::new();
        if !self.ligatures_on {
            // Tag = `liga`, value 0 = off. Same for `clig`.
            out.push(rustybuzz::Feature::new(
                ttf_parser::Tag::from_bytes(b"liga"),
                0,
                ..,
            ));
            out.push(rustybuzz::Feature::new(
                ttf_parser::Tag::from_bytes(b"clig"),
                0,
                ..,
            ));
        }
        if matches!(self.kerning, KerningMethod::Off) {
            out.push(rustybuzz::Feature::new(
                ttf_parser::Tag::from_bytes(b"kern"),
                0,
                ..,
            ));
        }
        out
    }
}

/// Shape `text` with explicit OpenType feature toggles.
///
/// The base call is identical to [`shape_run`]; this entry exists so the
/// pipeline can pass the resolved `LigaturesOn` / `KerningMethod` from a
/// `CharacterRun` without every call site having to construct a
/// `ShapingFeatures` when the defaults are fine.
pub fn shape_run_with_features(
    face: &Face,
    text: &str,
    point_size: f32,
    features: ShapingFeatures,
) -> ShapedRun {
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    let rb_features = features.to_rustybuzz();
    let shaped = rustybuzz::shape(face, &rb_features, buf);

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

/// Which margin a glyph sits against. The optical-margin trim table
/// is asymmetric — a comma at the right edge hangs *outward* (positive
/// shift past the column edge) so the visual margin reads straight,
/// while at the left edge it hangs *inward* the same distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginSide {
    Left,
    Right,
}

/// Optical-margin trim factor for a single codepoint, expressed as a
/// fraction of the run's point size. Returns `0.0` for glyphs that
/// shouldn't hang (the common case).
///
/// IDML's `<StoryPreference OpticalMarginAlignment="true"
/// OpticalMarginSize="12">` enables hanging punctuation: at a
/// paragraph's left and right margins, certain glyphs (commas,
/// periods, dashes, hyphens, quotes) shift slightly outward so the
/// optical edge of the column reads straight to the eye.
///
/// This table is a coarse approximation — Adobe's published values
/// are font-specific (per-glyph metrics tables in the font), but a
/// font-independent table covers ~90% of the visual win for the
/// typefaces we exercise. The trim factors below match published
/// rules of thumb (Bringhurst, *Elements of Typographic Style*; Adobe
/// optical-margin examples).
///
/// `point_size` here is the run's pt size — the result is *not* in
/// 1/64 pt, callers scale appropriately.
pub fn optical_margin_offset(codepoint: char, side: MarginSide, point_size: f32) -> f32 {
    let factor = trim_factor(codepoint, side);
    factor * point_size
}

/// Lookup of trim factors. Public so callers building their own
/// margin-trim passes can consult the table without going through
/// the per-call multiplication.
fn trim_factor(c: char, side: MarginSide) -> f32 {
    // The asymmetry: opening punctuation hangs more at the left,
    // closing punctuation more at the right, but for the common
    // ASCII set the two sides agree on most glyphs. Where Adobe's
    // documentation differs we keep the same value either side and
    // let the layer above tune.
    match c {
        // Period / comma — hang ~half their point size.
        '.' | ',' => 0.5,
        // En / em dash and hyphen — modest hang.
        '-' | '\u{2013}' | '\u{2014}' => 0.3,
        // Hyphen-minus visual variants.
        '\u{2010}' | '\u{2011}' => 0.3,
        // ASCII straight quotes.
        '"' | '\'' => 0.5,
        // Curly quotes — left/right pairs hang on their natural side
        // but we apply the same factor either way; the layer above
        // never asks for a left-side optical trim of a closing quote.
        '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' => 0.5,
        // Guillemets (French quotes).
        '\u{00AB}' | '\u{00BB}' => 0.4,
        // Colon / semicolon — small hang.
        ':' | ';' => 0.2,
        // Bullet / middot — small hang.
        '\u{2022}' | '\u{00B7}' => 0.2,
        // Inter-word space at the right margin only — never on the
        // left (a leading space is a paragraph-indent, not optical
        // margin). This handles the trailing-space case where the
        // shaped line happens to end with a space glyph.
        ' ' if side == MarginSide::Right => 0.5,
        _ => 0.0,
    }
}

/// Adjust the leftmost / rightmost glyphs in `glyphs` for optical
/// margin alignment. The leftmost glyph's `x_offset` shifts
/// *negative* (hangs outward past the column's left edge); the
/// rightmost glyph's `x_advance` shrinks (so the next glyph would
/// sit further right, but for the rightmost glyph the result is the
/// line *natural width* shrinks, letting the alignment pass push the
/// rest of the line further out).
///
/// `point_size` is the shaping point size. `optical_margin_size_pt`
/// is the IDML `OpticalMarginSize` attribute (typically 12pt) — when
/// non-zero it scales the trim. We treat any non-zero value as
/// "trim at the table's natural strength scaled by min(point_size,
/// optical_margin_size_pt) / point_size". This matches Adobe's
/// behaviour where the OpticalMarginSize bounds how far smaller-than-
/// margin-size glyphs hang.
///
/// Caller responsibility: the source codepoint of a glyph isn't
/// stored in `ShapedRun` (only the cluster). The caller passes the
/// first / last codepoint via `leftmost_char` / `rightmost_char`.
/// This keeps shape.rs from needing the source string.
pub fn apply_optical_margin(
    run: &mut ShapedRun,
    leftmost_char: Option<char>,
    rightmost_char: Option<char>,
    point_size: f32,
    optical_margin_size_pt: f32,
) {
    if run.glyphs.is_empty() {
        return;
    }
    // OpticalMarginSize bounds the hang for glyphs smaller than the
    // configured size: at 12pt margin and 6pt glyphs, the hang is
    // half what the table says. At point_size >= margin_size, full
    // hang. At margin_size <= 0, the feature is off.
    if optical_margin_size_pt <= 0.0 {
        return;
    }
    let scale = if point_size >= optical_margin_size_pt {
        1.0
    } else {
        point_size / optical_margin_size_pt
    };
    if let Some(c) = leftmost_char {
        let off_pt = optical_margin_offset(c, MarginSide::Left, point_size) * scale;
        if off_pt != 0.0 {
            let off_64 = (off_pt * ADVANCE_PRECISION).round() as i32;
            // Hang outward: shift the glyph left by `off_64`. We
            // apply via `x_offset` so the run's `total_advance`
            // (sum of advances) is unchanged — the alignment pass
            // still sees the same natural width.
            if let Some(g) = run.glyphs.first_mut() {
                g.x_offset -= off_64;
            }
        }
    }
    if let Some(c) = rightmost_char {
        let off_pt = optical_margin_offset(c, MarginSide::Right, point_size) * scale;
        if off_pt != 0.0 {
            let off_64 = (off_pt * ADVANCE_PRECISION).round() as i32;
            // Right-side hang: shrink the rightmost glyph's
            // *advance* so the line's natural width drops by
            // `off_64`. The glyph itself paints at the same
            // position — we only steal trailing whitespace from
            // the column. `total_advance` updates in lockstep so
            // alignment / justification sees the trimmed width.
            if let Some(g) = run.glyphs.last_mut() {
                let trim = off_64.min(g.x_advance);
                g.x_advance -= trim;
                run.total_advance -= trim;
            }
        }
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

    #[test]
    fn optical_margin_table_known_glyphs() {
        // Period / comma at the right margin hang half their pt size.
        let off_period = optical_margin_offset('.', MarginSide::Right, 12.0);
        assert!((off_period - 6.0).abs() < 1e-6, "{}", off_period);
        let off_comma = optical_margin_offset(',', MarginSide::Right, 12.0);
        assert!((off_comma - 6.0).abs() < 1e-6, "{}", off_comma);
        // Hyphen / dashes at 0.3 of pt size.
        let off_hyphen = optical_margin_offset('-', MarginSide::Right, 10.0);
        assert!((off_hyphen - 3.0).abs() < 1e-6, "{}", off_hyphen);
        let off_endash = optical_margin_offset('\u{2013}', MarginSide::Right, 10.0);
        assert!((off_endash - 3.0).abs() < 1e-6, "{}", off_endash);
        // Quote 0.5.
        let off_quote = optical_margin_offset('"', MarginSide::Left, 10.0);
        assert!((off_quote - 5.0).abs() < 1e-6, "{}", off_quote);
        // Letter — no hang.
        let off_a = optical_margin_offset('a', MarginSide::Right, 12.0);
        assert_eq!(off_a, 0.0);
        // Space hangs only on the right margin (trailing whitespace).
        assert_eq!(optical_margin_offset(' ', MarginSide::Left, 12.0), 0.0);
        assert!((optical_margin_offset(' ', MarginSide::Right, 12.0) - 6.0).abs() < 1e-6);
    }

    #[test]
    fn apply_optical_margin_disabled_when_size_zero() {
        let mut r = run(&[100, 80, 120]);
        let original_total = r.total_advance;
        let original_first_offset = r.glyphs[0].x_offset;
        apply_optical_margin(&mut r, Some('"'), Some('.'), 12.0, 0.0);
        assert_eq!(r.total_advance, original_total);
        assert_eq!(r.glyphs[0].x_offset, original_first_offset);
    }

    #[test]
    fn apply_optical_margin_hangs_left_glyph_outward() {
        // Three glyphs, leftmost is a quote. At 12pt with 12pt
        // margin, the trim is 0.5 * 12 = 6.0 pt = 384 in 1/64pt.
        let mut r = run(&[100, 80, 120]);
        apply_optical_margin(&mut r, Some('"'), None, 12.0, 12.0);
        assert_eq!(r.glyphs[0].x_offset, -384);
        // Total advance unchanged (we only moved x_offset).
        assert_eq!(r.total_advance, 300);
    }

    #[test]
    fn apply_optical_margin_trims_right_glyph_advance() {
        // Last glyph is a period. At 12pt with 12pt margin,
        // trim = 0.5 * 12 = 6.0 pt = 384 in 1/64pt. But the
        // glyph's advance is only 120, so we cap at 120.
        let mut r = run(&[100, 80, 120]);
        apply_optical_margin(&mut r, None, Some('.'), 12.0, 12.0);
        assert_eq!(r.glyphs[2].x_advance, 0);
        assert_eq!(r.total_advance, 100 + 80);
    }

    #[test]
    fn apply_optical_margin_scales_below_margin_size() {
        // Glyph at 6pt with 12pt margin → half trim.
        let mut r = run(&[100, 80, 1000]);
        apply_optical_margin(&mut r, None, Some('.'), 6.0, 12.0);
        // 0.5 * 6.0 * (6.0/12.0) = 1.5 pt = 96 in 1/64pt.
        assert_eq!(r.glyphs[2].x_advance, 1000 - 96);
        assert_eq!(r.total_advance, 100 + 80 + (1000 - 96));
    }

    #[test]
    fn apply_optical_margin_noop_for_letters() {
        let mut r = run(&[100, 80, 120]);
        let total = r.total_advance;
        apply_optical_margin(&mut r, Some('a'), Some('z'), 12.0, 12.0);
        assert_eq!(r.total_advance, total);
        assert_eq!(r.glyphs[0].x_offset, 0);
        assert_eq!(r.glyphs[2].x_advance, 120);
    }

    #[test]
    fn shaping_features_default_passes_empty_feature_list() {
        let f = ShapingFeatures::default();
        assert_eq!(f.to_rustybuzz().len(), 0);
    }

    #[test]
    fn shaping_features_disable_ligatures_adds_two_off_tags() {
        let f = ShapingFeatures {
            ligatures_on: false,
            ..Default::default()
        };
        let fs = f.to_rustybuzz();
        assert_eq!(fs.len(), 2, "expect liga + clig off entries");
        // Both should have value 0.
        for feat in &fs {
            assert_eq!(feat.value, 0);
        }
    }

    #[test]
    fn shaping_features_kerning_off_adds_kern_off() {
        let f = ShapingFeatures {
            kerning: KerningMethod::Off,
            ..Default::default()
        };
        let fs = f.to_rustybuzz();
        assert_eq!(fs.len(), 1);
        assert_eq!(fs[0].value, 0);
    }

    #[test]
    fn shaping_features_metrics_kerning_does_nothing_extra() {
        let f = ShapingFeatures {
            kerning: KerningMethod::Metrics,
            ..Default::default()
        };
        assert!(f.to_rustybuzz().is_empty());
    }

    #[test]
    fn shape_run_with_features_handles_empty_text() {
        // Sanity smoke — the function should produce an empty result
        // when fed empty text, regardless of features.
        let bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture");
        let face = rustybuzz::Face::from_slice(&bytes, 0).expect("parse Inter");
        let r = shape_run_with_features(
            &face,
            "",
            12.0,
            ShapingFeatures {
                ligatures_on: false,
                kerning: KerningMethod::Off,
            },
        );
        assert!(r.glyphs.is_empty());
        assert_eq!(r.total_advance, 0);
    }
}
