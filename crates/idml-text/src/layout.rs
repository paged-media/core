//! Positioned glyphs — the handoff format to the GPU rasterizer.
//!
//! Composes a paragraph into lines, shapes each line, and walks the
//! glyphs to turn per-glyph advances into absolute (x, y) coordinates
//! in 1/64 pt, frame-origin-relative.
//!
//! Alignment is a post-shape pass. Left/right/center shift each line's
//! glyphs by a constant. Justify distributes the leftover width across
//! the line's inter-word glue (glyphs whose cluster points at a
//! whitespace byte in the source paragraph).

use std::ops::Range;

use paragraph_breaker::{Breakpoint, Item};
use rustybuzz::Face;

use crate::compose::{compose_paragraph, ComposeOptions, TextShaper};
use crate::shape::{apply_tracking, shape_run, ShapedRun, ADVANCE_PRECISION};

/// A glyph positioned in frame space, ready for rasterization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub glyph_id: u32,
    /// Byte offset within the source paragraph.
    pub cluster: u32,
    /// Frame-origin-relative x, 1/64 pt.
    pub x: i32,
    /// Frame-origin-relative y (baseline + per-glyph y_offset), 1/64 pt.
    pub y: i32,
    /// Per-glyph horizontal advance, 1/64 pt. Carried so emission
    /// can compute the right-edge of contiguous decoration runs
    /// (underline / strikethrough) without re-shaping.
    pub x_advance: i32,
    /// Font id this glyph was shaped with. Single-font layouts
    /// (`layout_paragraph`) leave this 0; multi-font layouts
    /// (`layout_runs`) set it per run so the rasterizer can route
    /// glyph outlining through the right face.
    pub font_id: u32,
    /// Point size this glyph was shaped at. Single-font layouts
    /// leave this 0.0; multi-font layouts set it per run so emission
    /// can scale outlines with the correct em ratio.
    pub point_size: f32,
    /// Underline / strikethrough flags lifted from the run.
    /// Multi-font layouts (`layout_runs`) populate these from the
    /// originating `StyledRun`. Single-font `layout_paragraph` leaves
    /// them false.
    pub underline: bool,
    pub strikethru: bool,
}

#[derive(Debug, Clone)]
pub struct LaidOutLine {
    pub byte_range: Range<usize>,
    /// Baseline y, 1/64 pt, frame-origin-relative.
    pub baseline_y: i32,
    /// Natural (unjustified) width of the line, 1/64 pt.
    pub width: i32,
    /// Paragraph-breaker ratio. 0 = natural, >0 = stretched (would be
    /// justified), <0 = shrunk.
    pub ratio: f32,
    pub glyphs: Vec<PositionedGlyph>,
}

#[derive(Debug, Clone)]
pub struct LaidOutParagraph {
    pub lines: Vec<LaidOutLine>,
}

/// Paragraph-level horizontal alignment.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Right,
    Center,
    /// Fully justified — the last line of a paragraph stays
    /// left-aligned (common typographic convention). Intermediate
    /// lines distribute extra width across inter-word glue.
    Justify,
}

#[derive(Debug, Clone)]
pub struct LayoutOptions<'a> {
    pub compose: ComposeOptions<'a>,
    /// Distance between baselines, 1/64 pt.
    pub line_height: i32,
    /// Offset of the first baseline from the top of the paragraph box,
    /// 1/64 pt.
    pub first_baseline: i32,
    /// Horizontal alignment. Left by default.
    pub alignment: Alignment,
    /// When `Some`, every line's height is forced to this value
    /// (1/64 pt) instead of being computed from glyph point sizes
    /// (auto leading). Mirrors IDML's explicit `Leading` attribute on
    /// the leading run of a paragraph.
    pub leading_override: Option<i32>,
}

impl LayoutOptions<'_> {
    /// Convenience constructor from point-unit inputs. Uses 1.2×
    /// point_size as the default line height (common InDesign default
    /// for Auto leading) and `0.8 × point_size` for the first baseline.
    pub fn new(column_width_pt: f32, point_size: f32) -> Self {
        let line_height = (point_size * 1.2 * ADVANCE_PRECISION).round() as i32;
        let first_baseline = (point_size * 0.8 * ADVANCE_PRECISION).round() as i32;
        Self {
            compose: ComposeOptions::new(column_width_pt),
            line_height,
            first_baseline,
            leading_override: None,
            alignment: Alignment::Left,
        }
    }
}

/// Lay out `text` through `shaper` (which provides both widths for the
/// composer and glyph outlines for positioning).
pub fn layout_paragraph<S: TextShaper>(
    text: &str,
    shaper: &S,
    options: &LayoutOptions,
) -> LaidOutParagraph {
    let composed = compose_paragraph(text, shaper, &options.compose);
    let last_index = composed.len().saturating_sub(1);
    let mut lines = Vec::with_capacity(composed.len());
    let mut baseline = options.first_baseline;

    for (i, line) in composed.iter().enumerate() {
        let slice = &text[line.byte_range.clone()];
        // For hyphenated lines we shape `slice + "-"` so the trailing
        // hyphen sits in the same shaping context as the word part
        // (some fonts apply contextual kerning to "-"). The hyphen
        // glyph carries the `cluster` of the line's last byte so
        // run-paint pickers attribute it to the word, not the next.
        let owned;
        let to_shape: &str = if line.ends_with_hyphen {
            owned = format!("{slice}-");
            &owned
        } else {
            slice
        };
        let shaped = shaper.shape(to_shape);
        let mut glyphs = position_line(&shaped, 0, baseline, line.byte_range.start as u32);
        if line.ends_with_hyphen {
            // The last glyph corresponds to the synthetic "-" — pin
            // its cluster to the line's last source byte so it picks
            // up the right run paint and doesn't claim a cluster
            // beyond the line's byte range.
            if let Some(last) = glyphs.last_mut() {
                last.cluster = line.byte_range.end.saturating_sub(1) as u32;
            }
        }
        let is_last = i == last_index;
        apply_alignment(
            &mut glyphs,
            shaped.total_advance,
            options.column_width(),
            options.alignment,
            is_last,
            text.as_bytes(),
        );
        lines.push(LaidOutLine {
            byte_range: line.byte_range.clone(),
            baseline_y: baseline,
            width: shaped.total_advance,
            ratio: line.ratio,
            glyphs,
        });
        baseline += options.line_height;
    }

    LaidOutParagraph { lines }
}

