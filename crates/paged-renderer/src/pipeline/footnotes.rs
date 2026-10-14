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

//! Footnote composition, space reservation, and emission — extracted
//! from pipeline/mod.rs (audit 1.6b). Net-zero behaviour.

use super::*;
use std::collections::HashMap;

use bytes::Bytes;
use paged_compose::{emit_glyph_slice, emit_line, Color, DisplayList, Paint, Stroke, TtfOutliner};
use paged_model::{Graphic, TextFrame};
use paged_scene::Document;

use crate::diagnostics::{Diagnostic, DiagnosticCode};

/// W1.7 — default footnote body point size, used when a footnote run
/// carries no resolved `PointSize` (and the paragraph/character style
/// cascade likewise declares none). Real InDesign footnotes inherit
/// the `[Footnote]` paragraph style's size; absent that we fall back
/// to 8pt, the long-standing footnote convention.
///
/// W1.8 — footnote bodies now compose through the SAME styled-run path
/// as body text (per-run size/weight/colour). This constant is only
/// the per-run *fallback*; the composition + the space-reservation
/// measurement share [`compose_footnote_paragraphs`], so they remain
/// pixel-locked regardless of per-run size shifts.
pub(super) const FOOTNOTE_POINT_SIZE: f32 = 8.0;

/// W1.7 — bail cap for the footnote space-reservation fixpoint loop.
/// Pass 0 composes with no reservation and measures; pass 1 re-composes
/// against the measured pool; a third pass catches the rare case where
/// the pass-1 reflow pushed a footnote across a frame boundary and
/// changed a pool height. Two re-composes settle every realistic
/// layout; the cap guarantees termination even if a pathological
/// document oscillates, in which case the last pass's result (an
/// overlay, never dropped text) is accepted.
pub(super) const MAX_FOOTNOTE_RESERVE_PASSES: usize = 3;

/// W1.8 — the styled, laid-out form of one footnote's body, shared by
/// the space-reservation measure and the pool emit so they agree to the
/// pixel. Each entry is one body paragraph (its reference character
/// printing the note's number) laid out into the column width through
/// the SAME multi-font `layout_runs` path as body text.
pub(super) struct ComposedFootnote {
    /// One laid-out paragraph per paragraph of the note: per piece of
    /// one, where a paragraph mark or a forced line break splits it.
    paragraphs: Vec<paged_text::LaidOutParagraph>,
    /// Per paragraph, each line's leading (pt): how far its baseline sits
    /// below the line before it. An empty paragraph has no line but one
    /// leading, the room its line takes.
    leadings: Vec<Vec<f32>>,
    /// Per paragraph, the room above its first line on top of the
    /// leading: the paragraph before's `SpaceAfter` plus its own
    /// `SpaceBefore`; none for the note's first paragraph.
    space_above: Vec<f32>,
    /// Each paragraph's shaped text (the number printed in place), which
    /// its lines' byte ranges index.
    texts: Vec<String>,
    /// Per paragraph, where in its text the note's number prints, for
    /// the break records.
    numbers: Vec<Vec<super::GeneratedRange>>,
    /// Outline bytes keyed by the `font_id` carried on each positioned
    /// glyph, so the emit pass can build a [`TtfOutliner`] per font
    /// group (a footnote that mixes faces/weights needs more than one).
    font_outline_bytes: HashMap<u32, Bytes>,
    /// Per-run resolved attrs, one Vec per source paragraph, so the
    /// emit pass can build a per-cluster paint picker (per-run
    /// `FillColor`/tint) matching the styled-run text.
    resolved_runs_per_para: Vec<Vec<paged_scene::ResolvedRunAttrs>>,
    /// Per-run SHAPED text byte lengths (marker folded onto run 0 of
    /// paragraph 0), parallel to `resolved_runs_per_para`. Drives the
    /// paint picker's band offsets.
    run_text_lens_per_para: Vec<Vec<usize>>,
}

