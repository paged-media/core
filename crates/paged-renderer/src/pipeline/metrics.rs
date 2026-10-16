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

/// Resolve an IDML `Position` (super/subscript) into a `(size_factor,
/// baseline_offset_fraction)` pair, both relative to the run's base
/// point size. A positive offset lifts the glyphs (superscript); a
/// negative one drops them (subscript) — matching `baseline_shift_pt`'s
/// sign convention in the layout emit.
///
/// InDesign derives the exact factors from the document's Superscript /
/// Subscript Size & Position text preferences; we use its factory
/// defaults (58.3 % size, ±33.3 % of the base size) because
/// `Resources/Preferences.xml` is not parsed yet (a separate gap).
///
/// The OpenType positions (`OTSuperscript`, `OTSubscript`,
/// `OTNumerator`, `OTDenominator`) are NOT scaled or shifted: they are
/// the font's `sups` / `subs` / `numr` / `dnom` glyphs at the run's size
/// on its baseline, and a face without the feature prints its plain
/// glyph (asked of InDesign 20.0.1, the `opentype` fixture; see
/// [`paged_text::OtPosition`]).
pub fn position_metrics(position: Option<&str>) -> (f32, f32) {
    const SIZE_FACTOR: f32 = 0.583;
    const OFFSET_FACTOR: f32 = 0.333;
    match position {
        Some("Superscript") => (SIZE_FACTOR, OFFSET_FACTOR),
        Some("Subscript") => (SIZE_FACTOR, -OFFSET_FACTOR),
        // `Normal` / `None` / the OpenType positions / unknown ⇒ identity.
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

/// Phase 4 typography — translate a `ResolvedRunAttrs`'s `Ligatures` /
/// `KerningMethod`, `OTF*` toggles, `Capitalization` and `Position` into
/// the shaper's [`paged_text::ShapingFeatures`]. Inputs are
/// `None`-tolerant: missing `ligatures_on` defaults to true (InDesign's
/// CharacterStyle default); unrecognised `kerning_method` strings fall
/// through to `Metrics`. `"Optical"` falls through to `Optical` even
/// though the renderer currently shapes it the same as `Metrics` — the
/// cache key still distinguishes the two so the optical-kerning pass can
/// land later without invalidating the cache.
pub fn shaping_features_from(
    ligatures_on: Option<bool>,
    kerning_method: Option<&str>,
    otf: &paged_model::OtfFeatures,
    capitalization: Option<&str>,
    position: Option<&str>,
) -> paged_text::ShapingFeatures {
    use paged_text::KerningMethod as K;
    paged_text::ShapingFeatures {
        ligatures_on: ligatures_on.unwrap_or(true),
        // InDesign writes the value with its `$ID/` prefix (`$ID/None`,
        // `$ID/Optical`, `$ID/Metrics`; `kerning` fixture); hand-written
        // files spell it bare. Optical falls back to metrics in the shaper
        // until optical kerning lands (R-40).
        kerning: match kerning_method.map(|m| m.strip_prefix("$ID/").unwrap_or(m)) {
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
        // `SmallCaps` / `CapToSmallCap` are shaping, not a text
        // transform: `AllCaps` is uppercased upstream, these two ask
        // the font for its small-capital glyphs.
        small_caps: paged_text::SmallCaps::from_idml(capitalization),
        stylistic_alternates: otf.stylistic_alternate.unwrap_or(false),
        historical: otf.historical.unwrap_or(false),
        position: paged_text::OtPosition::from_idml(position),
        // InDesign 20.0.1 sets All Caps with `case` and `cpsp`, typed
        // capitals with neither (see `ShapingFeatures::all_caps`).
        all_caps: capitalization == Some("AllCaps"),
        // InDesign's Metrics kerning sets a CJK face's proportional widths
        // (`ShapingFeatures::proportional_widths`).
        proportional_widths: otf.proportional_metrics == Some(true)
            || !matches!(
                kerning_method.map(|m| m.strip_prefix("$ID/").unwrap_or(m)),
                Some("None" | "Optical")
            ),
        vertical: false,
        ruby_glyphs: false,
    }
}

/// The edge of a page its binding (the spine) runs along.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BindingEdge {
    /// A recto, and a page alone in its spread: the spine is to its left.
    #[default]
    Left,
    /// A verso: the spine is to its right.
    Right,
}

impl BindingEdge {
    /// From where a page sits in its spread (`spread_left` and `width`,
    /// spread-inner pt). IDML puts the spine at spread x = 0: a reader
    /// spread's verso spans `-W..0` and its recto `0..W`, and a facing
    /// document's lone first page is a recto at `0..W`. A page left of
    /// the spine binds on its right edge. A non-facing page centred on
    /// the spine keeps the left binding.
    pub fn of_page(spread_left: f32, width: f32) -> Self {
        if spread_left + width / 2.0 < -0.5 {
            BindingEdge::Right
        } else {
            BindingEdge::Left
        }
    }

    fn toward(self) -> paged_text::Alignment {
        match self {
            BindingEdge::Left => paged_text::Alignment::Left,
            BindingEdge::Right => paged_text::Alignment::Right,
        }
    }

    fn away(self) -> paged_text::Alignment {
        match self {
            BindingEdge::Left => paged_text::Alignment::Right,
            BindingEdge::Right => paged_text::Alignment::Left,
        }
    }
}

/// Map an IDML `Justification` to the composer's alignment and the
/// alignment of a justified paragraph's last line
/// ([`paged_text::LayoutOptions::last_line_alignment`]), as InDesign
/// 20.0.1 sets them (`justification` fixture):
///
/// * Left-, Center- and RightJustified break and stretch alike and set
///   the last line left, centred or right;
/// * FullyJustified stretches the last line too;
/// * ToBindingSide / AwayFromBindingSide align toward / away from the
///   spine: left / right on a recto, right / left on a verso.
///
/// `None` (no attribute anywhere in the cascade) is IDML's `LeftAlign`.
pub fn map_justification(
    j: Option<paged_model::Justification>,
    binding: BindingEdge,
) -> (paged_text::Alignment, paged_text::Alignment) {
    use paged_model::Justification as J;
    use paged_text::Alignment as A;
    let ragged = |a: A| (a, a);
    match j {
        Some(J::LeftAlign) | None => ragged(A::Left),
        Some(J::CenterAlign) => ragged(A::Center),
        Some(J::RightAlign) => ragged(A::Right),
        Some(J::LeftJustified) => (A::Justify, A::Left),
        Some(J::CenterJustified) => (A::Justify, A::Center),
        Some(J::RightJustified) => (A::Justify, A::Right),
        Some(J::FullyJustified) => (A::Justify, A::Justify),
        Some(J::ToBindingSide) => ragged(binding.toward()),
        Some(J::AwayFromBindingSide) => ragged(binding.away()),
    }
}

/// How far (1/64 pt) InDesign lets a line run past its measure before the
/// Paragraph Composer breaks it, at `size` pt. Asked of InDesign 20.0.1
/// (`tools/indesign-export/fit-tolerance-probe.json`, bisected at 6 to 48
/// pt): ragged lines keep a word up to `size / 32 + 0.254 / size` pt over,
/// justified lines up to `0.0466 × size + 0.178 / size` pt — every
/// measured point within 0.003 pt, at any word spacing and however many
/// spaces the line holds.
pub(super) fn fit_tolerance_64(size: f32, justified: bool) -> i32 {
    if size <= 0.0 {
        return 0;
    }
    let pt = if justified {
        0.0466 * size + 0.178 / size
    } else {
        size / 32.0 + 0.254 / size
    };
    (pt * paged_text::shape::ADVANCE_PRECISION).round() as i32
}

pub(super) fn map_tab_alignment(a: Option<&str>) -> paged_text::layout::TabAlignment {
    match a {
        Some("RightAlign") => paged_text::layout::TabAlignment::Right,
        Some("CenterAlign") => paged_text::layout::TabAlignment::Center,
        Some("CharacterAlign") => paged_text::layout::TabAlignment::Decimal,
        _ => paged_text::layout::TabAlignment::Left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fit-tolerance-probe.json`: InDesign 20.0.1's bisected allowances.
    #[test]
    fn fit_tolerance_matches_indesigns_measured_allowance() {
        let pt = |s: f32, j: bool| fit_tolerance_64(s, j) as f32 / 64.0;
        for (size, ragged, justified) in [
            (6.0, 0.2299, 0.3093),
            (12.0, 0.3973, 0.5741),
            (24.0, 0.7600, 1.1247),
            (48.0, 1.5023, 2.2377),
        ] {
            assert!(
                (pt(size, false) - ragged).abs() < 0.02,
                "{size} ragged {}",
                pt(size, false)
            );
            assert!(
                (pt(size, true) - justified).abs() < 0.02,
                "{size} justified {}",
                pt(size, true)
            );
        }
    }
    use paged_model::Justification as J;
    use paged_text::Alignment as A;

    /// The spine is at spread x = 0: a reader spread's verso (`-W..0`)
    /// binds on its right, its recto (`0..W`) and a facing document's
    /// lone first page on their left; a non-facing page centred on the
    /// spine keeps the left binding.
    /// `kerning` fixture: InDesign's `$ID/None` turns kerning off, exactly
    /// as the bare spelling does.
    #[test]
    fn kerning_method_id_none_is_off() {
        let otf = paged_model::OtfFeatures::default();
        for spelling in ["None", "$ID/None"] {
            let f = shaping_features_from(None, Some(spelling), &otf, None, None);
            assert_eq!(f.kerning, paged_text::KerningMethod::Off, "{spelling}");
        }
        let f = shaping_features_from(None, Some("$ID/Optical"), &otf, None, None);
        assert_eq!(f.kerning, paged_text::KerningMethod::Optical);
    }

    #[test]
    fn binding_edge_follows_the_spine() {
        assert_eq!(BindingEdge::of_page(-612.0, 612.0), BindingEdge::Right);
        assert_eq!(BindingEdge::of_page(0.0, 612.0), BindingEdge::Left);
        assert_eq!(BindingEdge::of_page(-306.0, 612.0), BindingEdge::Left);
    }

    /// InDesign 20.0.1 (`justification`): the X-Justified values stretch
    /// their lines and set the last one to X; FullyJustified stretches it
    /// too; the binding-side values follow the page side.
    #[test]
    fn justification_maps_body_and_last_line() {
        let recto = BindingEdge::Left;
        let verso = BindingEdge::Right;
        assert_eq!(map_justification(None, recto), (A::Left, A::Left));
        assert_eq!(
            map_justification(Some(J::LeftJustified), recto),
            (A::Justify, A::Left)
        );
        assert_eq!(
            map_justification(Some(J::CenterJustified), recto),
            (A::Justify, A::Center)
        );
        assert_eq!(
            map_justification(Some(J::RightJustified), recto),
            (A::Justify, A::Right)
        );
        assert_eq!(
            map_justification(Some(J::FullyJustified), recto),
            (A::Justify, A::Justify)
        );
        assert_eq!(
            map_justification(Some(J::ToBindingSide), recto),
            (A::Left, A::Left)
        );
        assert_eq!(
            map_justification(Some(J::ToBindingSide), verso),
            (A::Right, A::Right)
        );
        assert_eq!(
            map_justification(Some(J::AwayFromBindingSide), recto),
            (A::Right, A::Right)
        );
        assert_eq!(
            map_justification(Some(J::AwayFromBindingSide), verso),
            (A::Left, A::Left)
        );
    }
}