impl LayoutOptions<'_> {
    /// Column width in 1/64 pt (convenience for layout passes).
    pub fn column_width(&self) -> i32 {
        self.compose.column_width
    }
}

/// Walk a `ShapedRun`'s advances and turn them into absolute positions.
///
/// `start_x` and `baseline_y` are in 1/64 pt, frame-origin-relative.
/// `cluster_base` is added to each glyph's intra-slice cluster so the
/// output carries byte offsets back into the source paragraph.
pub fn position_line(
    shaped: &ShapedRun,
    start_x: i32,
    baseline_y: i32,
    cluster_base: u32,
) -> Vec<PositionedGlyph> {
    let mut out = Vec::with_capacity(shaped.glyphs.len());
    let mut pen_x = start_x;
    for g in &shaped.glyphs {
        out.push(PositionedGlyph {
            glyph_id: g.glyph_id,
            cluster: cluster_base + g.cluster,
            x: pen_x + g.x_offset,
            y: baseline_y + g.y_offset,
            x_advance: g.x_advance,
            font_id: 0,
            point_size: 0.0,
            underline: false,
            strikethru: false,
        });
        pen_x += g.x_advance;
    }
    out
}

/// Shift / justify a line's glyphs in-place.
///
/// `natural_width` is the sum of advances (= `ShapedRun::total_advance`).
/// `column_width` is the target column width. Both in 1/64 pt.
///
/// For `Justify`, the last line of a paragraph stays left-aligned
/// (indicated by `is_last_line`) to avoid stretching a short tail line.
fn apply_alignment(
    glyphs: &mut [PositionedGlyph],
    natural_width: i32,
    column_width: i32,
    alignment: Alignment,
    is_last_line: bool,
    paragraph_bytes: &[u8],
) {
    if glyphs.is_empty() || column_width <= 0 {
        return;
    }
    let extra = column_width - natural_width;
    match alignment {
        Alignment::Left => {}
        Alignment::Right => {
            for g in glyphs.iter_mut() {
                g.x += extra;
            }
        }
        Alignment::Center => {
            let shift = extra / 2;
            for g in glyphs.iter_mut() {
                g.x += shift;
            }
        }
        Alignment::Justify => {
            if is_last_line || extra <= 0 {
                return;
            }
            // Count glyphs whose cluster points at a whitespace byte
            // (skipping the first glyph so we don't indent the line).
            let space_count = glyphs
                .iter()
                .skip(1)
                .filter(|g| is_ws_at(paragraph_bytes, g.cluster as usize))
                .count() as i32;
            if space_count == 0 {
                return;
            }
            let per_space = extra / space_count;
            let remainder = extra - per_space * space_count;
            // Walk glyphs left-to-right, accumulating a shift as each
            // space is encountered. Integer division leaves a small
            // remainder which we bleed into the first few spaces so
            // the last glyph lands exactly on the column edge.
            let mut shift = 0i32;
            let mut spaces_seen = 0i32;
            for (i, g) in glyphs.iter_mut().enumerate() {
                if i > 0 && is_ws_at(paragraph_bytes, g.cluster as usize) {
                    let bleed = if spaces_seen < remainder { 1 } else { 0 };
                    shift += per_space + bleed;
                    spaces_seen += 1;
                }
                g.x += shift;
            }
        }
    }
}

fn is_ws_at(bytes: &[u8], i: usize) -> bool {
    matches!(
        bytes.get(i),
        Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
}

/// One styled run inside a paragraph. The pipeline assembles a
/// `Vec<StyledRun>` per paragraph; `layout_runs` shapes each run
/// with its own face, drives the line breaker against the
/// concatenated advances, and tags every output glyph with
/// the run's `font_id` and `point_size` so emission can route
/// outlining through the right face.
pub struct StyledRun<'a> {
    pub text: &'a str,
    pub face: &'a Face<'a>,
    pub point_size: f32,
    pub tracking: Option<f32>,
    pub font_id: u32,
    pub underline: bool,
    pub strikethru: bool,
    /// IDML `BaselineShift` in pt. Positive lifts glyphs above the
    /// baseline; negative drops them. Applied per-glyph after layout
    /// so the line height (computed from font metrics + leading)
    /// stays unchanged.
    pub baseline_shift_pt: f32,
}

