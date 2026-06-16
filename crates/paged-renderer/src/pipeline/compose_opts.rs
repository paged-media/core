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
    let zone_64 = resolved
        .hyphenation_zone
        .map(|z| (z.max(0.0) * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    lopts.compose.hyphenation_zone = if lopts.alignment == paged_text::Alignment::Justify {
        0
    } else {
        zone_64
    };
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
    // Q-20: fold letter-spacing budget into the per-word stretch /
    // shrink budget so the breaker can lean on inter-glyph space when
    // word-space alone can't justify a line. IDML's
    // `Min/Desired/Max LetterSpacing` is in pt and applies *between
    // glyphs*; we approximate by adding `letter_delta_pt * avg_chars_per_word`
    // into the existing space stretch / shrink ratios. Default values
    // (0 pt) are a no-op. Real per-glyph distribution after the
    // breaker picks breaks is queued.
    let ls_min = resolved.minimum_letter_spacing.unwrap_or(0.0);
    let ls_desired = resolved.desired_letter_spacing.unwrap_or(0.0);
    let ls_max = resolved.maximum_letter_spacing.unwrap_or(0.0);
    if ls_min != 0.0 || ls_desired != 0.0 || ls_max != 0.0 {
        // Cycle-6 Track 3: bounded mapping from LS budget (pt) to
        // stretch_add / shrink_add. The cycle-5 formula
        // `(ls_max - ls_desired) * AVG_CHARS_PER_WORD / space_width`
        // saturated `.min(2.0)` on typical IDML LS values (e.g.
        // newspaper's body MaximumLetterSpacing=25 ⇒ stretch_add ≈ 78
        // clamped to 2.0), making any AVG_CHARS_PER_WORD-style tweak
        // invisible to the harness. The new mapping caps the
        // contribution at 0.5 / 0.25 (half of the legacy ceiling) so
        // the breaker has letter-spacing budget without overwhelming
        // word-spacing budget. `LS_BUDGET_PT_FOR_FULL_STRETCH = 24.0`
        // calibrates from the InDesign default 25pt-vs-0 spread
        // mapping to ~full contribution; smaller spreads fall below
        // proportionally and remain unsaturated.
        const LS_BUDGET_PT_FOR_FULL_STRETCH: f32 = 12.0;
        let stretch_budget = (ls_max - ls_desired).max(0.0);
        let shrink_budget = (ls_desired - ls_min).max(0.0);
        let stretch_add = (stretch_budget / LS_BUDGET_PT_FOR_FULL_STRETCH).clamp(0.0, 0.5);
        let shrink_add = (shrink_budget / LS_BUDGET_PT_FOR_FULL_STRETCH).clamp(0.0, 0.25);
        lopts.compose.stretch_ratio = (lopts.compose.stretch_ratio + stretch_add).min(2.0);
        lopts.compose.shrink_ratio = (lopts.compose.shrink_ratio + shrink_add).min(0.5);
    }
    // Q-20: glyph scaling. When `Min/Max GlyphScaling` differ from
    // 100 the IDML allows the composer to scale per-glyph x-advance
    // by that percentage. Per-glyph distribution after Knuth-Plass
    // is the proper implementation; for now we widen the stretch
    // ratio so the breaker has the budget the IDML implies. None of
    // the cycle-2 evidence packs vary this from 100, so this is
    // foundation work that lights up on packs that do customise it.
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