/// W1.8 — compose one footnote's body into laid-out paragraphs through
/// the styled-run path. Each source run resolves its own
/// `PointSize` / `FontStyle` (bold/italic via the `wght` axis) /
/// `FillColor` / tracking / baseline-shift exactly like body text, so a
/// footnote with mixed styling renders faithfully instead of flattening
/// to a single face + size.
///
/// Each paragraph is set as body text sets one: its alignment, indents,
/// leading, tab stops, word and letter spacing, glyph scaling and
/// hyphenation, and a paragraph mark (`<Br/>`) in its range starts a
/// paragraph of its own. Its `SpaceBefore` and `SpaceAfter` count only
/// between the note's own paragraphs: InDesign sets two one-paragraph
/// notes that both ask for 12 pt before and 9 pt after one leading
/// apart, and does not move the area's top for the first's space
/// before, but sets the two paragraphs of one such note 12 + 9 pt more
/// than a leading apart (InDesign 20.0.1,
/// `tools/indesign-export/footnote-paragraphs-probe`).
///
/// `column_width_pt` is the host frame's content width. Returns `None`
/// when no run resolves to any font (nothing to shape) — the caller skips
/// it.
pub(super) fn compose_footnote_paragraphs(
    fn_: &EmittedFootnote,
    document: &Document,
    font_table: &FontTable,
    hyphenators: &paged_text::Hyphenators,
    binding: BindingEdge,
    column_width_pt: f32,
    default_size: f32,
) -> Option<ComposedFootnote> {
    if column_width_pt <= 0.0 {
        return None;
    }
    // The note opens with the number its reference prints.
    // The note prints its number where its text holds its reference
    // character — InDesign's `<?ACE 4?>`, the separator following it as
    // text (asked of InDesign 20.0.1, `footnote-references`). A note
    // whose text holds none prints no number: InDesign's PDF of the
    // `footnotes` fixture, whose notes carry no `ACE 4`, shows none.
    let label = document.designmap.footnote_options.label(fn_.number);
    let mut paragraphs: Vec<paged_text::LaidOutParagraph> = Vec::new();
    let mut leadings: Vec<Vec<f32>> = Vec::new();
    let mut space_above: Vec<f32> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut numbers = Vec::new();
    let mut font_outline_bytes: HashMap<u32, Bytes> = HashMap::new();
    let mut resolved_runs_per_para: Vec<Vec<paged_scene::ResolvedRunAttrs>> = Vec::new();
    let mut run_text_lens_per_para: Vec<Vec<usize>> = Vec::new();
    // The paragraph before's `SpaceAfter`, counted when another follows.
    let mut space_after_pt = 0.0f32;
    let to_64 = |pt: f32| (pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;

    for source in &fn_.paragraphs {
        // A footnote paragraph takes its nested styles as a body one does,
        // over the whole paragraph before it is split.
        let source = &*super::with_nested_styles(document, source);
        for segment in super::geom::split_paragraph_into_segments(source) {
            let para = &segment.paragraph;
            let resolved_paragraph = SegmentState {
                opens_paragraph: segment.opens_paragraph,
                closes_paragraph: segment.closes_paragraph,
                ..SegmentState::default()
            }
            .mask(document.resolved_paragraph_attrs(para));
            // Resolve every run's attrs against the footnote paragraph's
            // own style cascade — the footnote body parses into the same
            // Paragraph/CharacterRun shape as a top-level story, so the
            // standard resolver applies directly.
            let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = para
                .runs
                .iter()
                .map(|r| document.resolved_run_attrs(para, r))
                .collect();
            let resolved_fonts = match font_table.resolve_paragraph_bytes(&resolved_runs) {
                Some(b) => b,
                None => continue,
            };
            let (bytes_pool, substituted_flags): (Vec<Bytes>, Vec<bool>) =
                resolved_fonts.into_iter().unzip();
            let wghts: Vec<f32> = resolved_runs
                .iter()
                .map(|r| wght_for_font_style(r.font_style.as_deref()))
                .collect();

            // Build one shaping Face per (bytes, wght). Built on the fly
            // here (rather than via the per-render face cache) — footnote
            // pools are small, so the extra zero-copy Face construction is
            // negligible and keeps this path independent of the cache's
            // harvest pass (which never sees footnote stories).
            let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
            let mut owned_faces: Vec<Option<paged_text::Face>> =
                Vec::with_capacity(bytes_pool.len());
            for (i, b) in bytes_pool.iter().enumerate() {
                let face = paged_text::Face::from_slice(b.as_ref(), 0).map(|mut rf| {
                    let has_wght = ttf_parser::Face::parse(b.as_ref(), 0)
                        .ok()
                        .map(|of| of.variation_axes().into_iter().any(|a| a.tag == wght_tag))
                        .unwrap_or(false);
                    if has_wght {
                        rf.set_variations(&[paged_text::Variation {
                            tag: wght_tag,
                            value: wghts[i],
                        }]);
                    }
                    rf
                });
                owned_faces.push(face);
            }
            if owned_faces.iter().any(|f| f.is_none()) {
                continue;
            }

            // font_id mixes in the wght so the glyph-outline cache doesn't
            // conflate two weights of one variable font.
            let font_ids: Vec<u32> = bytes_pool
                .iter()
                .zip(wghts.iter())
                .map(|(b, w)| font_id(b) ^ w.to_bits())
                .collect();
            for (fid, b) in font_ids.iter().zip(bytes_pool.iter()) {
                font_outline_bytes.entry(*fid).or_insert_with(|| b.clone());
            }

            // Owned shaped texts, the number in its run's formatting. Owned
            // so the `&str` views in `StyledRun` outlive the layout call.
            let mut number_spans = Vec::new();
            let mut at = 0usize;
            let run_texts: Vec<String> = para
                .runs
                .iter()
                .map(|run| {
                    let mut text = String::with_capacity(run.text.len());
                    for ch in run.text.chars() {
                        if ch == paged_model::FOOTNOTE_REFERENCE_MARKER {
                            let b = at + text.len();
                            if !label.is_empty() {
                                number_spans.push((
                                    b..b + label.len(),
                                    super::GeneratedKind::FootnoteNumber,
                                    None,
                                ));
                            }
                            text.push_str(&label);
                        } else {
                            text.push(ch);
                        }
                    }
                    at += text.len();
                    text
                })
                .collect();
            let run_text_lens: Vec<usize> = run_texts.iter().map(|t| t.len()).collect();
            let text = run_texts.concat();

            let glyph_scale = desired_glyph_scale(&resolved_paragraph);
            let styled_runs: Vec<paged_text::StyledRun> = para
                .runs
                .iter()
                .enumerate()
                .map(|(i, _run)| {
                    let base_size = resolved_runs[i].point_size.unwrap_or(default_size);
                    let (point_size, baseline_shift_pt) = position_adjusted_metrics(
                        base_size,
                        resolved_runs[i].baseline_shift,
                        resolved_runs[i].position.as_deref(),
                    );
                    paged_text::StyledRun {
                        text: run_texts[i].as_str(),
                        face: owned_faces[i].as_ref().unwrap(),
                        point_size,
                        tracking: resolved_runs[i].tracking,
                        font_id: font_ids[i],
                        underline: resolved_runs[i].underline.unwrap_or(false),
                        strikethru: resolved_runs[i].strikethru.unwrap_or(false),
                        substituted: substituted_flags[i],
                        baseline_shift_pt,
                        horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0)
                            * glyph_scale,
                        vertical_scale_pct: resolved_runs[i].vertical_scale.unwrap_or(100.0),
                        skew_deg: resolved_runs[i].skew.unwrap_or(0.0),
                        fallback_faces: &[],
                        shaping_features: shaping_features_from(
                            resolved_runs[i].ligatures_on,
                            resolved_runs[i].kerning_method.as_deref(),
                            &resolved_runs[i].otf,
                            resolved_runs[i].capitalization.as_deref(),
                            resolved_runs[i].position.as_deref(),
                        ),
                    }
                })
                .collect();
            if styled_runs.is_empty() {
                continue;
            }

            // The paragraph's settings, as body text takes them. Indents
            // narrow the measure, a first-line indent line 0's; the lines
            // move right by them after layout.
            let paragraph_size = styled_runs[0].point_size;
            let left_indent_pt = resolved_paragraph.left_indent.unwrap_or(0.0).max(0.0);
            let right_indent_pt = resolved_paragraph.right_indent.unwrap_or(0.0).max(0.0);
            let first_line_indent_pt = resolved_paragraph.first_line_indent.unwrap_or(0.0);
            let measure_pt = (column_width_pt - left_indent_pt - right_indent_pt).max(1.0);
            let mut lopts = paged_text::LayoutOptions::new(measure_pt, paragraph_size);
            (lopts.alignment, lopts.last_line_alignment) =
                map_justification(resolved_paragraph.justification, binding);
            lopts.justify_last_line = !segment.closes_paragraph;
            // The run's language names the dictionary, as in body text
            // (`StoryEmitter::hyphenator_for`).
            let language = resolved_runs
                .iter()
                .find_map(|r| r.applied_language.as_deref())
                .or(resolved_paragraph.applied_language.as_deref());
            apply_paragraph_compose_options(
                &mut lopts,
                hyphenators.resolve(language),
                &resolved_paragraph,
            );
            let justified = lopts.alignment == paged_text::Alignment::Justify;
            lopts.compose.fit_tolerance = fit_tolerance_64(paragraph_size, justified);
            if !justified {
                lopts.compose.minimum_raggedness = true;
                lopts.compose.overrun_size = Some(paragraph_size);
            }
            apply_leadings(&mut lopts, &resolved_runs, &styled_runs, 0);
            let first_64 = to_64(first_line_indent_pt);
            if first_64 != 0 {
                let floor_64 = paged_text::shape::ADVANCE_PRECISION as i32;
                let w = lopts.compose.column_width;
                lopts.compose.column_widths = Some(vec![(w - first_64).max(floor_64), w]);
            }
            let left_64 = to_64(left_indent_pt);
            let tab_stops =
                paragraph_tab_stops(&resolved_paragraph, left_indent_pt, first_line_indent_pt);
            // Tabs are set while breaking, so a line is measured at the
            // width its tabs take; a flush-right or centred line then
            // moves only its text after the last tab (`apply_alignment`).
            let has_tab = text.contains('\t');
            if has_tab {
                lopts.tabs = Some(paged_text::layout::TabLayout {
                    stops: tab_stops.clone(),
                    default_stop_pt: 36.0,
                    line_starts_64: vec![left_64 + first_64, left_64],
                });
            }
            let mut laid = paged_text::cache::layout_runs_cached(&styled_runs, &lopts);
            for (i, line) in laid.lines.iter_mut().enumerate() {
                let dx = left_64 + if i == 0 { first_64 } else { 0 };
                if dx != 0 {
                    for g in &mut line.glyphs {
                        g.x += dx;
                    }
                }
            }
            if has_tab {
                let leader_ctx = tab_stops
                    .iter()
                    .any(|t| t.leader.is_some())
                    .then(|| paged_text::layout::LeaderContext::new(&styled_runs));
                for line in laid.lines.iter_mut() {
                    paged_text::layout::apply_tab_stops_with_leaders(
                        line,
                        &text,
                        &tab_stops,
                        36.0,
                        leader_ctx.as_ref(),
                    );
                }
            }
            let line_leadings: Vec<f32> = if laid.lines.is_empty() {
                // An empty paragraph still takes its line.
                vec![lopts
                    .leading_override_exact
                    .map_or(paragraph_size * 1.2, |l| {
                        (l / paged_text::shape::ADVANCE_PRECISION as f64) as f32
                    })]
            } else {
                laid.lines
                    .iter()
                    .map(|line| {
                        paged_text::layout::line_leading_exact(&line.glyphs, &lopts)
                            .map_or(paragraph_size * 1.2, |l| {
                                (l / paged_text::shape::ADVANCE_PRECISION as f64) as f32
                            })
                    })
                    .collect()
            };
            // `styled_runs` (which borrows `resolved_runs`, `owned_faces`,
            // `run_texts`) is no longer needed after layout; drop it so
            // `resolved_runs` can move into the returned struct.
            drop(styled_runs);
            space_above.push(if paragraphs.is_empty() {
                0.0
            } else {
                space_after_pt + resolved_paragraph.space_before.unwrap_or(0.0).max(0.0)
            });
            space_after_pt = resolved_paragraph.space_after.unwrap_or(0.0).max(0.0);
            texts.push(text);
            numbers.push(number_spans);
            paragraphs.push(laid);
            leadings.push(line_leadings);
            resolved_runs_per_para.push(resolved_runs);
            run_text_lens_per_para.push(run_text_lens);
        }
    }

    if paragraphs.is_empty() {
        return None;
    }
    Some(ComposedFootnote {
        paragraphs,
        leadings,
        space_above,
        texts,
        numbers,
        font_outline_bytes,
        resolved_runs_per_para,
        run_text_lens_per_para,
    })
}