/// Multi-font flavour of [`layout_paragraph`].
///
/// Pre-shapes each run with its own face (so advances reflect the
/// run's font + size), runs paragraph-breaker on the concatenated
/// item stream, and slices the resulting glyphs back per line. Every
/// `PositionedGlyph` carries the originating run's `font_id` and
/// `point_size`.
///
/// Hyphenation is intentionally not threaded through here yet —
/// `layout_paragraph` keeps that path while this batch lands.
pub fn layout_runs(runs: &[StyledRun], options: &LayoutOptions) -> LaidOutParagraph {
    if runs.is_empty() {
        return LaidOutParagraph { lines: Vec::new() };
    }

    // 1. Concatenate run text and remember the byte offset where
    // each run starts. Then shape every run with its face so the
    // glyph advances reflect that run's font.
    let mut paragraph_text = String::new();
    let mut run_starts = Vec::with_capacity(runs.len());
    let mut run_shapes: Vec<ShapedRun> = Vec::with_capacity(runs.len());
    for r in runs {
        run_starts.push(paragraph_text.len());
        paragraph_text.push_str(r.text);
        let mut s = shape_run(r.face, r.text, r.point_size);
        if let Some(t) = r.tracking {
            apply_tracking(&mut s, t, r.point_size);
        }
        run_shapes.push(s);
    }

    // 2. Build a flat array of (paragraph-cluster, run_index, glyph)
    // entries sorted by cluster. paragraph-breaker only needs widths
    // grouped by word; rendering needs the original glyph data
    // sliced by line. Both pull off this single source of truth.
    let mut flat: Vec<FlatGlyph> = Vec::new();
    for (run_i, shape) in run_shapes.iter().enumerate() {
        let base = run_starts[run_i] as u32;
        for g in &shape.glyphs {
            flat.push(FlatGlyph {
                cluster: base + g.cluster,
                run_idx: run_i,
                x_advance: g.x_advance,
                x_offset: g.x_offset,
                y_offset: g.y_offset,
                glyph_id: g.glyph_id,
            });
        }
    }
    // Each run's pre-shape emits glyphs in cluster order, and runs
    // append in run order with monotonically-increasing `base`
    // cluster offsets — so `flat` is already globally sorted by
    // cluster. The invariant matters because `run_index_for_word`
    // and `sum_advances_in` walk it in order.
    debug_assert!(
        flat.windows(2).all(|w| w[0].cluster <= w[1].cluster),
        "FlatGlyph cluster ordering invariant violated"
    );

    // 3. Build paragraph-breaker items: one Box per word (sum of
    // advances of glyphs whose cluster is within the word range),
    // glue between words, infinite-stretch glue + forced break at
    // the end. Track byte_end alongside each item so we can map
    // breakpoint indices back to source byte offsets.
    let words = segment_paragraph(&paragraph_text);
    if words.is_empty() {
        return LaidOutParagraph { lines: Vec::new() };
    }
    let opts = &options.compose;
    // Use the first run's space width as the glue width — IDML
    // doesn't change inter-word spacing across runs, and pulling a
    // per-word space face would require a synthetic face index.
    let space_width = shape_run(runs[0].face, " ", runs[0].point_size).total_advance;
    let stretch = (space_width as f32 * opts.stretch_ratio).round() as i32;
    let shrink = (space_width as f32 * opts.shrink_ratio).round() as i32;

    let mut items: Vec<Item<()>> = Vec::with_capacity(words.len() * 4 + 2);
    let mut byte_ends: Vec<usize> = Vec::with_capacity(items.capacity());
    let mut is_hyphen: Vec<bool> = Vec::with_capacity(items.capacity());
    for (i, w) in words.iter().enumerate() {
        // Hyphenate iff the word is entirely within one run AND a
        // hyphenator is configured. Multi-run words (rare — usually a
        // bold "hold" + italic "ing") fall through to a single Box;
        // they still break at glue boundaries.
        let single_run = run_index_for_word(&flat, w.start as u32, w.end as u32);
        let breaks: Vec<usize> = match (opts.hyphenator, single_run) {
            (Some(h), Some(_)) => {
                let word_text = &paragraph_text[w.start..w.end];
                h.opportunities(word_text)
                    .into_iter()
                    .filter(|&b| b > 0 && b < word_text.len())
                    .map(|b| w.start + b)
                    .collect()
            }
            _ => Vec::new(),
        };
        let hyphen_width = if !breaks.is_empty() {
            let r = &runs[single_run.unwrap()];
            shape_run(r.face, "-", r.point_size).total_advance
        } else {
            0
        };
        let mut seg_start = w.start;
        for offset in &breaks {
            let seg_width = sum_advances_in(&flat, seg_start as u32..*offset as u32);
            items.push(Item::Box {
                width: seg_width,
                data: (),
            });
            byte_ends.push(*offset);
            is_hyphen.push(false);
            items.push(Item::Penalty {
                width: hyphen_width,
                penalty: opts.hyphen_penalty,
                flagged: true,
            });
            byte_ends.push(*offset);
            is_hyphen.push(true);
            seg_start = *offset;
        }
        let final_width = sum_advances_in(&flat, seg_start as u32..w.end as u32);
        items.push(Item::Box {
            width: final_width,
            data: (),
        });
        byte_ends.push(w.end);
        is_hyphen.push(false);
        if i + 1 < words.len() {
            items.push(Item::Glue {
                width: space_width,
                stretch,
                shrink,
            });
            byte_ends.push(w.end);
            is_hyphen.push(false);
        }
    }
    items.push(Item::Glue {
        width: 0,
        stretch: paragraph_breaker::INFINITE_PENALTY,
        shrink: 0,
    });
    byte_ends.push(paragraph_text.len());
    is_hyphen.push(false);
    items.push(Item::Penalty {
        width: 0,
        penalty: -paragraph_breaker::INFINITE_PENALTY,
        flagged: true,
    });
    byte_ends.push(paragraph_text.len());
    is_hyphen.push(false);

    let single_width = [opts.column_width];
    let lengths: &[i32] = opts
        .column_widths
        .as_deref()
        .filter(|v| !v.is_empty())
        .unwrap_or(&single_width);
    // paragraph_breaker returns an empty break list when no feasible
    // fit exists at the configured tolerance. Very long real-world
    // paragraphs that interleave many run-color-switch boxes (e.g.
    // body copy that repeats the same sentence 60+ times with no
    // hyphenation opportunities) can trip this. Retry at
    // progressively looser tolerance so the breaker still produces
    // lines instead of dropping the paragraph entirely — the
    // resulting lines won't be perfectly tight, but a slightly looser
    // break beats no break at all.
    // paragraph_breaker returns an empty break list when no feasible
    // fit exists at the configured tolerance. Real-world body copy
    // that interleaves many run-color-switch boxes or runs past the
    // configured per-line `column_widths` slice can trip this. Retry
    // at progressively looser tolerance so the breaker still produces
    // lines instead of dropping the paragraph entirely — the
    // resulting lines won't be perfectly tight, but a slightly looser
    // break beats no break at all.
    let mut breaks: Vec<Breakpoint> =
        paragraph_breaker::total_fit(&items, lengths, opts.tolerance, opts.looseness);
    if breaks.is_empty() && !items.is_empty() {
        for fallback_tol in [opts.tolerance * 4.0, opts.tolerance * 16.0, 1_000.0] {
            breaks = paragraph_breaker::total_fit(&items, lengths, fallback_tol, opts.looseness);
            if !breaks.is_empty() {
                break;
            }
        }
    }

    // 4. For each chosen line, walk `flat` in cluster order and pull
    // glyphs whose cluster is in the line's byte range. Position
    // them with a running pen and tag with the run's font_id +
    // point_size so emission can route outlining.
    let mut lines = Vec::with_capacity(breaks.len());
    let mut byte_cursor = 0usize;
    let mut baseline = options.first_baseline;
    let last_break = breaks.len().saturating_sub(1);
    let bytes = paragraph_text.as_bytes();
    // Per-line widths drive both Knuth-Plass and post-layout
    // alignment: when an obstacle carves a line down to ~half-column
    // width, the right/center/justify alignments should snap glyphs
    // against THAT width, not the original scalar column. Defaults
    // to the scalar width when no per-line override is configured.
    let per_line_widths: Option<&[i32]> = opts
        .column_widths
        .as_deref()
        .filter(|v| !v.is_empty());
    for (i, bp) in breaks.iter().enumerate() {
        let Some(&end) = byte_ends.get(bp.index) else {
            continue;
        };
        let hyphenated = is_hyphen.get(bp.index).copied().unwrap_or(false);
        let mut start = byte_cursor;
        while start < end && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        if start >= end {
            continue;
        }
        let mut glyphs: Vec<PositionedGlyph> = Vec::new();
        let mut pen_x: i32 = 0;
        let mut last_run_idx: Option<usize> = None;
        for fg in &flat {
            if fg.cluster < start as u32 || fg.cluster >= end as u32 {
                continue;
            }
            let run = &runs[fg.run_idx];
            // IDML BaselineShift is positive-up; layout y grows down,
            // so subtract.
            let baseline_shift_64 = (run.baseline_shift_pt * ADVANCE_PRECISION).round() as i32;
            glyphs.push(PositionedGlyph {
                glyph_id: fg.glyph_id,
                cluster: fg.cluster,
                x: pen_x + fg.x_offset,
                y: baseline + fg.y_offset - baseline_shift_64,
                x_advance: fg.x_advance,
                font_id: run.font_id,
                point_size: run.point_size,
                underline: run.underline,
                strikethru: run.strikethru,
            });
            pen_x += fg.x_advance;
            last_run_idx = Some(fg.run_idx);
        }
        // Append a synthetic hyphen glyph for hyphenated breaks,
        // shaped with the run that owns the line's last glyph. The
        // hyphen carries the line's last source byte as its cluster
        // so per-cluster paint pickers attribute it to the same run.
        if hyphenated {
            if let Some(idx) = last_run_idx {
                let r = &runs[idx];
                let baseline_shift_64 = (r.baseline_shift_pt * ADVANCE_PRECISION).round() as i32;
                let hyphen_shape = shape_run(r.face, "-", r.point_size);
                for g in &hyphen_shape.glyphs {
                    glyphs.push(PositionedGlyph {
                        glyph_id: g.glyph_id,
                        cluster: end.saturating_sub(1) as u32,
                        x: pen_x + g.x_offset,
                        y: baseline + g.y_offset - baseline_shift_64,
                        x_advance: g.x_advance,
                        font_id: r.font_id,
                        point_size: r.point_size,
                        underline: r.underline,
                        strikethru: r.strikethru,
                    });
                    pen_x += g.x_advance;
                }
            }
        }
        let natural_width = pen_x;
        // When `column_widths` is configured, use the matching slot
        // for this line's alignment column (clamping at the slice
        // tail mirrors paragraph-breaker's own fallback). Right /
        // center / justify therefore align against the line's
        // *available* width, which is what the wrap-around-objects
        // pass needs.
        let line_column = per_line_widths
            .map(|w| w[i.min(w.len() - 1)])
            .unwrap_or_else(|| options.column_width());
        apply_alignment(
            &mut glyphs,
            natural_width,
            line_column,
            options.alignment,
            i == last_break,
            bytes,
        );
        // Per-line line-height: explicit `leading_override` wins
        // (mirrors IDML's `Leading` attribute), otherwise the largest
        // run's point size on the line × 1.2 (Adobe's Auto leading
        // default), with `options.line_height` as the empty-line
        // fallback.
        let line_height = options
            .leading_override
            .or_else(|| max_line_height_for_glyphs(&glyphs))
            .unwrap_or(options.line_height);
        lines.push(LaidOutLine {
            byte_range: start..end,
            baseline_y: baseline,
            width: natural_width,
            ratio: bp.ratio,
            glyphs,
        });
        baseline += line_height;
        byte_cursor = end;
    }
    LaidOutParagraph { lines }
}

