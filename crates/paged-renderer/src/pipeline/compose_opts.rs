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

//! apply_paragraph_compose_options — per-paragraph compose-option resolution. Extracted from pipeline/mod.rs (1.6b).

/// `DesiredGlyphScaling` as a factor on every glyph of the paragraph.
/// InDesign sets it as a horizontal scale — advances and outlines, ragged
/// text too — multiplied with a run's own `HorizontalScale`, and letter
/// and word spacing then take their percentages of the scaled space
/// (InDesign 20.0.1, tools/indesign-export/spelling-and-spacing-probe:
/// at 105 % every glyph of a 20 pt Open Sans line advances 1.050 times its
/// width, 0.840 times on a run already at 80 %). Callers fold it into each
/// run's `horizontal_scale_pct`; the Min..Max band stays a breaker budget
/// (below).
pub(super) fn desired_glyph_scale(resolved: &paged_scene::ResolvedParagraphAttrs) -> f32 {
    resolved.desired_glyph_scaling.unwrap_or(100.0).max(0.0) / 100.0
}

/// The paragraph's leading, onto `lopts`. Explicit `Leading` on the
/// first run mirrors IDML semantics: every line uses it regardless of
/// the largest glyph size on the line (no override: auto leading). Runs
/// that ask for DIFFERENT leadings each keep their own, and a line takes
/// the largest among its characters (`paged_text::layout::line_leading`).
/// `styled_runs` are the runs as shaped: `head_run` of them (a list
/// marker shaped as its own run) lead `resolved_runs`, and their glyphs
/// never count (`auto_leading_from_byte`).
pub(super) fn apply_leadings(
    lopts: &mut paged_text::LayoutOptions<'_>,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    styled_runs: &[paged_text::StyledRun<'_>],
    head_run: usize,
) {
    if let Some(leading_pt) = resolved_runs.first().and_then(|r| r.leading) {
        if leading_pt > 0.0 {
            lopts.leading_override =
                Some((leading_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
            lopts.leading_override_exact =
                Some(leading_pt as f64 * paged_text::shape::ADVANCE_PRECISION as f64);
        }
    }
    let explicit = |r: &paged_scene::ResolvedRunAttrs| {
        r.leading
            .filter(|l| *l > 0.0)
            .map(|l| (l * paged_text::shape::ADVANCE_PRECISION).round() as i32)
    };
    let leadings: Vec<Option<i32>> = resolved_runs.iter().map(explicit).collect();
    if leadings.windows(2).any(|w| w[0] != w[1]) {
        let mut at = 0u32;
        let mut out = Vec::with_capacity(styled_runs.len());
        for (k, run) in styled_runs.iter().enumerate() {
            let leading = k
                .checked_sub(head_run)
                .and_then(|i| leadings.get(i).copied().flatten());
            out.push((at, leading));
            at += run.text.len() as u32;
        }
        lopts.run_leadings = out;
    }
}

/// Set `lopts`' line setting from the paragraph (ADR 153): auto leading
/// from its `AutoLeading` (per cent, InDesign's 120 when unset) with the
/// default line height a line of `size` pt takes, and how a justified
/// line of one word is set.
pub(super) fn set_auto_leading(
    lopts: &mut paged_text::LayoutOptions<'_>,
    paragraph: &paged_scene::ResolvedParagraphAttrs,
    size: f32,
) {
    let ratio = paragraph
        .line_setting
        .auto_leading
        .filter(|p| *p > 0.0)
        .map_or(1.2, |p| p / 100.0);
    lopts.auto_leading = ratio;
    lopts.compose.balance_ragged =
        paragraph.line_setting.balance == Some(paged_model::BalanceRaggedLines::FullyBalanced);
    use paged_model::SingleWordJustification as W;
    lopts.single_word_alignment = match paragraph.line_setting.single_word {
        None | Some(W::FullyJustified) => paged_text::Alignment::Justify,
        Some(W::LeftAlign) => paged_text::Alignment::Left,
        Some(W::CenterAlign) => paged_text::Alignment::Center,
        Some(W::RightAlign) => paged_text::Alignment::Right,
    };
    let exact = size as f64 * ratio as f64 * paged_text::shape::ADVANCE_PRECISION as f64;
    lopts.line_height = exact.round() as i32;
    lopts.line_height_exact = Some(exact);
}

/// Text set with `NoBreak`, onto `lopts` as the byte ranges no line
/// break splits (ADR 153). `styled_runs` / `head_run` as for
/// [`apply_leadings`].
pub(super) fn apply_no_break(
    lopts: &mut paged_text::LayoutOptions<'_>,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    styled_runs: &[paged_text::StyledRun<'_>],
    head_run: usize,
) {
    let mut at = 0u32;
    for (k, run) in styled_runs.iter().enumerate() {
        let len = run.text.len() as u32;
        let held = k
            .checked_sub(head_run)
            .and_then(|i| resolved_runs.get(i))
            .and_then(|r| r.no_break)
            .unwrap_or(false);
        if held && len > 0 {
            match lopts.no_break.last_mut() {
                Some(last) if last.end == at => last.end = at + len,
                _ => lopts.no_break.push(at..at + len),
            }
        }
        at += len;
    }
}

/// Apply IDML paragraph-style attributes that drive the line breaker
/// onto a fresh `LayoutOptions`. Hyphenation defaults to *on* (IDML's
/// own default) when the cascade leaves the field unset; explicit
/// `Hyphenation="false"` disables it. Word-spacing percentages convert
/// to the composer's stretch / shrink ratios.
pub(super) fn apply_paragraph_compose_options<'a>(
    lopts: &mut paged_text::LayoutOptions<'a>,
    hyphenator: Option<&'a paged_text::Hyphenator>,
    resolved: &paged_scene::ResolvedParagraphAttrs,
) {
    // Hyphenation: IDML's default is true; only an explicit false
    // disables it. We treat None as "use the default" which lets
    // unstyled paragraphs hyphenate just like InDesign would.
    let hyphenate = resolved.hyphenation.unwrap_or(true);
    if hyphenate {
        lopts.compose.hyphenator = hyphenator;
    } else {
        lopts.compose.hyphenator = None;
    }
    // Hyphenation zone (pt → 1/64 pt). Only meaningful when a
    // hyphenator is wired; the composer ignores it otherwise. A word
    // that would start within `zone` of the right margin is kept whole
    // rather than broken (InDesign's "hyphenation zone"). `None`/0 ⇒
    // no zone restriction (hyphenate anywhere an opportunity exists).
    //
    // W1.17: the zone is a *ragged-edge* feature. Adobe: "The
    // Hyphenation Zone … applies only when you're using the Single-line
    // Composer with nonjustified text." (helpx.adobe.com/indesign/using/
    // text-composition.html — "Compose and hyphenate text".) The zone's
    // whole job is to bound how far the right edge may rag before a
    // hyphen is forced; a justified paragraph has no rag (every line is
    // flushed to the column), so the option has no meaning there and
    // InDesign ignores it. Mirror that exactly: zero the zone for
    // justified paragraphs so the composer's hyphenation penalties are
    // driven purely by geometric fit, as InDesign's justified composer
    // does. W1.3 landed the ragged-only zone gate; this closes the
    // justified case as a documented no-op rather than a behaviour.
    // A paragraph that states no zone has none: InDesign hyphenates the
    // `tints` labels' "Ma-genta" as it does at zone 0, not as at 36.
    let zone_64 = resolved
        .hyphenation_zone
        .map(|z| (z.max(0.0) * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    lopts.compose.hyphenation_zone = if lopts.alignment == paged_text::Alignment::Justify {
        0
    } else {
        zone_64
    };
    // The paragraph's own hyphenation limits. IDML's factory values
    // (2 / 2 / 5, capitalised and last words hyphenatable) stand in
    // where an attribute is absent — those were hard-coded in the
    // composer until the attributes were modelled, so a paragraph that
    // asked for four letters before the hyphen, or protected its
    // capitalised words, got the defaults anyway.
    let d = paged_text::HyphenationLimits::default();
    lopts.compose.hyphenation_limits = paged_text::HyphenationLimits {
        after_first: resolved
            .hyphenate_after_first
            .map(|n| n as usize)
            .unwrap_or(d.after_first),
        before_last: resolved
            .hyphenate_before_last
            .map(|n| n as usize)
            .unwrap_or(d.before_last),
        words_longer_than: resolved
            .hyphenate_words_longer_than
            .map(|n| n as usize)
            .unwrap_or(d.words_longer_than),
        capitalized_words: resolved
            .hyphenate_capitalized_words
            .unwrap_or(d.capitalized_words),
        last_word: resolved.hyphenate_last_word.unwrap_or(d.last_word),
        ladder_limit: resolved
            .hyphenate_ladder_limit
            .map(|n| n as usize)
            .unwrap_or(d.ladder_limit),
    };
    // `HyphenWeight` is InDesign's "Better Spacing ↔ Fewer Hyphens"
    // slider (0..=10, default 5). It trades line-fit quality against
    // hyphen count, which is exactly what Knuth–Plass's hyphen penalty
    // does — so map the slider onto the penalty: 0 (better spacing)
    // makes a hyphen nearly free, 10 (fewer hyphens) makes it costly.
    // The scale is a straight line through the composer's existing
    // default of 50 at the slider's own default of 5; calibrating it
    // against measured InDesign hyphen counts is still open.
    if let Some(w) = resolved.hyphen_weight {
        lopts.compose.hyphen_penalty = (w.min(10) as i32) * 10;
    }
    // Word spacing: IDML carries percentages on the [Min..=Desired..=Max]
    // axis relative to the natural space-glyph advance. The composer's
    // `desired_space_ratio` scales the glue's natural width;
    // `stretch_ratio` / `shrink_ratio` are still relative to the raw
    // glyph advance, so the breaker reads a Min..=Desired..=Max band
    // shifted by Desired (P-07).
    let desired = resolved.desired_word_spacing.unwrap_or(100.0).max(1.0);
    lopts.compose.desired_space_ratio = (desired / 100.0).max(0.0);
    if let Some(max) = resolved.maximum_word_spacing {
        lopts.compose.stretch_ratio = ((max - desired) / 100.0).max(0.0);
    }
    if let Some(min) = resolved.minimum_word_spacing {
        lopts.compose.shrink_ratio = ((desired - min) / 100.0).clamp(0.0, 1.0);
    }
    // Floor the stretch budget so the breaker can always find a feasible
    // line. IDML paragraphs like `MinimumWordSpacing=90 MaximumWordSpacing=100`
    // (Max == Desired) yield a zero-stretch budget which Knuth-Plass cannot
    // satisfy on wide columns, collapsing wrap to one word per line (Q-15).
    //
    // Cycle-6 Track 4 Round B: only floor the stretch if the IDML
    // didn't carry an explicit max — paragraphs that explicitly set
    // MaximumWordSpacing get exactly the budget they asked for. The
    // Q-15 fallback was protecting the case where IDML's Max == Min
    // == Desired which yields zero budget; that's still covered by
    // the unconditional floor below for paragraphs with no
    // MaximumWordSpacing attribute.
    if resolved.maximum_word_spacing.is_none() {
        lopts.compose.stretch_ratio = lopts.compose.stretch_ratio.max(0.1);
    }
    // Q-20: `Min/Desired/MaxLetterSpacing` are percentages of the space
    // width, between characters (InDesign 20.0.1: 50 adds half the font's
    // space after every character of a line but the last; Min and Max
    // change nothing in ragged text). Desired is set as real spacing by
    // `layout_runs`; the Min..Max band is folded into the per-word stretch
    // / shrink budget below, so the breaker can lean on inter-glyph space
    // when word space alone can't justify a line. Real per-glyph
    // distribution after the breaker picks breaks is queued.
    let ls_min = resolved.minimum_letter_spacing.unwrap_or(0.0);
    let ls_desired = resolved.desired_letter_spacing.unwrap_or(0.0);
    let ls_max = resolved.maximum_letter_spacing.unwrap_or(0.0);
    lopts.compose.letter_spacing_pct = ls_desired;
    // The Single-line Composer (`Composer="HL Single"`) sets one line at
    // a time (paged-text `single_line.rs`). It carries the letter-spacing
    // band itself, per character, so the approximation below — which
    // folds that band into the word spaces — stays out of its glue.
    let single_line = resolved
        .composer
        .as_ref()
        .is_some_and(paged_model::Composer::is_single_line);
    if single_line {
        // InDesign's own defaults where the cascade is silent: a 36 pt
        // hyphenation zone and three hyphens in a row (both read off
        // InDesign 20.0.1's `[No paragraph style]`).
        let d = paged_text::SingleLineOptions::default();
        let desired_ws = resolved.desired_word_spacing.unwrap_or(100.0);
        lopts.compose.single_line = Some(paged_text::SingleLineOptions {
            zone: resolved
                .hyphenation_zone
                .map(|z| (z.max(0.0) * paged_text::shape::ADVANCE_PRECISION).round() as i32)
                .unwrap_or(d.zone),
            ladder_limit: resolved
                .hyphenate_ladder_limit
                .map(|n| n as usize)
                .unwrap_or(d.ladder_limit),
            letter_shrink_ratio: ((ls_desired - ls_min) / 100.0).max(0.0),
            max_stretch_ratio: ((resolved.maximum_word_spacing.unwrap_or(133.0) - desired_ws)
                / 100.0)
                .max(0.0),
        });
    }
    if !single_line && (ls_min != 0.0 || ls_max != ls_desired) {
        // The band, read as a word space's worth: each of a word's
        // letters may take (Max - Desired) % of a space more, so a word
        // gap gains that much times the letters per word. Capped so the
        // letter band never outweighs the word-space band (the cycle-6
        // calibration: InDesign's common -5 / 25 band lands on both caps,
        // as it did when these values were misread as points).
        const LETTERS_PER_WORD: f32 = 5.0;
        let stretch_add = ((ls_max - ls_desired).max(0.0) / 100.0 * LETTERS_PER_WORD).min(0.5);
        let shrink_add = ((ls_desired - ls_min).max(0.0) / 100.0 * LETTERS_PER_WORD).min(0.25);
        lopts.compose.stretch_ratio = (lopts.compose.stretch_ratio + stretch_add).min(2.0);
        lopts.compose.shrink_ratio = (lopts.compose.shrink_ratio + shrink_add).min(0.5);
    }
    // Q-20: glyph scaling. The desired value scales every glyph
    // (`desired_glyph_scale`, folded into the runs). When `Min/Max
    // GlyphScaling` differ from it, the IDML allows the composer to scale
    // per-glyph x-advance within that band. Per-glyph distribution after
    // Knuth-Plass is the proper implementation; for now we widen the
    // stretch ratio so the breaker has the budget the IDML implies.
    let gs_desired = resolved.desired_glyph_scaling.unwrap_or(100.0);
    let gs_max = resolved.maximum_glyph_scaling.unwrap_or(gs_desired);
    let gs_min = resolved.minimum_glyph_scaling.unwrap_or(gs_desired);
    if (gs_max - gs_desired).abs() > 0.01 || (gs_desired - gs_min).abs() > 0.01 {
        let extra_stretch = ((gs_max - gs_desired) / 100.0).max(0.0);
        let extra_shrink = ((gs_desired - gs_min) / 100.0).max(0.0);
        lopts.compose.stretch_ratio = (lopts.compose.stretch_ratio + extra_stretch).min(2.0);
        lopts.compose.shrink_ratio = (lopts.compose.shrink_ratio + extra_shrink).min(0.5);
    }
    // CJK Stage 2: enable hard-kinsoku enforcement whenever the cascade
    // carries any `KinsokuType` ("WordbreakWithJustification" / "PushIn"
    // / "PushOut" / etc). The composer currently keys on presence only;
    // flavour-specific behaviour is queued under CJK Stage 4.
    lopts.compose.kinsoku_enforce = resolved.kinsoku_type.is_some();
    // Phase 7 — enable Mojikumi half-width tightening when the
    // cascade resolves a `MojikumiTable` or `MojikumiSet` reference.
    // The MVP applies a uniform "halve CJK punctuation advance"
    // rule rather than per-table per-adjacency lookups; richer
    // table-driven behaviour is queued.
    lopts.compose.mojikumi_half_width =
        resolved.mojikumi_table.is_some() || resolved.mojikumi_set.is_some();
}