/// The baselines of a frame's footnote area, top to bottom, measured
/// from the area's top: `(note, paragraph, line, baseline)`. Asked of
/// InDesign 20.0.1 (`tools/indesign-export/footnote-pool-probe.jsx` over
/// the `footnotes` fixture, and `footnote-paragraphs-probe`):
///
/// * a note's first line sits its first-baseline offset below the
///   previous note's last baseline plus `SpaceBetween` (the area's top for
///   the first note). The offset is InDesign's default, the line's
///   leading; `CAP_HEIGHT` and the other choices are not read yet;
/// * every further line sits its own leading below the one before, a
///   paragraph's first line its space above too;
/// * the separator rule takes no room: neither its offset nor its weight
///   moves a line.
///
/// An empty paragraph's line holds no glyph but has its baseline.
fn footnote_area_baselines(
    composed: &[ComposedFootnote],
    metrics: &FootnoteMetrics,
) -> Vec<(usize, usize, usize, f32)> {
    let mut out = Vec::new();
    let mut y = 0.0f32;
    for (fi, c) in composed.iter().enumerate() {
        if fi > 0 {
            y += metrics.space_between_pt;
        }
        for (p_idx, leadings) in c.leadings.iter().enumerate() {
            y += c.space_above[p_idx];
            for (l_idx, leading) in leadings.iter().enumerate() {
                y += leading;
                out.push((fi, p_idx, l_idx, y));
            }
        }
    }
    out
}