/// Auto-leading line height for a line of glyphs, in 1/64 pt:
/// `max(glyph.point_size) * 1.2 * 64`. Returns `None` for an empty
/// line so callers can fall back to a default.
pub fn max_line_height_for_glyphs(glyphs: &[PositionedGlyph]) -> Option<i32> {
    glyphs
        .iter()
        .map(|g| g.point_size)
        .fold(None, |acc: Option<f32>, ps| {
            Some(acc.map(|a| a.max(ps)).unwrap_or(ps))
        })
        .map(|max_pt| (max_pt * 1.2 * ADVANCE_PRECISION).round() as i32)
}

/// In-cell alignment for a tab stop. IDML's `Alignment` attribute on
/// `<TabStop>` distinguishes how text following the tab snaps
/// against `Position`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAlignment {
    /// Default. Text after the tab starts AT the stop.
    Left,
    /// Text after the tab ends AT the stop.
    Right,
    /// Text after the tab is centred ON the stop.
    Center,
    /// `CharacterAlign` — the alignment character (typically `.`)
    /// in the segment lands AT the stop. Segments without the
    /// character fall through to Left.
    Decimal,
}

/// One tab stop spec with position, alignment, and (for Decimal)
/// the character to align on.
#[derive(Debug, Clone, PartialEq)]
pub struct TabStopSpec {
    pub position_pt: f32,
    pub alignment: TabAlignment,
    /// Character the segment aligns on for `Decimal` stops.
    /// IDML defaults to `.` when `<TabStop AlignmentCharacter>` is
    /// absent. Ignored for the other alignments.
    pub alignment_character: char,
    /// Leader string tiled across the widened tab gap.
    /// IDML's `<TabStop Leader="..." />` is typically `"."` (TOC dot
    /// leaders), `". "` (dot + space pattern), or empty. `None` /
    /// empty ⇒ no leader. The renderer's `apply_tab_stops_with_leaders`
    /// shapes the leader with the run owning the `\t` and tiles whole
    /// copies across the gap; partial trailing copies are dropped to
    /// keep the leader visually uniform (matches InDesign behaviour
    /// where a leader wider than the gap emits nothing rather than
    /// overflowing into the snapped text).
    pub leader: Option<String>,
}

impl TabStopSpec {
    /// Convenience: a Left-aligned stop at `position_pt`. The
    /// `alignment_character` defaults to `.` so future Decimal
    /// stops in the same array don't have to be initialised
    /// specially.
    pub fn left(position_pt: f32) -> Self {
        Self {
            position_pt,
            alignment: TabAlignment::Left,
            alignment_character: '.',
            leader: None,
        }
    }
}

