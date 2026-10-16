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
//!   base, 0.88 × 30 + 0.12 × 15). A face's em box is `font_table`'s
//!   `em_box`: a CJK face's from its `BASE` table or its 永, any other one
//!   em centred on half the cap height (Open Sans 0.857 / 0.143 em,
//!   Courier New 0.786 / 0.214 em, both measured);
//! * a kenten mark is centred on its character;
//! * ruby narrower than its base spreads over it 1:2:1 (`RubyJIS`): one
//!   share before the first ruby character, two between each two, one
//!   after the last;
//! * ruby wider than its base first overhangs the characters beside it,
//!   then narrows if `RubyAutoScaling` allows, and only what is left
//!   spaces the base out (`ruby-parent-spacing-probe`, see
//!   [`ParentSpacing`] and [`Overhang`]); the rest of the line moves along
//!   (`LayoutOptions::ruby_aki`);
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

use super::font_table::em_box;

/// One ruby annotation: the base characters it stands over and its text.
pub(super) struct RubyGroup {
    /// The base characters' bytes in the paragraph's shaped text.
    pub base: Range<u32>,
    /// The styled run the base belongs to (its face and size).
    pub run: usize,
    pub text: String,
    /// The base spaced out to a wider ruby (no variants when it fits).
    pub aki: paged_text::layout::RubyAki,
    /// Where a wider ruby starts and how wide it prints, per
    /// [`RubyAki::variants`](paged_text::layout::RubyAki) slot.
    pub fit: Option<[WideFit; 4]>,
    /// The ruby's size (pt) and its horizontal and vertical scale.
    pub size: f32,
    pub x_scale: f32,
    pub y_scale: f32,
    /// `RubyXOffset` / `RubyYOffset` (pt): right and up.
    pub offset: (f32, f32),
    /// `RubyPosition="BelowLeft"`: under the base.
    pub below: bool,
    pub alignment: RubyAlignment,
}

/// How ruby narrower than its base spreads over it (`RubyAlignment`).
/// InDesign 20.0.1, かんじ (45 pt) over 60 and 120 pt of base
/// (`tools/indesign-export/ruby-kenten-probe`):
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RubyAlignment {
    /// `RubyJIS`, the default: one share of the spare room before the
    /// first character, two between each two, one after the last.
    Jis,
    /// `RubyLeft` / `RubyCenter` / `RubyRight`: set solid, flush left,
    /// centred or flush right.
    Left,
    Center,
    Right,
    /// `RubyFullJustify`: the room between the characters only, the ends
    /// flush (a single character centred).
    FullJustify,
    /// `RubyEqualAki`: equal shares at both ends and between.
    EqualAki,
    /// `Ruby1Aki`: one ruby em at each end, or half the room when there
    /// is less, the rest between the characters.
    OneAki,
}

impl RubyAlignment {
    fn from_idml(s: Option<&str>) -> Self {
        match s {
            Some("RubyLeft") => Self::Left,
            Some("RubyCenter") => Self::Center,
            Some("RubyRight") => Self::Right,
            Some("RubyFullJustify") => Self::FullJustify,
            Some("RubyEqualAki") => Self::EqualAki,
            Some("Ruby1Aki") => Self::OneAki,
            _ => Self::Jis,
        }
    }

    /// Where the ruby's first character starts after `left`, and the room
    /// between each two, for `spare` room over the base (1/64 pt), `n`
    /// characters and a ruby em of `em` (1/64 pt).
    fn spread(self, spare: i32, n: i32, em: i32) -> (i32, i32) {
        if spare <= 0 {
            return (0, 0);
        }
        let between = |room: i32| if n > 1 { room / (n - 1) } else { 0 };
        match self {
            Self::Jis => (spare / (2 * n), spare / n),
            Self::Left => (0, 0),
            Self::Center => (spare / 2, 0),
            Self::Right => (spare, 0),
            Self::FullJustify if n > 1 => (0, between(spare)),
            Self::FullJustify => (spare / 2, 0),
            Self::EqualAki => (spare / (n + 1), spare / (n + 1)),
            Self::OneAki if n > 1 => {
                let end = em.min(spare / 2);
                (end, between(spare - 2 * end))
            }
            Self::OneAki => (spare / 2, 0),
        }
    }
}