/// W1.8 — the height (pt) of one frame's footnote area, laid out exactly
/// as [`emit_footnote_pools`] draws it: from the area's top to its last
/// baseline, which InDesign sets on the frame's foot
/// ([`footnote_area_baselines`]). This, plus `Spacer`, is the band the
/// body text must vacate.
pub(super) fn footnote_pool_height_pt(
    group: &[&EmittedFootnote],
    document: &Document,
    font_table: &FontTable,
    hyphenators: &paged_text::Hyphenators,
    binding: BindingEdge,
    column_width_pt: f32,
    metrics: &FootnoteMetrics,
) -> f32 {
    if column_width_pt <= 0.0 {
        return 0.0;
    }
    let composed: Vec<ComposedFootnote> = group
        .iter()
        .filter_map(|fn_| {
            compose_footnote_paragraphs(
                fn_,
                document,
                font_table,
                hyphenators,
                binding,
                column_width_pt,
                metrics.default_size,
            )
        })
        .collect();
    footnote_area_baselines(&composed, metrics)
        .last()
        .map_or(0.0, |&(.., y)| y)
}

/// W1.8 — document-level footnote layout metrics, resolved once from the
/// `<FootnoteOption>` settings and shared by the measure + emit passes.
/// The spacing values come straight from the designmap; absent values
/// fall back to InDesign's defaults. (`SeparatorText` is what InDesign
/// types after the number when it MAKES a note; a note's text carries it.)
pub(super) struct FootnoteMetrics {
    /// `SpaceBetween` between consecutive footnotes, in pt.
    space_between_pt: f32,
    /// `Spacer`: minimum gap between body bottom and first footnote, pt.
    /// (Folded into the reservation so the pool sits clear of the body.)
    spacer_pt: f32,
    /// Default per-run point size fallback.
    default_size: f32,
    /// Resolved separator-rule spec (`None` when the rule is off).
    rule: Option<FootnoteRuleSpec>,
}

