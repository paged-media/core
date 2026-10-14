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

//! Ruby and emphasis marks (kenten), set as InDesign 20.0.1 sets them by
//! default. Asked of InDesign through `tools/indesign-export/ruby-kenten-
//! probe` (Kozuka Mincho, Open Sans and Courier New at 30 pt, read from
//! its PDF):
//!
//! * both are half the base's size (`RubyFontSize` / `KentenFontSize`
//!   -1, "auto"), ruby in the base's own face;
//! * both sit on the base's em box: the mark's em box stands on the
//!   base's (Kozuka: ruby and kenten baselines 28.2 pt above a 30 pt
//!   base, 0.88 × 30 + 0.12 × 15). A face's em box is its `BASE` table's
//!   ideographic baseline (`ideo`) and one em above it, or, without one,
//!   one em centred on half the cap height (Open Sans 0.857 / 0.143 em,
//!   Courier New 0.786 / 0.214 em, both measured);
//! * a kenten mark is centred on its character;
//! * ruby narrower than its base spreads over it 1:2:1 (`RubyJIS`): one
//!   share before the first ruby character, two between each two, one
//!   after the last. Ruby wider than its base spaces the base out the
//!   same way (`RubyParent121Aki`), and the ruby starts with it, so the
//!   rest of the line moves along (`LayoutOptions::glyph_aki`);
//! * per-character ruby (`PerCharacterRuby`, InDesign's default) puts
//!   each space-separated part over one base character in turn; a
//!   string with no space goes over the first character whole.
//!
//! InDesign draws the marks from its own font, KentenGeneric; the shapes
//! below are drawn to the same sizes, not copied from it.

use std::ops::Range;

use paged_compose::{
    emit_glyph_slice, DisplayCommand, DisplayList, Paint, PathData, PathSegment, Transform,
    TtfOutliner,
};
use paged_text::shape::ADVANCE_PRECISION;

/// Where a face's em box lies, in ems: `top` above the baseline, `bottom`
/// below it (`top + bottom == 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EmBox {
    pub top: f32,
    pub bottom: f32,
}

/// The face's em box (see the module notes).
pub(super) fn em_box(face: &ttf_parser::Face) -> EmBox {
    let upem = f32::from(face.units_per_em()).max(1.0);
    if let Some(ideo) = base_ideographic(face) {
        let bottom = -f32::from(ideo) / upem;
        return EmBox {
            top: 1.0 - bottom,
            bottom,
        };
    }
    match face.capital_height().filter(|&c| c > 0) {
        Some(cap) => {
            let half = f32::from(cap) / upem / 2.0;
            EmBox {
                top: 0.5 + half,
                bottom: 0.5 - half,
            }
        }
        // No cap height to centre on: a CJK font's conventional box.
        None => EmBox {
            top: 0.88,
            bottom: 0.12,
        },
    }
}

/// The horizontal `ideo` baseline of the face's `BASE` table, in font
/// units: the first script that states it (they agree in practice).
fn base_ideographic(face: &ttf_parser::Face) -> Option<i16> {
    let data = face
        .raw_face()
        .table(ttf_parser::Tag::from_bytes(b"BASE"))?;
    let u16_at = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    let axis = usize::from(u16_at(4)?);
    if axis == 0 {
        return None;
    }
    let tag_list = axis + usize::from(u16_at(axis)?);
    let script_list = axis + usize::from(u16_at(axis + 2)?);
    let tags = usize::from(u16_at(tag_list)?);
    let ideo = (0..tags).find(|&i| {
        data.get(tag_list + 2 + 4 * i..tag_list + 6 + 4 * i) == Some(b"ideo".as_slice())
    })?;
    let scripts = usize::from(u16_at(script_list)?);
    (0..scripts).find_map(|i| {
        let script = script_list + usize::from(u16_at(script_list + 2 + 6 * i + 4)?);
        let values = usize::from(u16_at(script)?);
        if values == 0 {
            return None;
        }
        let values = script + values;
        if ideo >= usize::from(u16_at(values + 2)?) {
            return None;
        }
        let coord = values + usize::from(u16_at(values + 4 + 2 * ideo)?);
        u16_at(coord + 2).map(|v| v as i16)
    })
}

/// One ruby annotation: the base characters it stands over and its text.
pub(super) struct RubyGroup {
    /// The base characters' bytes in the paragraph's shaped text.
    pub base: Range<u32>,
    /// The styled run the base belongs to (its face and size).
    pub run: usize,
    pub text: String,
    /// Space set on each side of every base character when the ruby is
    /// wider (1/64 pt); 0 when it fits over the base.
    pub share_64: i32,
}