/// How a ruby wider than its base starts and prints: `lead` is where its
/// first character starts from the base's spaced-out slot (1/64 pt,
/// negative where it overhangs the character before), `x_scale` its
/// horizontal scale once `RubyAutoScaling` has narrowed it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WideFit {
    pub lead: i32,
    pub x_scale: f32,
}

/// `RubyParentSpacing`: how the room a wider ruby still needs, once it
/// has overhung its neighbours, spaces the base out. InDesign 20.0.1
/// (`ruby-parent-spacing-probe`; E the room, n base characters):
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParentSpacing {
    /// `RubyParent121Aki`, the default: 2n shares, one before the first
    /// character, two between each two, one after the last.
    Jis,
    /// `RubyParentNoAdjustment`: the base is not spaced; the ruby is
    /// centred over it and overhangs both sides, whatever stands there.
    None,
    /// `RubyParentBothSides`: half before the base, half after.
    BothSides,
    /// `RubyParentEqualAki`: E/(n+1) before, between and after.
    EqualAki,
    /// `RubyParentFullJustify`: E/(n−1) between, nothing at the ends;
    /// one character takes it on both sides.
    FullJustify,
}

impl ParentSpacing {
    fn from_idml(s: Option<&str>) -> Self {
        match s {
            Some("RubyParentNoAdjustment") => Self::None,
            Some("RubyParentBothSides") => Self::BothSides,
            Some("RubyParentEqualAki") => Self::EqualAki,
            Some("RubyParentFullJustify") => Self::FullJustify,
            _ => Self::Jis,
        }
    }

    /// The shares before the first character, between each two, and
    /// after the last. A base starting a line drops its share before, one
    /// ending a line its share after, and the room goes to the others.
    fn shares(self, n: usize, drop_before: bool, drop_after: bool) -> (u32, u32, u32) {
        let (before, between, after) = match self {
            Self::Jis => (1, 2, 1),
            Self::None => (0, 0, 0),
            Self::BothSides => (1, 0, 1),
            Self::EqualAki => (1, 1, 1),
            Self::FullJustify if n > 1 => (0, 1, 0),
            Self::FullJustify => (1, 0, 1),
        };
        let shares = (
            if drop_before { 0 } else { before },
            between,
            if drop_after { 0 } else { after },
        );
        // Nowhere left to put it (one character alone on its line): keep
        // both ends.
        if shares.0 + shares.2 + shares.1 * (n as u32).saturating_sub(1) == 0 {
            (before.max(1), between, after.max(1))
        } else {
            shares
        }
    }
}

/// `RubyParentOverhangAmount`: how far a wider ruby may reach over each
/// neighbour of its base. InDesign overhangs a neighbour that is not a
/// kanji (kana, brackets and Latin, measured), never past a line's start
/// or end, by at most half the ruby's excess on each side, and never by
/// more than the neighbour's natural width, its em box before `palt` (a
/// 14.34 pt `c` takes 14.34 pt of a 15 pt overhang, a kana a whole em
/// however `palt` narrows it: `ruby-parent-spacing-probe` `latin`,
/// `wide-onechar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Overhang {
    /// `RubyOverhangOneRuby`, the default: one ruby character.
    OneRuby,
    HalfRuby,
    /// `RubyOverhangOneChar`: one base character.
    OneChar,
    HalfChar,
    NoLimit,
    /// `None`: no overhang.
    None,
}

impl Overhang {
    fn from_idml(s: Option<&str>) -> Self {
        match s {
            Some("RubyOverhangHalfRuby") => Self::HalfRuby,
            Some("RubyOverhangOneChar") => Self::OneChar,
            Some("RubyOverhangHalfChar") => Self::HalfChar,
            Some("RubyOverhangNoLimit") => Self::NoLimit,
            Some("None") => Self::None,
            _ => Self::OneRuby,
        }
    }

    /// The most it may reach over one neighbour, 1/64 pt.
    fn limit(self, ruby_em: i32, base_em: i32) -> i32 {
        match self {
            Self::OneRuby => ruby_em,
            Self::HalfRuby => ruby_em / 2,
            Self::OneChar => base_em,
            Self::HalfChar => base_em / 2,
            Self::NoLimit => i32::MAX,
            Self::None => 0,
        }
    }
}

