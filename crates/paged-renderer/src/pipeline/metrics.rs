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

//! Sub/superscript position metrics, shaping features, justification + tab-alignment mapping — extracted from pipeline/mod.rs (1.6b).

/// Phase 4 typography — translate a `ResolvedRunAttrs`'s `Ligatures` /
/// `KerningMethod` into the shaper's [`paged_text::ShapingFeatures`].
/// Inputs are `None`-tolerant: missing `ligatures_on` defaults to true
/// (InDesign's CharacterStyle default); unrecognised `kerning_method`
/// strings fall through to `Metrics`. `"Optical"` falls through to
/// `Optical` even though the renderer currently shapes it the same as
/// `Metrics` — the cache key still distinguishes the two so the
/// optical-kerning pass can land later without invalidating the cache.
/// Resolve an IDML `Position` (super/subscript) into a `(size_factor,
/// baseline_offset_fraction)` pair, both relative to the run's base
/// point size. A positive offset lifts the glyphs (superscript); a
/// negative one drops them (subscript) — matching `baseline_shift_pt`'s
/// sign convention in the layout emit.
///
/// InDesign derives the exact factors from the document's Superscript /
/// Subscript Size & Position text preferences; we use its factory
/// defaults (58.3 % size, ±33.3 % of the base size) because
/// `Resources/Preferences.xml` is not parsed yet (a separate gap). The
/// OpenType variants (`OT*`, `Numerator`/`Denominator`) reuse the same
/// geometric fallback until real OT feature lookup lands.
pub fn position_metrics(position: Option<&str>) -> (f32, f32) {
    const SIZE_FACTOR: f32 = 0.583;
    const OFFSET_FACTOR: f32 = 0.333;
    match position {
        Some("Superscript") | Some("OTSuperscript") | Some("OTNumerator") => {
            (SIZE_FACTOR, OFFSET_FACTOR)
        }
        Some("Subscript") | Some("OTSubscript") | Some("OTDenominator") => {
            (SIZE_FACTOR, -OFFSET_FACTOR)
        }
        // `Normal` / `None` / unknown ⇒ identity.
        _ => (1.0, 0.0),
    }
}

/// Combine a run's base point size, its explicit `BaselineShift`, and
/// its `Position` (super/subscript) into the `(point_size,
/// baseline_shift_pt)` pair the layout emit consumes.
///
/// - `point_size` shrinks by the `Position` size factor (super/subscript
///   render at a fraction of the base; `Normal` keeps the base).
/// - `baseline_shift_pt` adds the `Position` baseline offset (a fraction
///   of the *base* size) on top of any explicit `BaselineShift`, so a
///   superscript both lifts and shrinks while an explicit shift still
///   composes additively. The offset is computed against the base size
///   (not the shrunk size) to match InDesign's geometry.
pub fn position_adjusted_metrics(
    base_size: f32,
    explicit_baseline_shift: Option<f32>,
    position: Option<&str>,
) -> (f32, f32) {
    let (size_factor, offset_fraction) = position_metrics(position);
    let point_size = base_size * size_factor;
    let baseline_shift_pt = explicit_baseline_shift.unwrap_or(0.0) + base_size * offset_fraction;
    (point_size, baseline_shift_pt)
}

pub fn shaping_features_from(
    ligatures_on: Option<bool>,
    kerning_method: Option<&str>,
    otf: &paged_parse::OtfFeatures,
) -> paged_text::ShapingFeatures {
    use paged_text::KerningMethod as K;
    paged_text::ShapingFeatures {
        ligatures_on: ligatures_on.unwrap_or(true),
        kerning: match kerning_method {
            Some("None") => K::Off,
            Some("Optical") => K::Optical,
            // "Metrics" or anything else (incl. None) → default.
            _ => K::Metrics,
        },
        // Discrete OTF toggles: a `None` flag at the bottom of the
        // cascade means the feature is off (its OpenType default).
        // `OTFContextualAlternate` is the exception — fonts opt into
        // `calt` by default, so only an explicit `false` disables it.
        discretionary_ligatures: otf.discretionary_ligatures.unwrap_or(false),
        fractions: otf.fraction.unwrap_or(false),
        ordinals: otf.ordinal.unwrap_or(false),
        swash: otf.swash.unwrap_or(false),
        slashed_zero: otf.slashed_zero.unwrap_or(false),
        titling: otf.titling.unwrap_or(false),
        contextual_alternates: otf.contextual_alternates.unwrap_or(true),
        figure_style: paged_text::FigureStyle::from_idml(otf.figure_style.as_deref()),
        // Negative / absent bitfields ⇒ no stylistic set.
        stylistic_sets: otf.stylistic_sets.unwrap_or(0).max(0) as u32,
    }
}

pub fn map_justification(j: Option<paged_parse::Justification>) -> paged_text::Alignment {
    use paged_parse::Justification as J;
    match j {
        Some(J::RightAlign) | Some(J::RightJustified) | Some(J::AwayFromBindingSide) => {
            paged_text::Alignment::Right
        }
        Some(J::CenterAlign) | Some(J::CenterJustified) => paged_text::Alignment::Center,
        Some(J::FullyJustified) | Some(J::LeftJustified) => paged_text::Alignment::Justify,
        Some(J::LeftAlign) | Some(J::ToBindingSide) | None => paged_text::Alignment::Left,
    }
}

pub(super) fn map_tab_alignment(a: Option<&str>) -> paged_text::layout::TabAlignment {
    match a {
        Some("RightAlign") => paged_text::layout::TabAlignment::Right,
        Some("CenterAlign") => paged_text::layout::TabAlignment::Center,
        Some("CharacterAlign") => paged_text::layout::TabAlignment::Decimal,
        _ => paged_text::layout::TabAlignment::Left,
    }
}