/// The paragraph's ruby: one group per annotated base (a whole run for
/// group ruby, one character per part for per-character ruby), each
/// measured against its base. `runs` are the styled runs as shaped and
/// `resolved` their attributes, `head_run` of them (a list marker shaped
/// on its own) ahead of `resolved`.
pub(super) fn ruby_groups(
    runs: &[paged_text::StyledRun],
    resolved: &[paged_scene::ResolvedRunAttrs],
    head_run: usize,
) -> Vec<RubyGroup> {
    let mut out = Vec::new();
    let mut at = 0u32;
    for (k, run) in runs.iter().enumerate() {
        let start = at;
        at += run.text.len() as u32;
        let Some(attrs) = k.checked_sub(head_run).and_then(|i| resolved.get(i)) else {
            continue;
        };
        if !attrs.ruby_flag.unwrap_or(false) || run.text.is_empty() {
            continue;
        }
        let Some(text) = attrs
            .ruby_string
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let mut pieces: Vec<(Range<u32>, usize, String)> = Vec::new();
        if attrs.ruby_type.as_deref() == Some("GroupRuby") {
            pieces.push((start..at, run.text.chars().count(), text.trim().to_string()));
        } else {
            let parts: Vec<&str> = text
                .split([' ', '\u{3000}'])
                .filter(|p| !p.is_empty())
                .collect();
            let chars: Vec<(usize, char)> = run.text.char_indices().collect();
            for (i, part) in parts.iter().enumerate() {
                let Some(&(byte, ch)) = chars.get(i.min(chars.len() - 1)) else {
                    break;
                };
                let range = start + byte as u32..start + (byte + ch.len_utf8()) as u32;
                match pieces.last_mut() {
                    // More parts than characters: the rest join the last.
                    Some(last) if i >= chars.len() => last.2.push_str(part),
                    _ => pieces.push((range, 1, part.to_string())),
                }
            }
        }
        let scale = run.horizontal_scale_pct / 100.0;
        for (base, base_chars, text) in pieces {
            let base_text = &run.text[(base.start - start) as usize..(base.end - start) as usize];
            let base_w = paged_text::shape_run(run.face, base_text, run.point_size).total_advance
                as f32
                * scale;
            let ruby_w =
                paged_text::shape_run(run.face, &text, run.point_size * 0.5).total_advance as f32;
            let spare = ruby_w - base_w;
            let share_64 = if spare > 0.0 {
                (spare / (2 * base_chars) as f32).round() as i32
            } else {
                0
            };
            out.push(RubyGroup {
                base,
                run: k,
                text,
                share_64,
            });
        }
    }
    out
}

/// Draw the ruby of `groups` whose base lies on `line`. `face_of` gives a
/// styled run's outline face and the font it draws with; `paint_for` the
/// paint of a base character (ruby takes its base's colour).
pub(super) fn emit_ruby_for_line<'f>(
    line: &paged_text::layout::LaidOutLine,
    groups: &[RubyGroup],
    runs: &[paged_text::StyledRun],
    face_of: impl Fn(usize) -> Option<(&'f ttf_parser::Face<'f>, paged_compose::FontRef)>,
    paint_for: impl Fn(u32) -> Paint,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    for group in groups {
        let base: Vec<&paged_text::PositionedGlyph> = line
            .glyphs
            .iter()
            .filter(|g| group.base.contains(&g.cluster))
            .collect();
        let (Some(first), Some(last)) = (base.first(), base.last()) else {
            continue;
        };
        let Some((outline, font)) = face_of(group.run) else {
            continue;
        };
        let run = &runs[group.run];
        let base_pt = first.point_size.max(1.0);
        let ruby_pt = base_pt * 0.5;
        let shaped = paged_text::shape_run(run.face, &group.text, ruby_pt);
        if shaped.glyphs.is_empty() {
            continue;
        }
        // The base's slot, its spacing included.
        let left = first.x - group.share_64;
        let right = last.x - group.share_64 + last.x_advance;
        // 1:2:1 over a wider base; from the slot's start when the base
        // was spaced out to the ruby (or the ruby still overruns it).
        let clusters = 1 + shaped
            .glyphs
            .windows(2)
            .filter(|w| w[1].cluster != w[0].cluster)
            .count() as i32;
        let spare = right - left - shaped.total_advance;
        let share = if spare > 0 { spare / (2 * clusters) } else { 0 };
        let mut pen = left + share;
        let mut positioned = Vec::with_capacity(shaped.glyphs.len());
        for (i, g) in shaped.glyphs.iter().enumerate() {
            if i > 0 && g.cluster != shaped.glyphs[i - 1].cluster {
                pen += 2 * share;
            }
            positioned.push(paged_text::PositionedGlyph {
                glyph_id: g.glyph_id,
                cluster: first.cluster,
                x: pen + g.x_offset,
                y: g.y_offset,
                x_advance: g.x_advance,
                font_id: font.outline,
                point_size: ruby_pt,
                underline: false,
                strikethru: false,
                substituted: false,
                x_scale: 1.0,
                y_scale: 1.0,
                skew_deg: 0.0,
                ch: group
                    .text
                    .get(g.cluster as usize..)
                    .and_then(|s| s.chars().next()),
            });
            pen += g.x_advance;
        }
        // The ruby's em box stands on the base's.
        let em = em_box(outline);
        let baseline_pt = first.y as f32 / ADVANCE_PRECISION;
        let ruby_baseline = baseline_pt - em.top * base_pt - em.bottom * ruby_pt;
        let paint = paint_for(first.cluster);
        emit_glyph_slice(
            &positioned,
            font,
            ruby_pt,
            |_| paint,
            (frame_origin_pt.0, frame_origin_pt.1 + ruby_baseline),
            &TtfOutliner::new(outline),
            list,
        );
    }
}