/// Whether a wider ruby may overhang `c`, a neighbour of its base: any
/// character but a kanji (and nothing past a break).
fn overhangs(c: Option<char>) -> bool {
    match c {
        None | Some('\n' | '\r' | '\u{2028}' | '\u{2029}') => false,
        Some(c) => !matches!(c as u32,
            0x3005..=0x3007 | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF),
    }
}

/// The settings a wider ruby is fitted by.
struct WideSettings {
    spacing: ParentSpacing,
    overhang: Overhang,
    /// `RubyAutoScaling` on: the least horizontal scale it narrows to.
    min_scale: Option<f32>,
}

/// Fit a ruby `ruby_w` wide (1/64 pt, at `x_scale`) over a base of
/// `base_chars` characters `base_w` wide, for each line position: the
/// space before and after each base character, and where the ruby starts
/// and how wide it prints. `left` / `right`: the width of the character
/// beside the base where it may be overhung.
#[allow(clippy::too_many_arguments)]
fn fit_wide(
    settings: &WideSettings,
    base_w: i32,
    ruby_w: i32,
    base_chars: usize,
    x_scale: f32,
    ruby_em: i32,
    base_em: i32,
    (left_w, right_w): (Option<i32>, Option<i32>),
) -> ([Vec<(i32, i32)>; 4], [WideFit; 4]) {
    let excess = ruby_w - base_w;
    let n = base_chars.max(1);
    let mut akis: [Vec<(i32, i32)>; 4] = Default::default();
    let mut fits = [WideFit { lead: 0, x_scale }; 4];
    for v in 0..4 {
        let (at_start, at_end) = (v & 1 != 0, v & 2 != 0);
        if settings.spacing == ParentSpacing::None {
            akis[v] = vec![(0, 0); n];
            fits[v].lead = -excess / 2;
            continue;
        }
        let limit = settings.overhang.limit(ruby_em, base_em).min(excess / 2);
        let left = left_w.filter(|_| !at_start).map_or(0, |w| limit.min(w));
        let right = right_w.filter(|_| !at_end).map_or(0, |w| limit.min(w));
        let mut room = excess - left - right;
        let mut scale = x_scale;
        if let Some(min) = settings.min_scale {
            if room > 0 {
                let target = base_w + left + right;
                let factor = (target as f32 / ruby_w as f32).max(min);
                scale = x_scale * factor;
                room = ((ruby_w as f32 * factor).round() as i32 - target).max(0);
            }
        }
        let (sb, sg, sa) = settings.spacing.shares(n, at_start, at_end);
        let total = sb + sa + sg * (n as u32 - 1);
        let unit = room as f32 / total.max(1) as f32;
        let mut slots: Vec<i32> = Vec::with_capacity(n + 1);
        slots.push((unit * sb as f32).round() as i32);
        for _ in 1..n {
            slots.push((unit * sg as f32).round() as i32);
        }
        slots.push((unit * sa as f32).round() as i32);
        // Rounding: the last slot takes what the others left.
        let placed: i32 = slots[..n].iter().sum();
        slots[n] = room - placed;
        let mut aki = Vec::with_capacity(n);
        for i in 0..n {
            let before = if i == 0 {
                slots[0]
            } else {
                slots[i] - slots[i] / 2
            };
            let after = if i + 1 == n {
                slots[n]
            } else {
                slots[i + 1] / 2
            };
            aki.push((before, after));
        }
        akis[v] = aki;
        fits[v] = WideFit {
            lead: -left,
            x_scale: scale,
        };
    }
    (akis, fits)
}

/// Which line position a ruby base takes on `line` (see
/// [`paged_text::layout::ruby_edge`]): judged from the line's glyphs, the
/// same characters the layout judged by.
fn line_edge(line: &paged_text::layout::LaidOutLine, chars: &[Range<u32>]) -> usize {
    let first = line.glyphs.first().map(|g| g.cluster);
    let last = line
        .glyphs
        .iter()
        .rev()
        .find(|g| {
            !g.ch
                .is_some_and(|c| c.is_ascii_whitespace() || c == '\u{2028}' || c == '\u{2029}')
        })
        .map(|g| g.cluster);
    let starts = chars.first().is_some_and(|r| Some(r.start) == first);
    let ends = chars
        .last()
        .is_some_and(|r| last.is_some_and(|l| r.contains(&l)));
    usize::from(starts) + 2 * usize::from(ends)
}