/// Snap each `\t` glyph in `line` to the next tab stop. Widens the
/// tab's `x_advance` and pushes every following glyph right by the
/// resulting delta so the segment after the tab lands per the
/// stop's alignment:
///
///  - `Left`: segment starts at the stop.
///  - `Right`: segment ends at the stop.
///  - `Center`: segment is centred on the stop.
///  - `Decimal`: the segment's first occurrence of
///    `alignment_character` (typically `.`) lands at the stop.
///    Segments without the character fall through to Left.
///
/// When the stop's alignment can't be honoured (e.g. Right with a
/// segment wider than the gap to the stop), falls through to Left
/// for that tab so glyphs never collide.
///
/// `tab_stops` is sorted by position (pt). Falls back to a
/// `default_stop_pt` grid (IDML default: 36 pt) when no explicit
/// stop sits past the current pen.
pub fn apply_tab_stops(
    line: &mut LaidOutLine,
    paragraph_text: &str,
    tab_stops: &[TabStopSpec],
    default_stop_pt: f32,
) {
    apply_tab_stops_with_leaders(line, paragraph_text, tab_stops, default_stop_pt, None);
}

/// Like [`apply_tab_stops`] but with optional leader-character
/// emission for stops whose `<TabStop Leader="...">` carries a
/// non-empty string. When `leader_ctx` is `Some`, each snapped tab
/// whose stop has a leader gets the leader string shaped (using the
/// run that owns the tab's `\t` cluster) and tiled across the gap;
/// the synthesised leader glyphs are inserted into `line.glyphs`
/// between the tab and the following segment.
///
/// Tiling strategy: whole copies only. The number of full leader
/// copies that fit is `floor(gap / leader_width)`; any partial
/// trailing space is left empty so the leader stays visually uniform
/// and never collides with the snapped text. A leader strictly wider
/// than the gap emits zero copies (matches InDesign's behaviour of
/// dropping the leader when there isn't room for one whole repeat).
///
/// The shaped leader glyphs are placed with absolute `x` values
/// inside the already-widened tab's span, so they do not contribute
/// further advance to subsequent glyphs (positions are absolute by
/// this stage of the pipeline). The leader inherits the font /
/// point_size / paint of the run that owns the `\t`; IDML doesn't
/// expose a separate style for the leader characters.
pub fn apply_tab_stops_with_leaders(
    line: &mut LaidOutLine,
    paragraph_text: &str,
    tab_stops: &[TabStopSpec],
    default_stop_pt: f32,
    leader_ctx: Option<&LeaderContext<'_, '_>>,
) {
    let bytes = paragraph_text.as_bytes();
    let default_stop_64 = (default_stop_pt * ADVANCE_PRECISION).round() as i32;
    if default_stop_64 <= 0 && tab_stops.is_empty() {
        return;
    }
    let mut i = 0;
    while i < line.glyphs.len() {
        let cluster = line.glyphs[i].cluster as usize;
        if cluster >= bytes.len() || bytes[cluster] != b'\t' {
            i += 1;
            continue;
        }
        let current_x = line.glyphs[i].x;
        let (next_stop_64, alignment, decimal_char, leader_str) =
            next_tab_stop_at(current_x, tab_stops, default_stop_64);
        if next_stop_64 <= current_x {
            i += 1;
            continue;
        }
        let segment_end = next_tab_or_end(&line.glyphs, i, bytes);
        let target_segment_left = match alignment {
            TabAlignment::Right => {
                let segment_width = segment_natural_width(&line.glyphs, i + 1, segment_end);
                next_stop_64 - segment_width
            }
            TabAlignment::Center => {
                let segment_width = segment_natural_width(&line.glyphs, i + 1, segment_end);
                next_stop_64 - segment_width / 2
            }
            TabAlignment::Decimal => {
                // Find the alignment character's natural offset
                // inside the segment (0 = right at segment start)
                // and back the segment up so it lands on the stop.
                // Falls through to Left when the char is missing.
                match decimal_offset(
                    &line.glyphs,
                    i + 1,
                    segment_end,
                    paragraph_text,
                    decimal_char,
                ) {
                    Some(off) => next_stop_64 - off,
                    None => next_stop_64,
                }
            }
            TabAlignment::Left => next_stop_64,
        };
        let original_advance = line.glyphs[i].x_advance;
        let mut new_advance = target_segment_left - current_x;
        // Tabs can only widen — if non-Left alignment would shrink
        // the tab below its natural advance, fall through to Left
        // at the stop.
        if new_advance < original_advance && alignment != TabAlignment::Left {
            new_advance = next_stop_64 - current_x;
        }
        let delta = new_advance - original_advance;
        if delta > 0 {
            for g in &mut line.glyphs[(i + 1)..] {
                g.x += delta;
            }
            line.glyphs[i].x_advance = new_advance;
            line.width += delta;
        }
        // Leader emission: after the tab has been widened, tile the
        // leader string across [current_x .. target_segment_left]
        // using the run that owns this tab's `\t` cluster.
        let leader_glyphs = leader_ctx
            .zip(leader_str)
            .filter(|(_, s)| !s.is_empty())
            .and_then(|(ctx, leader)| {
                let gap_64 = target_segment_left - current_x;
                if gap_64 <= 0 {
                    return None;
                }
                ctx.shape_leader_for_tab(line.glyphs[i].cluster, leader, gap_64, line.glyphs[i].y)
            })
            .unwrap_or_default();
        if !leader_glyphs.is_empty() {
            let n = leader_glyphs.len();
            let insert_at = i + 1;
            // Shift positioned glyphs to absolute x within the tab's
            // widened span. shape_leader_for_tab returns glyphs whose
            // x is relative to current_x.
            let mut adjusted: Vec<PositionedGlyph> = leader_glyphs;
            for g in &mut adjusted {
                g.x += current_x;
            }
            line.glyphs.splice(insert_at..insert_at, adjusted);
            // Skip past the inserted leader glyphs — they don't carry
            // tabs and shouldn't trigger another snap.
            i += n;
        }
        i += 1;
    }
}