/// An emphasis mark's shape (`KentenKind`), in the order of InDesign's
/// `KentenCharacter` enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kenten {
    SesameDot,
    WhiteSesameDot,
    Fisheye,
    BlackCircle,
    SmallBlackCircle,
    Bullseye,
    BlackTriangle,
    WhiteTriangle,
    WhiteCircle,
    SmallWhiteCircle,
}

impl Kenten {
    /// IDML's `KentenKind` spelling (`KentenSesameDot`, …); `None` for
    /// `None` and for kinds drawn from a character (`Custom`).
    pub(super) fn from_idml(kind: &str) -> Option<Self> {
        Some(match kind {
            "KentenSesameDot" => Self::SesameDot,
            "KentenWhiteSesameDot" => Self::WhiteSesameDot,
            "KentenFisheye" => Self::Fisheye,
            "KentenBlackCircle" => Self::BlackCircle,
            "KentenSmallBlackCircle" => Self::SmallBlackCircle,
            "KentenBullseye" => Self::Bullseye,
            "KentenBlackTriangle" => Self::BlackTriangle,
            "KentenWhiteTriangle" => Self::WhiteTriangle,
            "KentenWhiteCircle" => Self::WhiteCircle,
            "KentenSmallWhiteCircle" => Self::SmallWhiteCircle,
            _ => return None,
        })
    }

    /// The mark in its own em: origin on the em box's centre, one unit an
    /// em, y down. Sizes follow KentenGeneric's marks (in thousandths of
    /// an em): circles 700 wide, the small ones 440 and 490, rings 50 to
    /// 64 thick, triangles 770 wide, the sesame dot about 660 × 675.
    fn path(self) -> PathData {
        let mut p = PathData {
            segments: Vec::new(),
        };
        let ring = |p: &mut PathData, outer: f32, inner: f32| {
            circle(p, outer, false);
            circle(p, inner, true);
        };
        match self {
            Self::BlackCircle => circle(&mut p, 0.350, false),
            Self::SmallBlackCircle => circle(&mut p, 0.220, false),
            Self::WhiteCircle => ring(&mut p, 0.350, 0.286),
            Self::SmallWhiteCircle => ring(&mut p, 0.245, 0.181),
            Self::Fisheye => {
                ring(&mut p, 0.350, 0.300);
                circle(&mut p, 0.206, false);
            }
            Self::Bullseye => {
                ring(&mut p, 0.350, 0.300);
                ring(&mut p, 0.206, 0.159);
            }
            Self::BlackTriangle => triangle(&mut p, 0.385, 0.325, -0.342, false),
            Self::WhiteTriangle => {
                triangle(&mut p, 0.385, 0.325, -0.342, false);
                triangle(&mut p, 0.281, 0.265, -0.222, true);
            }
            Self::SesameDot => sesame(&mut p, 1.0, false),
            Self::WhiteSesameDot => {
                sesame(&mut p, 0.97, false);
                sesame(&mut p, 0.78, true);
            }
        }
        p
    }

    fn key(self) -> u64 {
        0xD001_0003_0000_0000 | self as u64
    }
}

/// Cubic pieces of the arc round `c`, radius `r`, from angle `a0` to
/// `a1` (radians; y down, so a positive turn runs clockwise on the page),
/// each at most a quarter turn. The pen stands at the arc's start.
fn arc(p: &mut PathData, c: (f32, f32), r: f32, a0: f32, a1: f32) {
    let n = ((a1 - a0).abs() / std::f32::consts::FRAC_PI_2)
        .ceil()
        .max(1.0) as usize;
    let step = (a1 - a0) / n as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    for i in 0..n {
        let (s0, c0) = (a0 + step * i as f32).sin_cos();
        let (s1, c1) = (a0 + step * (i + 1) as f32).sin_cos();
        p.segments.push(PathSegment::CubicTo {
            cx1: c.0 + r * (c0 - k * s0),
            cy1: c.1 + r * (s0 + k * c0),
            cx2: c.0 + r * (c1 + k * s1),
            cy2: c.1 + r * (s1 - k * c1),
            x: c.0 + r * c1,
            y: c.1 + r * s1,
        });
    }
}