/// W1.8 — resolved separator-rule geometry/paint, ready to stroke.
pub(super) struct FootnoteRuleSpec {
    weight_pt: f32,
    left_indent_pt: f32,
    width_pt: f32,
    offset_pt: f32,
    paint: Paint,
}

/// W1.8 — resolve the document's `<FootnoteOption>` into the metrics the
/// pool measure + emit consume. Applies InDesign's defaults for any
/// value the designmap left unset (rule ON, ~0.5pt black rule 50% of the
/// column wide, no extra spacing).
pub(super) fn resolve_footnote_metrics(
    document: &Document,
    column_width_pt: f32,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    default_size: f32,
) -> FootnoteMetrics {
    let fo = &document.designmap.footnote_options;
    let space_between_pt = fo.space_between.unwrap_or(0.0).max(0.0);
    let spacer_pt = fo.spacer.unwrap_or(0.0).max(0.0);

    let rule = if fo.rule_on_effective() {
        // Defaults mirror InDesign's new-document footnote rule: 0.5pt
        // black, full offset 0, indent 0, length = half the column.
        let weight_pt = fo.rule_line_weight.unwrap_or(0.5).max(0.0);
        let left_indent_pt = fo.rule_left_indent.unwrap_or(0.0).max(0.0);
        let width_pt = fo
            .rule_width
            .filter(|w| *w > 0.0)
            .unwrap_or(column_width_pt * 0.5)
            .min((column_width_pt - left_indent_pt).max(0.0));
        let offset_pt = fo.rule_offset.unwrap_or(0.0);
        let base_paint = fo
            .rule_color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, palette, color_ctx))
            .unwrap_or(Paint::Solid(Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }));
        let paint = apply_fill_tint_ctx(
            base_paint,
            object_tint(palette, fo.rule_color.as_deref(), fo.rule_tint),
            &color_ctx,
        );
        Some(FootnoteRuleSpec {
            weight_pt,
            left_indent_pt,
            width_pt,
            offset_pt,
            paint,
        })
    } else {
        None
    };

    FootnoteMetrics {
        space_between_pt,
        spacer_pt,
        default_size,
        rule,
    }
}

/// W1.7/W1.8 — per (page, host-frame) footnote pool heights in pt.
/// Keyed by the same quantised `host_frame_rect_pt` tuple the emit
/// groups by, so the reservation pass can map a pool back to the chain
/// frame that hosts it. Returns an empty map when the document carries
/// no font bytes (footnotes can't be measured or drawn without a face).
/// Only `page_indices` are measured; a pool depends on its own page alone.
#[allow(clippy::too_many_arguments)]
pub(super) fn measure_footnote_pools(
    pages: &[BuiltPage],
    page_indices: &[usize],
    options: &PipelineOptions,
    document: &Document,
    font_table: &FontTable,
    hyphenators: &paged_text::Hyphenators,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
) -> std::collections::HashMap<(usize, i32, i32, i32, i32), f32> {
    let mut out = std::collections::HashMap::new();
    if options.font.is_none() && !font_table.has_any_face() {
        return out;
    }
    for &page_idx in page_indices {
        let Some(page) = pages.get(page_idx) else {
            continue;
        };
        if page.footnotes.is_empty() {
            continue;
        }
        let mut by_frame: std::collections::BTreeMap<(i32, i32, i32, i32), Vec<&EmittedFootnote>> =
            Default::default();
        for fn_ in &page.footnotes {
            by_frame
                .entry(footnote_frame_key(&fn_.host_frame_rect_pt))
                .or_default()
                .push(fn_);
        }
        for (key, group) in by_frame {
            let column_width_pt = group[0].host_frame_rect_pt.w;
            let metrics = resolve_footnote_metrics(
                document,
                column_width_pt,
                palette,
                color_ctx,
                FOOTNOTE_POINT_SIZE,
            );
            let h = footnote_pool_height_pt(
                &group,
                document,
                font_table,
                hyphenators,
                page.binding_edge(),
                column_width_pt,
                &metrics,
            ) + metrics.spacer_pt;
            if h > 0.0 {
                out.insert((page_idx, key.0, key.1, key.2, key.3), h);
            }
        }
    }
    out
}

/// Quantised grouping key for a host frame's content rect (1/64 pt),
/// shared by the pool emit and the reservation measure so they agree
/// on which footnotes belong to which frame.
pub(super) fn footnote_frame_key(rect: &paged_compose::Rect) -> (i32, i32, i32, i32) {
    (
        (rect.x * 64.0) as i32,
        (rect.y * 64.0) as i32,
        (rect.w * 64.0) as i32,
        (rect.h * 64.0) as i32,
    )
}