fn next_tab_stop_at(
    current_x_64: i32,
    stops: &[TabStopSpec],
    default_stop_64: i32,
) -> (i32, TabAlignment, char, Option<&str>) {
    for spec in stops {
        let stop_64 = (spec.position_pt * ADVANCE_PRECISION).round() as i32;
        if stop_64 > current_x_64 {
            return (
                stop_64,
                spec.alignment,
                spec.alignment_character,
                spec.leader.as_deref(),
            );
        }
    }
    if default_stop_64 <= 0 {
        return (current_x_64, TabAlignment::Left, '.', None);
    }
    let n = current_x_64 / default_stop_64 + 1;
    (n * default_stop_64, TabAlignment::Left, '.', None)
}

/// Per-paragraph context the leader-aware tab pass needs to shape
/// `<TabStop Leader="...">` characters with the right font + size.
/// Wraps the styled run slice the paragraph was laid out with so
/// `apply_tab_stops_with_leaders` can look up which run owns a given
/// `\t` cluster and reuse its `Face` for the leader glyphs.
pub struct LeaderContext<'a, 'b> {
    pub runs: &'a [StyledRun<'b>],
    /// `runs[i].text` starts at byte `run_starts[i]` in the
    /// concatenated paragraph text the layout pass saw. Used to map a
    /// glyph cluster back to its originating run.
    pub run_starts: Vec<usize>,
}

impl<'a, 'b> LeaderContext<'a, 'b> {
    /// Build a context from a styled-run slice. `run_starts` is
    /// derived by accumulating each run's byte length.
    pub fn new(runs: &'a [StyledRun<'b>]) -> Self {
        let mut starts = Vec::with_capacity(runs.len());
        let mut acc = 0usize;
        for r in runs {
            starts.push(acc);
            acc += r.text.len();
        }
        Self {
            runs,
            run_starts: starts,
        }
    }

    fn run_for_cluster(&self, cluster: u32) -> Option<&StyledRun<'b>> {
        let cl = cluster as usize;
        let mut owner: Option<usize> = None;
        for (i, &s) in self.run_starts.iter().enumerate() {
            if s <= cl {
                owner = Some(i);
            } else {
                break;
            }
        }
        owner.and_then(|i| self.runs.get(i))
    }

    /// Shape `leader_str` with the run owning `tab_cluster` and tile
    /// whole copies across `gap_64` 1/64 pt. Returns positioned
    /// glyphs with `x` relative to the gap's left edge (caller adds
    /// the absolute offset). `y` is the baseline carried in from the
    /// tab glyph (so leader sits on the same baseline as the line).
    fn shape_leader_for_tab(
        &self,
        tab_cluster: u32,
        leader_str: &str,
        gap_64: i32,
        baseline_y: i32,
    ) -> Option<Vec<PositionedGlyph>> {
        let run = self.run_for_cluster(tab_cluster)?;
        let shape = shape_run(run.face, leader_str, run.point_size);
        if shape.glyphs.is_empty() || shape.total_advance <= 0 {
            return None;
        }
        let leader_w = shape.total_advance;
        let copies = (gap_64 / leader_w) as usize;
        if copies == 0 {
            return None;
        }
        let baseline_shift_64 = (run.baseline_shift_pt * ADVANCE_PRECISION).round() as i32;
        let mut out: Vec<PositionedGlyph> = Vec::with_capacity(shape.glyphs.len() * copies);
        let mut pen_x: i32 = 0;
        for _ in 0..copies {
            for g in &shape.glyphs {
                out.push(PositionedGlyph {
                    glyph_id: g.glyph_id,
                    // Carry the tab's cluster so per-cluster paint
                    // pickers attribute the leader to the same run.
                    cluster: tab_cluster,
                    x: pen_x + g.x_offset,
                    y: baseline_y + g.y_offset - baseline_shift_64,
                    x_advance: g.x_advance,
                    font_id: run.font_id,
                    point_size: run.point_size,
                    // Leaders don't carry underline / strikethrough —
                    // those decorations belong to the visible content
                    // runs, not the synthesised tab fill.
                    underline: false,
                    strikethru: false,
                });
                pen_x += g.x_advance;
            }
        }
        Some(out)
    }
}

/// Find the first byte offset of `target_char` in
/// `paragraph_text[clusters[start..end]]`, then return the natural
/// x position of that glyph relative to the segment's start.
/// `None` when the character isn't in the segment.
fn decimal_offset(
    glyphs: &[PositionedGlyph],
    start: usize,
    end: usize,
    paragraph_text: &str,
    target_char: char,
) -> Option<i32> {
    if start >= end || end > glyphs.len() {
        return None;
    }
    let segment_start_byte = glyphs[start].cluster as usize;
    let segment_end_byte = glyphs[end - 1].cluster as usize + 1;
    let bytes = paragraph_text.as_bytes();
    if segment_end_byte > bytes.len() {
        return None;
    }
    let mut buf = [0u8; 4];
    let needle = target_char.encode_utf8(&mut buf).as_bytes();
    let segment_bytes = &bytes[segment_start_byte..segment_end_byte];
    let pos = segment_bytes
        .windows(needle.len())
        .position(|w| w == needle)?;
    let target_cluster = (segment_start_byte + pos) as u32;
    let target_glyph = glyphs[start..end]
        .iter()
        .find(|g| g.cluster == target_cluster)?;
    Some(target_glyph.x - glyphs[start].x)
}

fn next_tab_or_end(glyphs: &[PositionedGlyph], from: usize, bytes: &[u8]) -> usize {
    for (j, g) in glyphs.iter().enumerate().skip(from + 1) {
        let cluster = g.cluster as usize;
        if cluster < bytes.len() && bytes[cluster] == b'\t' {
            return j;
        }
    }
    glyphs.len()
}

fn segment_natural_width(glyphs: &[PositionedGlyph], start: usize, end: usize) -> i32 {
    if start >= end || end > glyphs.len() {
        return 0;
    }
    let last = &glyphs[end - 1];
    let first_x = glyphs[start].x;
    last.x + last.x_advance - first_x
}

#[derive(Debug, Clone, Copy)]
struct FlatGlyph {
    cluster: u32,
    run_idx: usize,
    x_advance: i32,
    x_offset: i32,
    y_offset: i32,
    glyph_id: u32,
}

#[derive(Debug, Clone, Copy)]
struct WordSpan {
    start: usize,
    end: usize,
}

fn segment_paragraph(text: &str) -> Vec<WordSpan> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor > start {
            out.push(WordSpan { start, end: cursor });
        }
    }
    out
}