/// The paragraph's ruby: one group per annotated base (a whole run for
/// group ruby, one character per part for per-character ruby), each
/// measured against its base. `runs` are the styled runs as shaped and
/// `resolved` their attributes, `head_run` of them (a list marker shaped
/// on its own) ahead of `resolved`; `ruby_face` gives a styled run's
/// `RubyFont` where it names one at hand (the base's face otherwise).
pub(super) fn ruby_groups<'f>(
    runs: &[paged_text::StyledRun],
    resolved: &[paged_scene::ResolvedRunAttrs],
    head_run: usize,
    ruby_face: impl Fn(usize) -> Option<&'f paged_text::Face<'f>>,
    vertical: bool,
) -> Vec<RubyGroup> {
    let mut out = Vec::new();
    let mut at = 0u32;
    let paragraph: String = runs.iter().map(|r| r.text).collect();
    let starts: Vec<usize> = runs
        .iter()
        .scan(0, |at, r| {
            let start = *at;
            *at += r.text.len();
            Some(start)
        })
        .collect();
    // The natural width of the character at `byte` where a wider ruby may
    // overhang it, in its own run's face and size.
    let overhang_room = |byte: usize| -> Option<i32> {
        let c = paragraph[byte..]
            .chars()
            .next()
            .filter(|&c| overhangs(Some(c)))?;
        let r = &runs[starts.iter().rposition(|&s| s <= byte)?];
        let mut buf = [0u8; 4];
        let text = c.encode_utf8(&mut buf);
        let natural = paged_text::ShapingFeatures {
            proportional_widths: false,
            ..r.shaping_features
        };
        let shaped = if vertical {
            paged_text::shape::shape_run_vertical_at(r.face, text, r.point_size, natural, &mut 0.0)
        } else {
            paged_text::shape_run_with_features(r.face, text, r.point_size, natural)
        };
        Some((shaped.total_advance as f32 * r.horizontal_scale_pct / 100.0).round() as i32)
    };
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
        let marks = &attrs.marks;
        let pct = |v: Option<f32>| v.filter(|p| *p > 0.0).map_or(1.0, |p| p / 100.0);
        let size = marks
            .ruby_font_size
            .filter(|s| *s > 0.0)
            .unwrap_or(run.point_size * 0.5);
        let (x_scale, y_scale) = (pct(marks.ruby_x_scale), pct(marks.ruby_y_scale));
        let wide = WideSettings {
            spacing: ParentSpacing::from_idml(marks.ruby_parent_spacing.as_deref()),
            overhang: Overhang::from_idml(marks.ruby_overhang_amount.as_deref()),
            min_scale: marks.ruby_auto_scaling.unwrap_or(false).then(|| {
                marks
                    .ruby_parent_scaling_percent
                    .filter(|p| *p > 0.0)
                    .unwrap_or(66.0)
                    / 100.0
            }),
        };
        for (base, _, text) in pieces {
            let base_text = &run.text[(base.start - start) as usize..(base.end - start) as usize];
            let base_shaped = if vertical {
                paged_text::shape::shape_run_vertical_at(
                    run.face,
                    base_text,
                    run.point_size,
                    run.shaping_features,
                    &mut 0.0,
                )
            } else {
                paged_text::shape_run_with_features(
                    run.face,
                    base_text,
                    run.point_size,
                    run.shaping_features,
                )
            };
            let base_w = (base_shaped.total_advance as f32 * scale).round() as i32;
            let face = ruby_face(k).unwrap_or(run.face);
            let ruby_w =
                (paged_text::shape_run_with_features(face, &text, size, ruby_features(vertical))
                    .total_advance as f32
                    * x_scale)
                    .round() as i32;
            let chars: Vec<Range<u32>> = base_text
                .char_indices()
                .map(|(i, c)| base.start + i as u32..base.start + (i + c.len_utf8()) as u32)
                .collect();
            let (aki, fit) = if ruby_w > base_w {
                let neighbours = (
                    paragraph[..base.start as usize]
                        .char_indices()
                        .next_back()
                        .and_then(|(i, _)| overhang_room(i)),
                    overhang_room(base.end as usize),
                );
                let (variants, fit) = fit_wide(
                    &wide,
                    base_w,
                    ruby_w,
                    chars.len(),
                    x_scale,
                    (size * x_scale * ADVANCE_PRECISION).round() as i32,
                    (run.point_size * scale * ADVANCE_PRECISION).round() as i32,
                    neighbours,
                );
                (paged_text::layout::RubyAki { chars, variants }, Some(fit))
            } else {
                (paged_text::layout::RubyAki::default(), None)
            };
            out.push(RubyGroup {
                base,
                run: k,
                text,
                aki,
                fit,
                size,
                x_scale,
                y_scale,
                offset: (
                    marks.ruby_x_offset.unwrap_or(0.0),
                    marks.ruby_y_offset.unwrap_or(0.0),
                ),
                below: marks.ruby_position.as_deref() == Some("BelowLeft"),
                alignment: RubyAlignment::from_idml(marks.ruby_alignment.as_deref()),
            });
        }
    }
    out
}