/// W1.7 — the page index and quantised content-rect key a chain frame
/// would capture footnotes under, computed with the EXACT formula
/// [`emit_paragraph_into_chain`] uses (`frame_spread_top_left` minus the
/// page origin, plus L/T insets; width/height minus L+R / T+B insets).
/// Lets the reservation pass map a measured pool back to the chain
/// frame whose text area must shrink. Returns `None` for a frame whose
/// `self_id` doesn't resolve to a page.
pub(super) fn footnote_host_key_for_frame(
    frame: &TextFrame,
    page_idx: usize,
    pages: &[BuiltPage],
) -> (usize, i32, i32, i32, i32) {
    let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
    let (ox, oy) = pages[page_idx].spread_origin;
    let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
    let frame_w = frame.bounds.width();
    let frame_h = frame.bounds.height();
    let rect = paged_compose::Rect {
        x: sx - ox + insets[1],
        y: sy - oy + insets[0],
        w: (frame_w - insets[1] - insets[3]).max(0.0),
        h: (frame_h - insets[0] - insets[2]).max(0.0),
    };
    let k = footnote_frame_key(&rect);
    (page_idx, k.0, k.1, k.2, k.3)
}

/// W1.7 — per-page display-list + capture lengths plus stats, snapshot
/// before a story's body emit so the footnote space-reservation loop
/// can roll the page back and re-emit with a reduced text area. The
/// re-emit truncates `paths` (cache-aware, [`PathBuffer::truncate_to`]),
/// `commands`, the gradient/image pools, `story_layout`, and
/// `footnotes` to these lengths and restores `stats` — returning the
/// page to exactly its pre-story state. (The body emit never appends to
/// `page.diagnostics`; footnote-overflow diagnostics come from the
/// later pool pass, so they need no rollback here.)
#[derive(Clone, Copy)]
pub(super) struct BodyStoryPageReset {
    paths: usize,
    commands: usize,
    gradients: usize,
    radial_gradients: usize,
    images: usize,
    story_layout: usize,
    footnotes: usize,
    cell_rects: usize,
    stats: PipelineStats,
}

pub(super) fn snapshot_body_story_reset(pages: &[BuiltPage]) -> Vec<BodyStoryPageReset> {
    pages
        .iter()
        .map(|p| BodyStoryPageReset {
            paths: p.list.paths.len(),
            commands: p.list.commands.len(),
            gradients: p.list.gradients.len(),
            radial_gradients: p.list.radial_gradients.len(),
            images: p.list.images.len(),
            story_layout: p.story_layout.len(),
            footnotes: p.footnotes.len(),
            cell_rects: p.cell_rects.len(),
            stats: p.stats,
        })
        .collect()
}

pub(super) fn rollback_body_story(pages: &mut [BuiltPage], snap: &[BodyStoryPageReset]) {
    for (page, s) in pages.iter_mut().zip(snap.iter()) {
        page.list.paths.truncate_to(s.paths);
        page.list.commands.truncate(s.commands);
        page.list.gradients.truncate(s.gradients);
        page.list.radial_gradients.truncate(s.radial_gradients);
        page.list.images.truncate(s.images);
        page.story_layout.truncate(s.story_layout);
        page.footnotes.truncate(s.footnotes);
        page.cell_rects.truncate(s.cell_rects);
        // The glyph-run side channel indexes commands ABSOLUTELY, so a
        // rolled-back pass leaves entries pointing past the end of the
        // list — harmless while nothing renumbered commands, a real
        // mis-address once W2's relocation remaps them.
        if let Some(table) = page.list.glyph_runs.as_mut() {
            table
                .entries
                .retain(|e| (e.command_index as usize) < s.commands);
        }
        page.stats = s.stats;
    }
}