fn sum_advances_in(flat: &[FlatGlyph], range: Range<u32>) -> i32 {
    flat.iter()
        .filter(|g| g.cluster >= range.start && g.cluster < range.end)
        .map(|g| g.x_advance)
        .sum()
}

/// Returns `Some(run_idx)` when every glyph whose cluster falls in
/// `[start, end)` belongs to the same run, else `None`. Used to
/// gate hyphenation: a word that crosses a run boundary is rare and
/// needs per-segment hyphen widths we don't model yet.
fn run_index_for_word(flat: &[FlatGlyph], start: u32, end: u32) -> Option<usize> {
    let mut run: Option<usize> = None;
    for g in flat {
        if g.cluster < start || g.cluster >= end {
            continue;
        }
        match run {
            None => run = Some(g.run_idx),
            Some(r) if r != g.run_idx => return None,
            _ => {}
        }
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::MonospaceMeasurer;
    use crate::shape::ShapedGlyph;

    fn fake_run(advances: &[i32]) -> ShapedRun {
        let glyphs: Vec<ShapedGlyph> = advances
            .iter()
            .enumerate()
            .map(|(i, &adv)| ShapedGlyph {
                glyph_id: 100 + i as u32,
                cluster: i as u32,
                x_advance: adv,
                y_offset: 0,
                x_offset: 0,
            })
            .collect();
        ShapedRun {
            glyphs,
            total_advance: advances.iter().sum(),
        }
    }

    fn opts(column_chars: i32, alignment: Alignment) -> LayoutOptions<'static> {
        LayoutOptions {
            compose: ComposeOptions {
                column_width: column_chars * 10,
                column_widths: None,
                tolerance: 10.0,
                stretch_ratio: 1.0,
                shrink_ratio: 0.5,
                looseness: 0,
                hyphenator: None,
                hyphen_penalty: 50,
                kinsoku_enforce: false,
            },
            line_height: 20,
            first_baseline: 15,
            alignment,
            leading_override: None,
        }
    }

    #[test]
    fn position_line_accumulates_advances() {
        let run = fake_run(&[100, 80, 120]);
        let out = position_line(&run, 50, 200, 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].x, 50);
        assert_eq!(out[1].x, 150);
        assert_eq!(out[2].x, 230);
        for g in &out {
            assert_eq!(g.y, 200);
        }
    }

    #[test]
    fn position_line_applies_offsets() {
        let run = ShapedRun {
            glyphs: vec![ShapedGlyph {
                glyph_id: 1,
                cluster: 0,
                x_advance: 100,
                x_offset: 5,
                y_offset: -7,
            }],
            total_advance: 100,
        };
        let out = position_line(&run, 10, 50, 0);
        assert_eq!(out[0].x, 15); // 10 + x_offset 5
        assert_eq!(out[0].y, 43); // 50 + y_offset -7
    }

    #[test]
    fn position_line_offsets_cluster_by_base() {
        let run = fake_run(&[10, 10]);
        let out = position_line(&run, 0, 0, 42);
        assert_eq!(out[0].cluster, 42);
        assert_eq!(out[1].cluster, 43);
    }

    #[test]
    fn left_alignment_leaves_glyphs_at_zero() {
        let shaper = MonospaceMeasurer::new(10, 10);
        let out = layout_paragraph("ab", &shaper, &opts(20, Alignment::Left));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 0);
    }

    #[test]
    fn right_alignment_pushes_line_to_column_edge() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // "ab" = 20 units, column = 200, expected shift = 180.
        let out = layout_paragraph("ab", &shaper, &opts(20, Alignment::Right));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 180);
    }

    #[test]
    fn center_alignment_halves_the_gap() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // "ab" = 20, column = 200, gap = 180, shift = 90.
        let out = layout_paragraph("ab", &shaper, &opts(20, Alignment::Center));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 90);
    }

    #[test]
    fn justify_last_line_stays_left_aligned() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // Only one line — it IS the last — so justify stays at 0.
        let out = layout_paragraph("ab cd", &shaper, &opts(20, Alignment::Justify));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 0);
    }

    #[test]
    fn justify_stretches_intermediate_lines_to_column() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // Column = 80, paragraph "ab cd ef gh ij kl" → multiple lines.
        // Intermediate lines should land the last glyph exactly on the
        // right column edge.
        let out = layout_paragraph("ab cd ef gh ij kl", &shaper, &opts(8, Alignment::Justify));
        assert!(out.lines.len() >= 2, "need ≥ 2 lines to exercise justify");
        let non_last: Vec<_> = out.lines.iter().take(out.lines.len() - 1).collect();
        for line in non_last {
            let last_glyph = line.glyphs.last().unwrap();
            // Last glyph sits at column_edge - last_glyph_advance.
            // advance = 10, column = 80 → last glyph x ≥ 70.
            assert!(
                last_glyph.x >= 70 - 2 && last_glyph.x <= 70 + 2,
                "expected last glyph near 70, got {}",
                last_glyph.x
            );
        }
    }

    #[test]
    fn layout_paragraph_uses_monospace_shaper_end_to_end() {
        let shaper = MonospaceMeasurer::new(10, 10);
        let o = opts(12, Alignment::Left);
        let out = layout_paragraph("lorem ipsum dolor sit amet", &shaper, &o);

        assert!(!out.lines.is_empty(), "no lines emitted");
        for w in out.lines.windows(2) {
            assert_eq!(w[1].baseline_y - w[0].baseline_y, 20);
        }
        assert_eq!(out.lines[0].baseline_y, 15);
        let line0 = &out.lines[0];
        for pair in line0.glyphs.windows(2) {
            assert!(pair[0].x <= pair[1].x);
        }
        let expected_width: i32 = line0.glyphs.iter().map(|_| 10).sum::<i32>();
        assert_eq!(line0.width, expected_width);
    }

    fn pg(point_size: f32) -> PositionedGlyph {
        PositionedGlyph {
            glyph_id: 0,
            cluster: 0,
            x: 0,
            y: 0,
            x_advance: 0,
            font_id: 0,
            point_size,
            underline: false,
            strikethru: false,
        }
    }

    #[test]
    fn auto_leading_picks_largest_run_size() {
        let glyphs = vec![pg(11.0), pg(22.0), pg(11.0)];
        // 22 * 1.2 * 64 = 1689.6 → 1690.
        assert_eq!(max_line_height_for_glyphs(&glyphs), Some(1690));
    }

    #[test]
    fn auto_leading_returns_none_for_empty_line() {
        assert_eq!(max_line_height_for_glyphs(&[]), None);
    }

    fn line_with_tab(text: &str) -> LaidOutLine {
        // Build a synthetic line whose glyphs have monotonic x +
        // small advances. Tab byte is at index 1.
        let bytes = text.as_bytes();
        let mut glyphs = Vec::new();
        let mut pen = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let adv = if b == b'\t' { 640 } else { 320 }; // 10 / 5 pt
            glyphs.push(PositionedGlyph {
                glyph_id: 0,
                cluster: i as u32,
                x: pen,
                y: 0,
                x_advance: adv,
                font_id: 0,
                point_size: 12.0,
                underline: false,
                strikethru: false,
            });
            pen += adv;
        }
        LaidOutLine {
            byte_range: 0..bytes.len(),
            baseline_y: 0,
            width: pen,
            ratio: 0.0,
            glyphs,
        }
    }

    #[test]
    fn apply_tab_stops_snaps_to_next_explicit_stop() {
        let text = "a\tb";
        let mut line = line_with_tab(text);
        // 'a' at x=0 (advance 320); '\t' at x=320; 'b' at x=960.
        // With a stop at 36 pt = 2304 1/64pt, the tab widens to
        // (2304 - 320) = 1984; 'b' shifts to 2304.
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 36.0,
                alignment: TabAlignment::Left,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[0].x, 0);
        assert_eq!(line.glyphs[1].x, 320);
        assert_eq!(line.glyphs[1].x_advance, 1984);
        assert_eq!(line.glyphs[2].x, 2304);
    }

    #[test]
    fn apply_tab_stops_falls_back_to_default_grid() {
        let text = "a\tb";
        let mut line = line_with_tab(text);
        apply_tab_stops(&mut line, text, &[], 36.0);
        assert_eq!(line.glyphs[2].x, 2304);
    }

    #[test]
    fn apply_tab_stops_skips_when_pen_past_all_stops() {
        let text = "abc\tx";
        let mut line = line_with_tab(text);
        let before_x = line.glyphs[4].x;
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 1.0,
                alignment: TabAlignment::Left,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[4].x, before_x);
    }

    #[test]
    fn right_align_pulls_segment_right_edge_to_stop() {
        // "a\tbc" — 'a' at 0..320, '\t' at 320..960, 'b' 960..1280,
        // 'c' 1280..1600. Right-align stop at 36 pt = 2304 1/64pt.
        // Segment after tab is "bc" (2 glyphs * 320 = 640 wide), so
        // the segment should start at 2304 - 640 = 1664; tab takes
        // (1664 - 320) = 1344 1/64pt of advance.
        let text = "a\tbc";
        let mut line = line_with_tab(text);
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 36.0,
                alignment: TabAlignment::Right,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[1].x_advance, 1344);
        assert_eq!(line.glyphs[2].x, 1664, "'b' should start at 1664");
        assert_eq!(line.glyphs[3].x, 1984, "'c' should start at 1984");
        // Right edge of last glyph at the stop.
        assert_eq!(line.glyphs[3].x + line.glyphs[3].x_advance, 2304);
    }

    #[test]
    fn center_align_centres_segment_on_stop() {
        let text = "a\tbc";
        let mut line = line_with_tab(text);
        // Center stop at 36 pt = 2304; segment is 640 wide; centre
        // at 2304 means segment starts at 2304 - 320 = 1984.
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 36.0,
                alignment: TabAlignment::Center,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[2].x, 1984);
        // Last glyph right edge at 1984 + 640 = 2624.
        assert_eq!(line.glyphs[3].x + line.glyphs[3].x_advance, 2624);
    }

    #[test]
    fn right_align_falls_back_when_segment_overflows() {
        // Stop at 8 pt = 512 (just past tab's natural x of 320),
        // segment width 640 → Right would want segment to start at
        // -128. Falls through to Left, but Left would also shrink
        // the tab below its natural 640 advance — so the tab keeps
        // its natural width and no snap happens.
        let text = "a\tbc";
        let mut line = line_with_tab(text);
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 8.0,
                alignment: TabAlignment::Right,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[1].x_advance, 640, "tab keeps natural width");
        assert_eq!(line.glyphs[2].x, 960, "'b' unchanged at natural position");
    }

    #[test]
    fn decimal_align_snaps_dot_onto_stop() {
        // "a\t1.5" — 'a' 0..320, '\t' 320..960, '1' 960..1280,
        // '.' 1280..1600, '5' 1600..1920. Decimal stop at
        // 36 pt = 2304: '.' should land at 2304.
        // segment_start_x = 960; '.' is at 1280 → offset 320.
        // target_segment_left = 2304 - 320 = 1984.
        // tab advance widens to 1984 - 320 = 1664.
        let text = "a\t1.5";
        let mut line = line_with_tab(text);
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 36.0,
                alignment: TabAlignment::Decimal,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[1].x_advance, 1664);
        assert_eq!(line.glyphs[2].x, 1984, "'1' starts at 1984");
        assert_eq!(line.glyphs[3].x, 2304, "'.' lands at the stop");
        assert_eq!(line.glyphs[4].x, 2624, "'5' starts after the dot");
    }

    #[test]
    fn decimal_align_falls_back_to_left_when_char_missing() {
        // No '.' in segment — should fall through to Left.
        let text = "a\tbc";
        let mut line = line_with_tab(text);
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 36.0,
                alignment: TabAlignment::Decimal,
                alignment_character: '.',
                leader: None,
            }],
            0.0,
        );
        // Same outcome as Left at 36pt: tab widens so 'b' starts at 2304.
        assert_eq!(line.glyphs[2].x, 2304);
    }

    #[test]
    fn decimal_align_with_custom_character() {
        // Use ',' as the decimal character (European convention).
        let text = "a\t1,5";
        let mut line = line_with_tab(text);
        apply_tab_stops(
            &mut line,
            text,
            &[TabStopSpec {
                position_pt: 36.0,
                alignment: TabAlignment::Decimal,
                alignment_character: ',',
                leader: None,
            }],
            0.0,
        );
        assert_eq!(line.glyphs[3].x, 2304, "',' lands at the stop");
    }
}