/// How ruby is shaped: full width in its ruby forms, top to bottom (so
/// upright, in its vertical forms) in vertical text.
fn ruby_features(vertical: bool) -> paged_text::ShapingFeatures {
    paged_text::ShapingFeatures {
        vertical,
        ..paged_text::ShapingFeatures::ruby()
    }
}

/// The face a ruby or a custom kenten mark prints in: its shaping face,
/// its outline face and the font it draws with.
pub(super) type MarkFace<'f> = (
    &'f paged_text::Face<'f>,
    &'f ttf_parser::Face<'f>,
    paged_compose::FontRef,
);

/// Draw the ruby of `groups` whose base lies on `line`. `em_of` gives the
/// em box of the font a base glyph is drawn in, `face_of` the face a
/// styled run's ruby prints in, `paint_for` the paint of a base character
/// (ruby takes its base's colour).
pub(super) fn emit_ruby_for_line<'f>(
    line: &paged_text::layout::LaidOutLine,
    groups: &[RubyGroup],
    em_of: impl Fn(u32) -> super::font_table::EmBox,
    face_of: impl Fn(usize) -> Option<MarkFace<'f>>,
    paint_for: impl Fn(u32) -> Paint,
    frame_origin_pt: (f32, f32),
    vertical: bool,
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
        let Some((shaping, outline, font)) = face_of(group.run) else {
            continue;
        };
        let base_pt = first.point_size.max(1.0);
        let ruby_pt = group.size;
        let shaped = paged_text::shape_run_with_features(
            shaping,
            &group.text,
            ruby_pt,
            ruby_features(vertical),
        );
        if shaped.glyphs.is_empty() {
            continue;
        }
        // A ruby wider than its base was fitted for the place its base
        // takes on this line; one that fits spreads by its alignment.
        let edge = line_edge(line, &group.aki.chars);
        let wide = group.fit.map(|f| f[edge]);
        let sx = wide.map_or(group.x_scale, |w| w.x_scale);
        let scaled = |v: i32| (v as f32 * sx).round() as i32;
        // The base's slot, its spacing included.
        let space = |g: &paged_text::PositionedGlyph| {
            group
                .aki
                .chars
                .iter()
                .position(|r| r.contains(&g.cluster))
                .and_then(|i| group.aki.variants[edge].get(i))
                .map_or(0, |&(before, _)| before)
        };
        let left = first.x - space(first);
        let right = last.x - space(last) + last.x_advance;
        let clusters = 1 + shaped
            .glyphs
            .windows(2)
            .filter(|w| w[1].cluster != w[0].cluster)
            .count() as i32;
        let width: i32 = shaped.glyphs.iter().map(|g| scaled(g.x_advance)).sum();
        let em_64 = (ruby_pt * sx * ADVANCE_PRECISION).round() as i32;
        let (lead, between) = match wide {
            Some(w) => (w.lead, 0),
            None => group
                .alignment
                .spread(right - left - width, clusters, em_64),
        };
        let mut pen = left + lead + (group.offset.0 * ADVANCE_PRECISION).round() as i32;
        let mut positioned = Vec::with_capacity(shaped.glyphs.len());
        for (i, g) in shaped.glyphs.iter().enumerate() {
            if i > 0 && g.cluster != shaped.glyphs[i - 1].cluster {
                pen += between;
            }
            positioned.push(paged_text::PositionedGlyph {
                glyph_id: g.glyph_id,
                cluster: first.cluster,
                x: pen + scaled(g.x_offset),
                y: g.y_offset,
                x_advance: scaled(g.x_advance),
                font_id: font.outline,
                point_size: ruby_pt,
                underline: false,
                strikethru: false,
                substituted: false,
                x_scale: sx,
                y_scale: group.y_scale,
                skew_deg: 0.0,
                ch: group
                    .text
                    .get(g.cluster as usize..)
                    .and_then(|s| s.chars().next()),
                upright: g.upright,
            });
            pen += scaled(g.x_advance);
        }
        // The ruby's em box stands on the base's, or hangs from it below;
        // the offset moves it up either way, and a vertical scale grows
        // it from its baseline.
        let (base, em) = (em_of(first.font_id), em_box(outline));
        let baseline_pt = first.y as f32 / ADVANCE_PRECISION;
        let ruby_baseline = if group.below {
            baseline_pt + base.bottom * base_pt + em.top * ruby_pt
        } else {
            baseline_pt - base.top * base_pt - em.bottom * ruby_pt
        } - group.offset.1;
        // Upright ruby in vertical text is drawn from its column's centre
        // line, its em box's middle.
        let draw_at = if vertical {
            ruby_baseline - (em.top - 0.5) * ruby_pt
        } else {
            ruby_baseline
        };
        let paint = paint_for(first.cluster);
        emit_glyph_slice(
            &positioned,
            font,
            ruby_pt,
            |_| paint,
            (frame_origin_pt.0, frame_origin_pt.1 + draw_at),
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

/// One character's emphasis mark and where it goes (`KentenKind` and the
/// other `Kenten*` settings). InDesign 20.0.1
/// (`tools/indesign-export/ruby-kenten-probe`): `KentenFontSize` and the
/// scales keep a mark centred on its character (`AlignKentenLeft` puts
/// its em box on the character's left edge); a vertical scale grows it
/// up from its baseline; `KentenPlacement` moves it up, above or below
/// the line; `BelowLeft` hangs its em box from the base's.
pub(super) struct KentenMark {
    pub shape: MarkShape,
    /// The mark's size (pt) and its horizontal and vertical scale.
    pub size: f32,
    pub x_scale: f32,
    pub y_scale: f32,
    /// `KentenPlacement` (pt): up.
    pub placement: f32,
    /// `KentenPosition="BelowLeft"`: under the base.
    pub below: bool,
    /// `KentenAlignment="AlignKentenLeft"`.
    pub left: bool,
    /// The styled run the character belongs to: its face prints a
    /// `Custom` mark.
    pub run: usize,
}

/// What a mark prints: one of the drawn kinds, or a `Custom` kind's
/// character in its `KentenFont` (the base's face where it names none),
/// centred on its advance, its em box on the base's (InDesign: Minion
/// Pro's "A" over Kozuka Mincho, 29.025 pt up at 30 pt).
pub(super) enum MarkShape {
    Drawn(Kenten),
    Character(char),
}

/// Draw the emphasis marks over `line`'s characters, one a character.
/// `mark_of` gives a glyph's mark, `em_of` the em box of the font a glyph
/// is drawn in, `face_of` the face a styled run's custom mark prints in
/// (its `KentenFont`, else the base's) and `paint_for` the colour of a
/// character (a mark takes its text's).
/// Whether a character carries its emphasis mark: InDesign 20.0.1 marks
/// kanji and kana but not punctuation — of 24 marked characters it left
/// 。、、「」 bare, set across or down (vertical-text-probe
/// `kenten-punct`).
fn takes_kenten(c: char) -> bool {
    !(c.is_whitespace()
        || c.is_ascii_punctuation()
        || matches!(c as u32,
            0x2018..=0x201F
            | 0x2025..=0x2026
            | 0x3001..=0x3003
            | 0x3008..=0x3011
            | 0x3014..=0x301F
            | 0x30FB
            | 0xFF01..=0xFF0F
            | 0xFF1A..=0xFF20
            | 0xFF3B..=0xFF40
            | 0xFF5B..=0xFF65))
}

pub(super) fn emit_kenten_for_line<'f>(
    line: &paged_text::layout::LaidOutLine,
    mark_of: impl Fn(&paged_text::PositionedGlyph) -> Option<KentenMark>,
    em_of: impl Fn(u32) -> super::font_table::EmBox,
    face_of: impl Fn(usize) -> Option<MarkFace<'f>>,
    paint_for: impl Fn(u32) -> Paint,
    frame_origin_pt: (f32, f32),
    vertical: bool,
    list: &mut DisplayList,
) {
    let mut last_cluster = None;
    for g in &line.glyphs {
        if last_cluster.replace(g.cluster) == Some(g.cluster) {
            continue;
        }
        if g.ch.is_some_and(|c| !takes_kenten(c)) {
            continue;
        }
        let Some(mark) = mark_of(g) else {
            continue;
        };
        let base = em_of(g.font_id);
        let baseline = g.y as f32 / ADVANCE_PRECISION;
        // The mark's em box stands on the base's, or hangs from it below.
        let (on_top, hang_below) = (
            baseline - base.top * g.point_size - mark.placement,
            baseline + base.bottom * g.point_size - mark.placement,
        );
        let char_left = g.x as f32 / ADVANCE_PRECISION;
        let char_centre = char_left + g.x_advance as f32 * 0.5 / ADVANCE_PRECISION;
        let width = mark.size * mark.x_scale;
        let paint = paint_for(g.cluster);
        match mark.shape {
            MarkShape::Drawn(kind) => {
                // A drawn mark has KentenGeneric's em box: 0.12 em below
                // its baseline, its centre 0.38 em above.
                let mark_baseline = if mark.below {
                    hang_below + 0.88 * mark.size
                } else {
                    on_top - 0.12 * mark.size
                };
                let cx = if mark.left {
                    char_left + width * 0.5
                } else {
                    char_centre
                };
                let cy = mark_baseline - 0.38 * mark.size * mark.y_scale;
                let (path_id, _) = list.paths.intern(kind.key(), kind.path());
                let height = mark.size * mark.y_scale;
                let (ox, oy) = (frame_origin_pt.0 + cx, frame_origin_pt.1 + cy);
                // In vertical text InDesign draws the mark upright: turned
                // back a quarter about its centre, so the turn of the line
                // into a column leaves it as drawn (vertical-text-probe).
                let transform = if vertical {
                    Transform([0.0, -width, height, 0.0, ox, oy])
                } else {
                    Transform([width, 0.0, 0.0, height, ox, oy])
                };
                list.push(DisplayCommand::FillPath {
                    path_id,
                    paint,
                    transform,
                });
            }
            MarkShape::Character(ch) => {
                let Some((shaping, outline, font)) = face_of(mark.run) else {
                    continue;
                };
                let mut buf = [0u8; 4];
                let shaped = paged_text::shape_run_with_features(
                    shaping,
                    ch.encode_utf8(&mut buf),
                    mark.size,
                    paged_text::ShapingFeatures {
                        vertical,
                        ..paged_text::ShapingFeatures::default()
                    },
                );
                let advance = shaped.total_advance as f32 / ADVANCE_PRECISION * mark.x_scale;
                let em = em_box(outline);
                let mark_baseline = if mark.below {
                    hang_below + em.top * mark.size
                } else {
                    on_top - em.bottom * mark.size
                };
                let x0 = if mark.left {
                    char_left
                } else {
                    char_centre - advance * 0.5
                };
                let mut pen = 0i32;
                let glyphs: Vec<paged_text::PositionedGlyph> = shaped
                    .glyphs
                    .iter()
                    .map(|sg| {
                        let x = pen + (sg.x_offset as f32 * mark.x_scale).round() as i32;
                        pen += (sg.x_advance as f32 * mark.x_scale).round() as i32;
                        paged_text::PositionedGlyph {
                            glyph_id: sg.glyph_id,
                            cluster: g.cluster,
                            x,
                            y: sg.y_offset,
                            x_advance: (sg.x_advance as f32 * mark.x_scale).round() as i32,
                            font_id: font.outline,
                            point_size: mark.size,
                            underline: false,
                            strikethru: false,
                            substituted: false,
                            x_scale: mark.x_scale,
                            y_scale: mark.y_scale,
                            skew_deg: 0.0,
                            ch: Some(ch),
                            upright: sg.upright,
                        }
                    })
                    .collect();
                let draw_at = if vertical {
                    mark_baseline - (em.top - 0.5) * mark.size
                } else {
                    mark_baseline
                };
                emit_glyph_slice(
                    &glyphs,
                    font,
                    mark.size,
                    |_| paint,
                    (frame_origin_pt.0 + x0, frame_origin_pt.1 + draw_at),
                    &TtfOutliner::new(outline),
                    list,
                );
            }
        }
    }
}