/// A circle of radius `r` round the origin; `reverse` winds it the other
/// way, to cut a hole under the non-zero rule.
fn circle(p: &mut PathData, r: f32, reverse: bool) {
    let turn = if reverse { -1.0 } else { 1.0 } * std::f32::consts::TAU;
    p.segments.push(PathSegment::MoveTo { x: r, y: 0.0 });
    arc(p, (0.0, 0.0), r, 0.0, turn);
    p.segments.push(PathSegment::Close);
}

/// An upright triangle: its base `half_w` either side of the axis at
/// `base_y`, its apex at `apex_y`.
fn triangle(p: &mut PathData, half_w: f32, base_y: f32, apex_y: f32, reverse: bool) {
    let pts = [(-half_w, base_y), (half_w, base_y), (0.0, apex_y)];
    let order = if reverse { [0, 2, 1] } else { [0, 1, 2] };
    p.segments.push(PathSegment::MoveTo {
        x: pts[order[0]].0,
        y: pts[order[0]].1,
    });
    for &i in &order[1..] {
        p.segments.push(PathSegment::LineTo {
            x: pts[i].0,
            y: pts[i].1,
        });
    }
    p.segments.push(PathSegment::Close);
}

/// The sesame dot: a comma from a pointed tip at the upper left to a
/// round end at the lower right, its outer edge bulging and its inner one
/// nearly straight, scaled by `scale` round a point towards its round end
/// (the white sesame's hole is a smaller one, wound the other way).
fn sesame(p: &mut PathData, scale: f32, reverse: bool) {
    let at = |(x, y): (f32, f32)| (0.10 + (x - 0.10) * scale, 0.13 + (y - 0.13) * scale);
    let cubic = |p: &mut PathData, c1: (f32, f32), c2: (f32, f32), to: (f32, f32)| {
        let (c1, c2, to) = (at(c1), at(c2), at(to));
        p.segments.push(PathSegment::CubicTo {
            cx1: c1.0,
            cy1: c1.1,
            cx2: c2.0,
            cy2: c2.1,
            x: to.0,
            y: to.1,
        });
    };
    // The round end and where the edges meet it; the edges' controls.
    let (centre, r) = ((0.19, 0.22), 0.135);
    let (a_out, a_in) = ((-40f32).to_radians(), 144f32.to_radians());
    let on_end = |a: f32| (centre.0 + r * a.cos(), centre.1 + r * a.sin());
    let tip = (-0.30, -0.33);
    let outer = [(-0.13, -0.31), (0.152, -0.035)];
    let inner = [(-0.006, 0.178), (-0.13, -0.22)];
    let start = at(tip);
    p.segments.push(PathSegment::MoveTo {
        x: start.0,
        y: start.1,
    });
    if reverse {
        cubic(p, inner[1], inner[0], on_end(a_in));
        arc(p, at(centre), r * scale, a_in, a_out);
        cubic(p, outer[1], outer[0], tip);
    } else {
        cubic(p, outer[0], outer[1], on_end(a_out));
        arc(p, at(centre), r * scale, a_out, a_in);
        cubic(p, inner[0], inner[1], tip);
    }
    p.segments.push(PathSegment::Close);
}

/// Draw the emphasis marks over `line`'s characters, one a character.
/// `mark_of` gives a glyph's mark and its size (pt), `em_top_of` the
/// em-box top (ems above the baseline) of the font a glyph is drawn in,
/// `paint_for` the colour of a character (a mark takes its text's).
pub(super) fn emit_kenten_for_line(
    line: &paged_text::layout::LaidOutLine,
    mark_of: impl Fn(&paged_text::PositionedGlyph) -> Option<(Kenten, f32)>,
    em_top_of: impl Fn(u32) -> f32,
    paint_for: impl Fn(u32) -> Paint,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    let mut last_cluster = None;
    for g in &line.glyphs {
        if last_cluster.replace(g.cluster) == Some(g.cluster) {
            continue;
        }
        let Some((kind, size)) = mark_of(g) else {
            continue;
        };
        // Centred on the character; its em box on the base's.
        let cx = frame_origin_pt.0 + (g.x as f32 + g.x_advance as f32 * 0.5) / ADVANCE_PRECISION;
        let cy = frame_origin_pt.1 + g.y as f32 / ADVANCE_PRECISION
            - em_top_of(g.font_id) * g.point_size
            - size * 0.5;
        let (path_id, _) = list.paths.intern(kind.key(), kind.path());
        list.push(DisplayCommand::FillPath {
            path_id,
            paint: paint_for(g.cluster),
            transform: Transform([size, 0.0, 0.0, size, cx, cy]),
        });
    }
}