/// W1.7/W1.8 — lay out each page's captured footnote pool at the bottom
/// of its host frame: separator rule, then the footnote bodies composed
/// through the styled-run path, stacked so the last body's bottom sits
/// at the frame's content bottom.
///
/// DEFERRED (2026-06-07, W1.8) — cross-frame footnote SPLITTING.
/// InDesign, when the last footnote on a column doesn't fit, splits that
/// footnote: it keeps the reference line plus as many footnote lines as
/// fit in the current column, and continues the remaining footnote lines
/// in the next column/frame's pool (no repeated number).
///
/// The current model can't express this. First, the pool is laid out in
/// THIS post-pass, AFTER the whole story's body emit has finished and the
/// frame chain is fixed, so there is no live feedback from "footnote line
/// N overflows" back into the body fill to push the reference line
/// forward. Second, the reservation fixpoint (`measure_footnote_pools` →
/// `with_footnote_reservation`) reserves a whole-pool height per frame; it
/// has no notion of a partial footnote or a per-line continuation cursor
/// carrying a remainder to the next frame. Third, `EmittedFootnote` is
/// captured per-page at the host paragraph's starting frame; a split would
/// need ONE footnote to contribute to two pages' pools — a (footnote,
/// line_range, page) fan-out the capture vec doesn't model.
///
/// Design sketch for a future pass: change the reservation loop to reserve
/// only what fits, have the pool emit return an overflow remainder per
/// frame (the unplaced laid-out lines plus a "continued" flag), and thread
/// that remainder into the NEXT chain frame's pool as a leading
/// continuation block before its own captured footnotes. That requires the
/// pool pass to run inside the frame-chain walk (or at least be
/// chain-aware) rather than as a flat per-page post-pass — a multi-day
/// restructure of the StoryEmitter-to-pool boundary. Until then a too-tall
/// footnote overruns the body (overlay) and we fire
/// `DiagnosticCode::FootnoteOverflow` (below) so it is never silent.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_footnote_pools(
    pages: &mut [BuiltPage],
    skip: &[bool],
    font_table: &FontTable,
    hyphenators: &paged_text::Hyphenators,
    options: &PipelineOptions,
    document: &Document,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    breaks: &mut Vec<super::BreakRecord>,
) {
    // The footnote pool needs at least one resolvable face. The
    // styled-run composer resolves per-run bytes through the FontTable
    // (which already folds in `options.font` as its fallback, and holds
    // what the asset resolver supplied), so the only hard requirement is
    // that *some* font is available. (Checking `options.font` alone kept
    // every resolver-fed build — the fidelity gates' inspector among
    // them — from drawing a single footnote.)
    if options.font.is_none() && !font_table.has_any_face() {
        return;
    }
    let default_paint = Paint::Solid(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    for (page_idx, page) in pages.iter_mut().enumerate() {
        // An adopted page already carries its pool (ADR 027 plan step 7).
        if skip.get(page_idx).copied().unwrap_or(false) {
            continue;
        }
        if page.footnotes.is_empty() {
            continue;
        }
        // Sort by number (already inserted in order, but defensive).
        let mut pool: Vec<EmittedFootnote> = page.footnotes.clone();
        pool.sort_by_key(|f| f.number);
        // Group by host_frame_rect_pt — render each frame's pool
        // independently. Real-world: most pages have one frame
        // hosting all the footnotes; this loop handles the rare
        // multi-host case too.
        let mut by_frame: std::collections::BTreeMap<(i32, i32, i32, i32), Vec<&EmittedFootnote>> =
            Default::default();
        for fn_ in &pool {
            by_frame
                .entry(footnote_frame_key(&fn_.host_frame_rect_pt))
                .or_default()
                .push(fn_);
        }
        let binding = page.binding_edge();
        for (_key, group) in by_frame {
            let rect = group[0].host_frame_rect_pt;
            let column_width_pt = rect.w;
            if column_width_pt <= 0.0 {
                continue;
            }
            let metrics = resolve_footnote_metrics(
                document,
                column_width_pt,
                palette,
                color_ctx,
                FOOTNOTE_POINT_SIZE,
            );
            // Compose each footnote through the styled-run path (per-run
            // size / weight / colour). Skipping any that resolve to no
            // font, exactly as the measure pass does.
            let composed: Vec<ComposedFootnote> = group
                .iter()
                .filter_map(|fn_| {
                    compose_footnote_paragraphs(
                        fn_,
                        document,
                        font_table,
                        hyphenators,
                        binding,
                        column_width_pt,
                        metrics.default_size,
                    )
                })
                .collect();
            if composed.is_empty() {
                continue;
            }
            // The area sits on the frame's foot: its last baseline on the
            // content bottom. When it is taller than the room under its
            // references, InDesign keeps as many of its lines as fit and
            // oversets the rest: the area's top may come no higher than
            // the lowest reference line's baseline plus `Spacer` (asked of
            // InDesign 20.0.1, `footnote-pool-probe`: `Spacer` 0, 4 and 12
            // keep three, two and two lines of the `footnotes` fixture's
            // too-tall third note, its last line on the frame's foot).
            let baselines = footnote_area_baselines(&composed, &metrics);
            let frame_bottom_pt = rect.y + rect.h;
            let floor_pt = group
                .iter()
                .map(|f| f.ref_baseline_pt)
                .fold(f32::MIN, f32::max)
                + metrics.spacer_pt;
            let kept = (1..=baselines.len())
                .rev()
                .find(|&m| frame_bottom_pt - baselines[m - 1].3 >= floor_pt - 0.01)
                .unwrap_or(1);
            let area_top_pt = frame_bottom_pt - baselines[kept - 1].3;

            // A cut area loses lines: report it so callers know the render
            // is lossy. Continuing them in the next frame is the
            // documented deferral (see the note on `EmittedFootnote`).
            if kept < baselines.len() {
                page.diagnostics.push(
                    Diagnostic::new(
                        DiagnosticCode::FootnoteOverflow,
                        "footnote area is taller than the room below its references; its last \
                         lines are overset (cross-frame footnote splitting is not yet implemented)",
                    )
                    .with_page(page_idx)
                    .with_story(group[0].host_story_id.clone()),
                );
            }

            // The separator rule, centred its offset above the area's top
            // (InDesign 20.0.1: offset 2, weight 1 strokes 236.4–237.4 pt
            // over an area starting at 238.9 pt).
            if let Some(spec) = metrics.rule.as_ref() {
                let rule_y_pt = area_top_pt - spec.offset_pt;
                let x0 = rect.x + spec.left_indent_pt;
                let x1 = x0 + spec.width_pt;
                if spec.width_pt > 0.0 && spec.weight_pt > 0.0 {
                    emit_line(
                        x0,
                        rule_y_pt,
                        x1,
                        rule_y_pt,
                        Stroke::new(spec.weight_pt),
                        spec.paint,
                        &mut page.list,
                    );
                }
            }

            // Each kept line at its baseline; the rest are overset.
            let kept = &baselines[..kept];
            for (fi, c) in composed.iter().enumerate() {
                // Build the per-font outliners for this footnote once.
                let ttf_faces: HashMap<u32, ttf_parser::Face> = c
                    .font_outline_bytes
                    .iter()
                    .filter_map(|(fid, bytes)| {
                        ttf_parser::Face::parse(bytes.as_ref(), 0)
                            .ok()
                            .map(|f| (*fid, f))
                    })
                    .collect();
                for (p_idx, laid) in c.paragraphs.iter().enumerate() {
                    let line_baselines: Vec<f32> = kept
                        .iter()
                        .filter(|&&(f, p, ..)| f == fi && p == p_idx)
                        .map(|&(.., y)| area_top_pt + y)
                        .collect();
                    if line_baselines.is_empty() {
                        continue;
                    }
                    let picker = build_footnote_paint_picker(
                        &c.resolved_runs_per_para[p_idx],
                        &c.run_text_lens_per_para[p_idx],
                        palette,
                        color_ctx,
                        default_paint,
                    );
                    // A note's lines are lines of the page too: they
                    // join the break records when those are collected.
                    let host = group[fi].host_story_id.as_str();
                    if options.collect_breaks
                        && options
                            .break_page_range
                            .as_ref()
                            .map_or(true, |r| r.contains(&(page_idx as u32)))
                        && options
                            .break_story_filter
                            .as_deref()
                            .map_or(true, |want| want == host)
                    {
                        let text = &c.texts[p_idx];
                        for (line_idx, (line, &baseline)) in
                            laid.lines.iter().zip(&line_baselines).enumerate()
                        {
                            let range = line.byte_range.start.min(text.len())
                                ..line.byte_range.end.min(text.len());
                            breaks.push(super::BreakRecord {
                                story_id: format!("{host}#footnote{}", group[fi].number),
                                paragraph_idx: p_idx as u32,
                                line_idx: line_idx as u32,
                                page_idx: page_idx as u32,
                                frame_idx: 0,
                                first_byte: range.start as u32,
                                last_byte: range.end as u32,
                                baseline_y_pt: baseline,
                                width_pt: line.width as f32 / paged_text::shape::ADVANCE_PRECISION,
                                source_text: text.get(range.clone()).unwrap_or("").to_string(),
                                generated: super::GeneratedSpan::clip_to_line(
                                    &c.numbers[p_idx],
                                    range,
                                ),
                                text_path: None,
                            });
                        }
                    }
                    emit_footnote_paragraph(
                        laid,
                        &ttf_faces,
                        font_table,
                        &picker,
                        rect.x,
                        &line_baselines,
                        &mut page.list,
                    );
                }
            }
        }
    }
}

/// W1.8 — emit one laid-out footnote paragraph's glyphs, grouping by
/// `font_id` so each face/weight uses its own outliner (a footnote that
/// mixes bold + regular needs more than one). `paint_for` returns the
/// per-cluster fill. Each line is drawn on its baseline in
/// `baselines_pt` (page space, from [`footnote_area_baselines`]), its
/// glyphs starting at `left_pt`; a line with no baseline there is
/// overset and not drawn.
pub(super) fn emit_footnote_paragraph(
    laid: &paged_text::LaidOutParagraph,
    ttf_faces: &HashMap<u32, ttf_parser::Face>,
    // Only to unmix a glyph's outline id back into a `FontRef` — see
    // `FontTable::font_ref`. Without it a footnote's text is the one
    // text on the page the PDF exporter cannot embed.
    font_table: &FontTable,
    picker: &RunPaintPicker,
    left_pt: f32,
    baselines_pt: &[f32],
    list: &mut DisplayList,
) {
    for (line, &baseline_pt) in laid.lines.iter().zip(baselines_pt) {
        // The glyphs are positioned from the paragraph's top; move that
        // top so this line's baseline lands where the area puts it.
        let frame_origin_pt = (
            left_pt,
            baseline_pt - line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION,
        );
        let mut start = 0;
        while start < line.glyphs.len() {
            // Group by (font_id, point_size): `emit_glyph_slice` applies
            // ONE point-size scale to the whole slice, so a footnote line
            // that mixes sizes under one face (e.g. an 8pt body with a
            // 10pt inline phrase) must split at every size change or the
            // larger run would render — and be recorded in the glyph-run
            // side channel — at the first glyph's size. Body text never
            // hit this because its composer assigns size via the run's
            // own slice; footnote runs share one fallback font_id.
            let fid = line.glyphs[start].font_id;
            let size = line.glyphs[start].point_size;
            let mut end = start + 1;
            while end < line.glyphs.len()
                && line.glyphs[end].font_id == fid
                && (line.glyphs[end].point_size - size).abs() < 0.01
            {
                end += 1;
            }
            if let Some(face) = ttf_faces.get(&fid) {
                let outliner = TtfOutliner::new(face);
                emit_glyph_slice(
                    &line.glyphs[start..end],
                    font_table.font_ref(fid),
                    size,
                    |cluster| picker.pick(cluster),
                    frame_origin_pt,
                    &outliner,
                    list,
                );
            }
            start = end;
        }
    }
}
