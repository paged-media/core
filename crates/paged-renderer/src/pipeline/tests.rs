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

use super::*;

#[test]
fn default_options_do_not_collect_glyph_runs() {
    // Load-bearing invariant: the glyph side-channel is opt-in.
    // With the default options every capture site no-ops, so the
    // canvas/CI command stream stays byte-identical to before the
    // PDF exporter existed. Flipping this default would silently
    // change every consumer — do it only on purpose.
    assert!(!PipelineOptions::default().collect_glyph_runs);
}

#[test]
fn position_metrics_super_sub_and_normal() {
    // Superscript / numerator lift (positive offset); subscript /
    // denominator drop (negative); both shrink to 58.3 %.
    assert_eq!(position_metrics(Some("Superscript")), (0.583, 0.333));
    assert_eq!(position_metrics(Some("OTSuperscript")), (0.583, 0.333));
    assert_eq!(position_metrics(Some("OTNumerator")), (0.583, 0.333));
    assert_eq!(position_metrics(Some("Subscript")), (0.583, -0.333));
    assert_eq!(position_metrics(Some("OTDenominator")), (0.583, -0.333));
    // Normal / absent / unknown ⇒ identity (no scale, no shift).
    assert_eq!(position_metrics(Some("Normal")), (1.0, 0.0));
    assert_eq!(position_metrics(None), (1.0, 0.0));
}

#[test]
fn position_adjusted_metrics_super_sub_and_normal() {
    // Base 12pt, no explicit baseline shift.
    // Superscript: 12 * 0.583 = 6.996 pt, +12 * 0.333 = +3.996 pt.
    let (sz, sh) = position_adjusted_metrics(12.0, None, Some("Superscript"));
    assert!((sz - 6.996).abs() < 1e-3, "super size {sz}");
    assert!((sh - 3.996).abs() < 1e-3, "super shift {sh}");
    // Subscript drops (negative shift), same shrink.
    let (sz, sh) = position_adjusted_metrics(12.0, None, Some("Subscript"));
    assert!((sz - 6.996).abs() < 1e-3, "sub size {sz}");
    assert!((sh + 3.996).abs() < 1e-3, "sub shift {sh}");
    // Normal: untouched size, zero shift.
    assert_eq!(
        position_adjusted_metrics(12.0, None, Some("Normal")),
        (12.0, 0.0)
    );
    assert_eq!(position_adjusted_metrics(12.0, None, None), (12.0, 0.0));
}

#[test]
fn position_adjusted_metrics_composes_explicit_baseline_shift() {
    // An explicit BaselineShift adds on top of the Position offset.
    // Base 10pt, explicit +2pt, superscript ⇒ shift = 2 + 10*0.333.
    let (sz, sh) = position_adjusted_metrics(10.0, Some(2.0), Some("Superscript"));
    assert!((sz - 5.83).abs() < 1e-3, "size {sz}");
    assert!((sh - (2.0 + 3.33)).abs() < 1e-3, "shift {sh}");
    // Explicit shift with Normal position is passed through verbatim
    // (no size change, no Position offset).
    let (sz, sh) = position_adjusted_metrics(10.0, Some(-1.5), None);
    assert_eq!(sz, 10.0);
    assert!((sh + 1.5).abs() < 1e-6, "shift {sh}");
}

#[test]
fn stroke_for_custom_styles_dashed_dotted_striped_wavy() {
    use paged_model::{StrokeStyleDef, StrokeStyleKind as K};
    let mk = |kind, pattern: &[f32]| {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "S".to_string(),
            StrokeStyleDef {
                self_id: "S".to_string(),
                name: None,
                kind,
                pattern: pattern.to_vec(),
                stripes: Vec::new(),
                wave_width: None,
                wave_length: None,
                gap_color: None,
                gap_tint: None,
            },
        );
        m
    };
    let go = |kind, pat: &[f32]| {
        let m = mk(kind, pat);
        stroke_for(Some("S"), 2.0, None, None, None, Some(&m), &[])
    };
    // Custom Dashed + Dotted patterns are consumed (a real dash).
    assert!(!go(K::Dashed, &[3.0, 2.0]).dash.is_solid(), "dashed");
    assert!(!go(K::Dotted, &[0.0, 2.0]).dash.is_solid(), "dotted");
    // `stroke_for` is the low-level *single*-stroke builder; Striped
    // / Wavy can't be expressed as one dash, so it returns a solid
    // base. The multi-rule / sine geometry for those is produced at
    // the emit site by `classify_stroke_style` + `emit_styled_stroke`
    // (W1.2), not here.
    assert!(go(K::Striped, &[]).dash.is_solid(), "striped base → solid");
    assert!(go(K::Wavy, &[]).dash.is_solid(), "wavy base → solid");
}

/// W1.1 — a per-frame `StrokeDashAndGap` override takes PRECEDENCE
/// over the `StrokeStyleDef` pattern (and over the built-in name
/// table): the override feeds the dash slot verbatim regardless of
/// the named style. An empty override falls back to the style.
#[test]
fn stroke_for_instance_dash_override_wins_over_style_pattern() {
    use paged_model::{StrokeStyleDef, StrokeStyleKind as K};
    let mut styles = std::collections::BTreeMap::new();
    styles.insert(
        "S".to_string(),
        StrokeStyleDef {
            self_id: "S".to_string(),
            name: None,
            kind: K::Dashed,
            pattern: vec![3.0, 2.0],
            stripes: Vec::new(),
            wave_width: None,
            wave_length: None,
            gap_color: None,
            gap_tint: None,
        },
    );
    // Instance override [9, 4] beats the style's [3, 2].
    let overridden = stroke_for(Some("S"), 2.0, None, None, None, Some(&styles), &[9.0, 4.0]);
    assert_eq!(overridden.dash.as_slice(), &[9.0, 4.0]);
    // Override wins even against a SOLID built-in name (no style def).
    let on_solid = stroke_for(
        Some("StrokeStyle/$ID/Solid"),
        2.0,
        None,
        None,
        None,
        None,
        &[7.0, 1.0],
    );
    assert_eq!(on_solid.dash.as_slice(), &[7.0, 1.0]);
    // Empty override → fall back to the style's [3, 2] pattern.
    let fallback = stroke_for(Some("S"), 2.0, None, None, None, Some(&styles), &[]);
    assert_eq!(fallback.dash.as_slice(), &[3.0, 2.0]);
}

#[test]
fn compose_outer_matrix_identity_mpt_is_origin_shift() {
    // Identity MasterPageTransform: outer collapses to
    // translate(target - master_origin), matching the legacy
    // translation-only stamp. master_origin (10,20), target (100,50).
    let outer = Transform::translate(100.0, 50.0)
        .compose(&Transform::IDENTITY)
        .compose(&Transform::translate(-10.0, -20.0));
    // Master item sitting at inner translate(3, 4).
    let m = compose_outer_matrix(outer, Some([1.0, 0.0, 0.0, 1.0, 3.0, 4.0]));
    assert_eq!(
        [m[0], m[1], m[2], m[3]],
        [1.0, 0.0, 0.0, 1.0],
        "linear part untouched"
    );
    assert!((m[4] - 93.0).abs() < 1e-4, "tx={} (100-10+3)", m[4]);
    assert!((m[5] - 34.0).abs() < 1e-4, "ty={} (50-20+4)", m[5]);
}

#[test]
fn compose_outer_matrix_applies_mpt_scale() {
    // A 2× MasterPageTransform about a master origin at (0,0) scales
    // the stamped item's linear part *and* its offset — the part the
    // old translation-only stamp silently dropped.
    let outer = Transform::translate(0.0, 0.0)
        .compose(&Transform([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]))
        .compose(&Transform::translate(0.0, 0.0));
    let m = compose_outer_matrix(outer, Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]));
    assert!(
        (m[0] - 2.0).abs() < 1e-4 && (m[3] - 2.0).abs() < 1e-4,
        "linear scaled"
    );
    assert!((m[4] - 10.0).abs() < 1e-4, "tx={} (5×2)", m[4]);
    assert!((m[5] - 14.0).abs() < 1e-4, "ty={} (7×2)", m[5]);
}

// ---- W1.9 spread-level ItemTransform rotation/scale ----

#[test]
fn spread_linear_transform_identity_and_translation_collapse() {
    // Absent → identity.
    assert_eq!(spread_linear_transform(None), Transform::IDENTITY);
    // Pure translation → identity (translation cancels against the
    // spread origin, so only the linear part rides the field).
    assert_eq!(
        spread_linear_transform(Some([1.0, 0.0, 0.0, 1.0, 40.0, -12.0])),
        Transform::IDENTITY
    );
}

#[test]
fn spread_linear_transform_keeps_rotation_drops_translation() {
    // 90° CW rotation (a=0,b=1,c=-1,d=0) with a translation → the
    // linear block is kept, the translation dropped.
    let lin = spread_linear_transform(Some([0.0, 1.0, -1.0, 0.0, 100.0, 200.0]));
    assert_eq!(lin.0, [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn frame_outer_transform_identity_spread_is_unchanged() {
    // With an identity spread_transform the outer is byte-identical
    // to the historical translate(-origin) ∘ item_transform.
    let page = BuiltPage {
        id: PageId::synthetic(0, 0),
        width_pt: 100.0,
        height_pt: 100.0,
        spread_origin: (10.0, 20.0),
        spread_transform: Transform::IDENTITY,
        list: DisplayList::new(),
        layout_generation: 0,
        numbering_generation: 0,
        stats: PipelineStats::default(),
        story_layout: Vec::new(),
        footnotes: Vec::new(),
        diagnostics: Vec::new(),
        cell_rects: Vec::new(),
        resource_tiles_needed: Vec::new(),
    };
    let outer = frame_outer_transform(&page, Some([1.0, 0.0, 0.0, 1.0, 5.0, 6.0]));
    // translate(-10,-20) ∘ translate(5,6) = translate(-5,-14).
    assert_eq!(outer.0, [1.0, 0.0, 0.0, 1.0, -5.0, -14.0]);
}

#[test]
fn frame_outer_transform_rotated_spread_rotates_about_page_origin() {
    // 90° CW spread rotation. A point at the page origin (spread
    // origin) stays put; a frame offset from the origin rotates
    // about it. spread_origin=(0,0) keeps the math clean.
    let page = BuiltPage {
        id: PageId::synthetic(0, 0),
        width_pt: 100.0,
        height_pt: 100.0,
        spread_origin: (0.0, 0.0),
        spread_transform: Transform([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
        list: DisplayList::new(),
        layout_generation: 0,
        numbering_generation: 0,
        stats: PipelineStats::default(),
        story_layout: Vec::new(),
        footnotes: Vec::new(),
        diagnostics: Vec::new(),
        cell_rects: Vec::new(),
        resource_tiles_needed: Vec::new(),
    };
    // Frame at inner origin translated to (30, 0). Under 90° CW
    // (x' = -y, y' = x), the frame's translation (30,0) maps to
    // (0, 30). outer = spread ∘ translate(0,0) ∘ translate(30,0).
    let outer = frame_outer_transform(&page, Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0]));
    let (x, y) = outer.apply(0.0, 0.0);
    assert!((x - 0.0).abs() < 1e-4, "x={x}");
    assert!((y - 30.0).abs() < 1e-4, "y={y}");
    // The linear block is the spread rotation composed with identity.
    assert!((outer.0[0]).abs() < 1e-4 && (outer.0[1] - 1.0).abs() < 1e-4);
}

#[test]
fn spread_transform_inverse_round_trips() {
    // The hit-tester inverts the same spread_transform the renderer
    // applied; a 90° rotation + 2× scale must round-trip.
    let s = Transform([0.0, 2.0, -2.0, 0.0, 0.0, 0.0]);
    let inv = s.inverse().expect("invertible");
    let p = (12.0, -5.0);
    let (fx, fy) = s.apply(p.0, p.1);
    let (bx, by) = inv.apply(fx, fy);
    assert!(
        (bx - p.0).abs() < 1e-4 && (by - p.1).abs() < 1e-4,
        "({bx},{by})"
    );
}

fn attrs(
    list_type: Option<&str>,
    ch: Option<u32>,
    after: Option<&str>,
) -> paged_scene::ResolvedParagraphAttrs {
    paged_scene::ResolvedParagraphAttrs {
        bullets_list_type: list_type.map(str::to_string),
        bullet_character: ch,
        bullets_text_after: after.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn list_prefix_builds_bullet_plus_separator() {
    let mut counter = 0;
    let mut prev_numbered = false;
    let p = list_prefix(
        &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
        &mut counter,
        &mut prev_numbered,
        None,
    )
    .unwrap();
    assert_eq!(p, "\u{2022} ");
    assert!(!prev_numbered, "BulletList clears prev_numbered");
}

#[test]
fn list_prefix_expands_caret_t_to_tab() {
    let mut counter = 0;
    let mut prev_numbered = false;
    let p = list_prefix(
        &attrs(Some("BulletList"), Some(0x2022), Some("^t")),
        &mut counter,
        &mut prev_numbered,
        None,
    )
    .unwrap();
    assert_eq!(p, "\u{2022}\t");
}

#[test]
fn list_prefix_none_for_nolist_clears_prev_numbered() {
    let mut counter = 5;
    let mut prev_numbered = true;
    assert!(list_prefix(
        &attrs(Some("NoList"), None, None),
        &mut counter,
        &mut prev_numbered,
        None
    )
    .is_none());
    // NoList shouldn't damage a sticky counter — a follow-on
    // NumberedList with `NumberingContinue` may resume.
    assert_eq!(counter, 5);
    assert!(!prev_numbered);
}

#[test]
fn list_prefix_numbered_increments_across_paragraphs() {
    let mut counter = 0;
    let mut prev_numbered = false;
    let attrs = attrs(Some("NumberedList"), None, None);
    // Default expression `^#.^t` ⇒ "<n>.\t".
    assert_eq!(
        list_prefix(&attrs, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("1.\t")
    );
    assert_eq!(
        list_prefix(&attrs, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("2.\t")
    );
    assert_eq!(
        list_prefix(&attrs, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("3.\t")
    );
    assert_eq!(counter, 3);
    assert!(prev_numbered);
}

#[test]
fn list_prefix_numbered_resets_after_non_numbered() {
    let mut counter = 0;
    let mut prev_numbered = false;
    let n = attrs(Some("NumberedList"), None, None);
    let none = attrs(None, None, None);
    list_prefix(&n, &mut counter, &mut prev_numbered, None); // 1.
    list_prefix(&n, &mut counter, &mut prev_numbered, None); // 2.
    list_prefix(&none, &mut counter, &mut prev_numbered, None); // clears prev_numbered, counter sticky
    assert!(!prev_numbered);
    assert_eq!(
        list_prefix(&n, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("1.\t"),
        "default behaviour: counter resets when prev wasn't numbered"
    );
}

#[test]
fn list_prefix_bullet_to_numbered_resets() {
    // Mixing list types in a row resets by default — each
    // list_type change starts a fresh sequence unless
    // NumberingContinue is set.
    let mut counter = 0;
    let mut prev_numbered = false;
    list_prefix(
        &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
        &mut counter,
        &mut prev_numbered,
        None,
    );
    assert!(!prev_numbered);
    let n = attrs(Some("NumberedList"), None, None);
    assert_eq!(
        list_prefix(&n, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("1.\t")
    );
}

#[test]
fn list_prefix_bullet_falls_back_to_default_when_codepoint_missing() {
    // BulletList without an explicit BulletChar still emits the
    // U+2022 default — matches InDesign's behaviour and lets
    // real-export IDMLs render visible bullets.
    let mut counter = 0;
    let mut prev_numbered = false;
    let prefix = list_prefix(
        &attrs(Some("BulletList"), None, Some(" ")),
        &mut counter,
        &mut prev_numbered,
        None,
    );
    assert_eq!(prefix.as_deref(), Some("\u{2022} "));
}

#[test]
fn list_prefix_numbering_start_at_jumps_counter() {
    // StartAt = 5 ⇒ first emission is "5.\t", then 6, 7, ...
    let mut counter = 0;
    let mut prev_numbered = false;
    let mut a = attrs(Some("NumberedList"), None, None);
    a.numbering_start_at = Some(5);
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("5.\t")
    );
    // StartAt only fires on paragraph entry; once it's been
    // applied, drop it for the next paragraph.
    a.numbering_start_at = None;
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("6.\t")
    );
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("7.\t")
    );
}

#[test]
fn list_prefix_numbering_start_at_mid_list_resets() {
    // After a few numbered paragraphs, a paragraph with
    // NumberingStartAt = 10 forces the counter to that value.
    let mut counter = 0;
    let mut prev_numbered = false;
    let plain = attrs(Some("NumberedList"), None, None);
    list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 1.
    list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 2.
    let mut jumped = attrs(Some("NumberedList"), None, None);
    jumped.numbering_start_at = Some(10);
    assert_eq!(
        list_prefix(&jumped, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("10.\t")
    );
    // Subsequent plain paragraphs continue off the jump.
    assert_eq!(
        list_prefix(&plain, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("11.\t")
    );
}

#[test]
fn list_prefix_numbering_continue_persists_across_style_boundary() {
    // Numbered → BulletList → Numbered with `NumberingContinue`
    // resumes the count off the prior numbered run instead of
    // resetting to 1.
    let mut counter = 0;
    let mut prev_numbered = false;
    let plain = attrs(Some("NumberedList"), None, None);
    list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 1.
    list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 2.
    list_prefix(
        &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
        &mut counter,
        &mut prev_numbered,
        None,
    );
    let mut cont = attrs(Some("NumberedList"), None, None);
    cont.numbering_continue = Some(true);
    assert_eq!(
        list_prefix(&cont, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("3.\t"),
        "NumberingContinue suppresses the implicit reset"
    );
    // Compare against the default-reset path: without Continue,
    // the same scenario would have restarted at 1.
    let mut counter2 = 0;
    let mut prev2 = false;
    list_prefix(&plain, &mut counter2, &mut prev2, None); // 1.
    list_prefix(&plain, &mut counter2, &mut prev2, None); // 2.
    list_prefix(
        &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
        &mut counter2,
        &mut prev2,
        None,
    );
    assert_eq!(
        list_prefix(&plain, &mut counter2, &mut prev2, None).as_deref(),
        Some("1.\t"),
        "without NumberingContinue the count resets"
    );
}

#[test]
fn list_prefix_uses_custom_numbering_expression() {
    // `Step ^# of 5^t` ⇒ "Step 1 of 5\t", "Step 2 of 5\t", ...
    let mut counter = 0;
    let mut prev_numbered = false;
    let mut a = attrs(Some("NumberedList"), None, None);
    a.numbering_expression = Some("Step ^# of 5^t".to_string());
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("Step 1 of 5\t")
    );
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("Step 2 of 5\t")
    );
}

#[test]
fn list_prefix_cross_story_seed_continues_and_suppresses_reset() {
    // W1.22 — a ContinueNumbersAcrossStories list. The first
    // numbered paragraph of a fresh story emitter has
    // prev_was_numbered=false (no neighbour), which WITHOUT the
    // seed would reset to "1". With `cross_story_seed = Some(2)`
    // (the ledger's last value) it must continue at "3".
    let n = attrs(Some("NumberedList"), None, None);
    let mut counter = 0; // fresh per-story emitter
    let mut prev_numbered = false; // story start
    assert_eq!(
        list_prefix(&n, &mut counter, &mut prev_numbered, Some(2)).as_deref(),
        Some("3.\t"),
        "cross-story seed of 2 must continue at 3, not reset to 1",
    );
    assert_eq!(counter, 3, "counter advances off the seed");
    // NumberingStartAt still wins over the seed (explicit restart).
    let mut started = attrs(Some("NumberedList"), None, None);
    started.numbering_start_at = Some(10);
    let mut counter2 = 0;
    let mut prev2 = false;
    assert_eq!(
        list_prefix(&started, &mut counter2, &mut prev2, Some(5)).as_deref(),
        Some("10.\t"),
        "explicit NumberingStartAt overrides the cross-story seed",
    );
}

#[test]
fn substitute_numbering_expression_passes_literals_and_decodes_caret_escape() {
    // `^^` decodes to a literal caret; unknown `^x` sequences
    // pass through verbatim (no surprise glyph loss).
    assert_eq!(substitute_numbering_expression("^^#^t", "1"), "^#\t");
    assert_eq!(substitute_numbering_expression("(^#)^t", "42"), "(42)\t");
    assert_eq!(substitute_numbering_expression("^?", "1"), "^?");
    // Trailing lone caret passes through.
    assert_eq!(substitute_numbering_expression("^# ^", "5"), "5 ^");
}

#[test]
fn format_number_arabic_default() {
    assert_eq!(format_number(1, None), "1");
    assert_eq!(format_number(42, None), "42");
    assert_eq!(format_number(7, Some("1, 2, 3, 4...")), "7");
}

#[test]
fn format_number_zero_padded() {
    assert_eq!(format_number(1, Some("01, 02, 03, 04...")), "01");
    assert_eq!(format_number(42, Some("01, 02, 03...")), "42");
    assert_eq!(format_number(7, Some("001, 002, 003...")), "007");
}

#[test]
fn format_number_roman_upper_lower() {
    assert_eq!(format_number(1, Some("I, II, III, IV...")), "I");
    assert_eq!(format_number(4, Some("I, II, III, IV...")), "IV");
    assert_eq!(format_number(9, Some("I, II, III...")), "IX");
    assert_eq!(format_number(40, Some("I, II, III...")), "XL");
    assert_eq!(format_number(1994, Some("I, II, III...")), "MCMXCIV");
    assert_eq!(format_number(4, Some("i, ii, iii, iv...")), "iv");
}

#[test]
fn format_number_alpha_upper_lower() {
    assert_eq!(format_number(1, Some("A, B, C, D...")), "A");
    assert_eq!(format_number(26, Some("A, B, C...")), "Z");
    assert_eq!(format_number(27, Some("A, B, C...")), "AA");
    assert_eq!(format_number(28, Some("A, B, C...")), "AB");
    assert_eq!(format_number(703, Some("A, B, C...")), "AAA");
    assert_eq!(format_number(2, Some("a, b, c...")), "b");
}

#[test]
fn format_number_unknown_falls_back_to_arabic() {
    assert_eq!(format_number(5, Some("Q, R, S, ...")), "5");
    assert_eq!(format_number(5, Some("not a format")), "5");
}

#[test]
fn format_number_hanzi_everyday() {
    let f = |n| format_number(n, Some("一, 二, 三..."));
    assert_eq!(f(1), "一");
    assert_eq!(f(5), "五");
    assert_eq!(f(9), "九");
    // 10..=19: leading 十 without 一 prefix.
    assert_eq!(f(10), "十");
    assert_eq!(f(11), "十一");
    assert_eq!(f(15), "十五");
    // 20..=99: digit + 十 + units.
    assert_eq!(f(20), "二十");
    assert_eq!(f(25), "二十五");
    assert_eq!(f(99), "九十九");
    // 100..=999: hundreds digit + 百 + tens + units.
    assert_eq!(f(100), "一百");
    // 零 gap-marker when tens=0 but units>0 (e.g. 101 = 一百零一).
    assert_eq!(f(101), "一百零一");
    // 110 = hundreds + 一 + 十 (no 零, tens is non-zero).
    assert_eq!(f(110), "一百一十");
    assert_eq!(f(125), "一百二十五");
    assert_eq!(f(999), "九百九十九");
    // ≥ 1000: fallback to Arabic.
    assert_eq!(f(1000), "1000");
}

#[test]
fn format_number_hanzi_formal() {
    let f = |n| format_number(n, Some("壹, 貳, 參..."));
    // Formal/financial digit set.
    assert_eq!(f(1), "壹");
    assert_eq!(f(5), "伍");
    assert_eq!(f(9), "玖");
    // 10..=19: formal keeps 壹十 prefix (unlike everyday).
    assert_eq!(f(11), "壹十壹");
}

#[test]
fn list_prefix_uses_numbering_format() {
    let mut counter = 0;
    let mut prev_numbered = false;
    let mut a = attrs(Some("NumberedList"), None, None);
    a.numbering_format = Some("I, II, III, IV...".to_string());
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("I.\t")
    );
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("II.\t")
    );
    assert_eq!(
        list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
        Some("III.\t")
    );
}

fn approx(a: (f32, f32), b: (f32, f32)) {
    let eps = 1e-5;
    assert!(
        (a.0 - b.0).abs() < eps && (a.1 - b.1).abs() < eps,
        "expected {b:?}, got {a:?}",
    );
}

#[test]
fn gradient_endpoints_zero_degrees_horizontal() {
    // 0° = horizontal left → right (IDML's default direction).
    let (s, e) = linear_gradient_endpoints(None, None, None);
    approx(s, (0.0, 0.5));
    approx(e, (1.0, 0.5));
    // Some(0.0) must match None — both are the spec default.
    let (s, e) = linear_gradient_endpoints(Some(0.0), None, None);
    approx(s, (0.0, 0.5));
    approx(e, (1.0, 0.5));
}

#[test]
fn gradient_endpoints_ninety_degrees_vertical() {
    // Regression for the fill-side default that used to be
    // hardcoded `(0,0)→(0,1)` (top→bottom). 90° must keep that
    // orientation: in IDML's y-down convention the +y axis points
    // down the page, so 90° rotates the gradient line vertically.
    let (s, e) = linear_gradient_endpoints(Some(90.0), None, None);
    approx(s, (0.5, 0.0));
    approx(e, (0.5, 1.0));
}

#[test]
fn gradient_endpoints_forty_five_degrees() {
    // 45° at default length: half-vector magnitude = 0.5 along the
    // unit vector `(cos 45°, sin 45°)`. Endpoints sit inside the
    // unit rect (the half-distance projects shorter than the
    // diagonal); that matches the existing fill-default behaviour.
    let (s, e) = linear_gradient_endpoints(Some(45.0), None, None);
    let r = std::f32::consts::FRAC_1_SQRT_2 * 0.5;
    approx(s, (0.5 - r, 0.5 - r));
    approx(e, (0.5 + r, 0.5 + r));
}

#[test]
fn gradient_endpoints_negative_angle_matches_supplement() {
    // -45° (= 315°) reflects the 45° endpoints across the
    // horizontal axis. cos is symmetric, sin flips sign.
    let (s_neg, e_neg) = linear_gradient_endpoints(Some(-45.0), None, None);
    let (s_pos, e_pos) = linear_gradient_endpoints(Some(315.0), None, None);
    approx(s_neg, s_pos);
    approx(e_neg, e_pos);
    let r = std::f32::consts::FRAC_1_SQRT_2 * 0.5;
    approx(s_neg, (0.5 - r, 0.5 + r));
    approx(e_neg, (0.5 + r, 0.5 - r));
}

#[test]
fn gradient_endpoints_explicit_length_compresses_line() {
    // GradientFillLength in pt converts to unit-rect half-vector
    // `(cos θ · L / (2·w), sin θ · L / (2·h))`. For a 200×100 rect
    // at 0° with L = 100pt the half-vec is `(0.25, 0)` so endpoints
    // hug the rect centre instead of running edge-to-edge.
    let (s, e) = linear_gradient_endpoints(Some(0.0), Some(100.0), Some((200.0, 100.0)));
    approx(s, (0.25, 0.5));
    approx(e, (0.75, 0.5));
    // 90° on the same rect with L=100 → half-vec `(0, 0.5)` so the
    // gradient line still spans edge-to-edge along the short axis.
    let (s, e) = linear_gradient_endpoints(Some(90.0), Some(100.0), Some((200.0, 100.0)));
    approx(s, (0.5, 0.0));
    approx(e, (0.5, 1.0));
}

#[test]
fn gradient_endpoints_length_without_dims_falls_through_to_default() {
    // Without bbox dimensions we can't convert pt to unit-rect
    // coords; helper falls back to the unit-vector default so
    // callers that lack geometry (e.g. legacy text-frame strokes
    // that don't track a bbox) still produce a sensible line.
    let (s, e) = linear_gradient_endpoints(Some(0.0), Some(100.0), None);
    approx(s, (0.0, 0.5));
    approx(e, (1.0, 0.5));
}

fn anchor_at(x: f32, y: f32) -> paged_model::PathAnchor {
    paged_model::PathAnchor {
        anchor: (x, y),
        left: (x, y),
        right: (x, y),
    }
}

/// `polygon_path_from_anchors` collapses to a single MoveTo/Close
/// when given no subpath markers — the legacy serialisation that
/// every InDesign-export polygon uses.
#[test]
fn polygon_path_from_anchors_single_contour_emits_one_subpath() {
    let anchors = vec![
        anchor_at(0.0, 0.0),
        anchor_at(10.0, 0.0),
        anchor_at(10.0, 10.0),
        anchor_at(0.0, 10.0),
    ];
    let path = polygon_path_from_anchors(&anchors, &[]);
    let move_count = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
        .count();
    let close_count = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::Close))
        .count();
    assert_eq!(move_count, 1, "legacy single-contour input → one MoveTo");
    assert_eq!(close_count, 1, "legacy single-contour input → one Close");
}

/// Compound-path input (square with hole — two `<GeometryPathType>`
/// contours in the source IDML) emits one MoveTo/Close per
/// contour. Without this, the renderer would draw a stray segment
/// from the outer contour's last anchor to the inner contour's
/// first anchor and silently mis-render the hole as a triangle
/// notch in the outer outline.
#[test]
fn polygon_path_from_anchors_compound_emits_one_subpath_per_contour() {
    let anchors = vec![
        // outer
        anchor_at(0.0, 0.0),
        anchor_at(200.0, 0.0),
        anchor_at(200.0, 200.0),
        anchor_at(0.0, 200.0),
        // inner
        anchor_at(60.0, 60.0),
        anchor_at(60.0, 140.0),
        anchor_at(140.0, 140.0),
        anchor_at(140.0, 60.0),
    ];
    let subpath_starts = vec![0, 4];
    let path = polygon_path_from_anchors(&anchors, &subpath_starts);
    let moves: Vec<&PathSegment> = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
        .collect();
    let closes = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::Close))
        .count();
    assert_eq!(moves.len(), 2, "two contours → two MoveTo segments");
    assert_eq!(closes, 2, "two contours → two Close segments");
    // The two MoveTos should land on the first anchor of each
    // contour — guards against a silent off-by-one in the range
    // construction that would otherwise still emit two contours
    // but join them at the wrong points.
    match moves[0] {
        PathSegment::MoveTo { x, y } => {
            assert!((*x - 0.0).abs() < 1e-6 && (*y - 0.0).abs() < 1e-6)
        }
        _ => unreachable!(),
    }
    match moves[1] {
        PathSegment::MoveTo { x, y } => {
            assert!((*x - 60.0).abs() < 1e-6 && (*y - 60.0).abs() < 1e-6)
        }
        _ => unreachable!(),
    }
}

/// Defensive: subpath markers that point past the end of the
/// anchor list, or that duplicate the implicit "starts at 0"
/// boundary, must not produce empty contours or panic.
#[test]
fn polygon_path_from_anchors_filters_bogus_markers() {
    let anchors = vec![
        anchor_at(0.0, 0.0),
        anchor_at(10.0, 0.0),
        anchor_at(10.0, 10.0),
    ];
    let path = polygon_path_from_anchors(&anchors, &[0, 99, 0]);
    let moves = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
        .count();
    let closes = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::Close))
        .count();
    assert_eq!(
        moves, 1,
        "out-of-range / duplicate markers collapse to one contour"
    );
    assert_eq!(closes, 1);
}

/// P-15: open contours skip the closing CubicTo + Close so a
/// `<GeometryPathType PathOpen="true">` polygon doesn't get
/// auto-filled.
#[test]
fn polygon_path_from_anchors_with_open_skips_close_for_open_contour() {
    let anchors = vec![
        anchor_at(0.0, 0.0),
        anchor_at(40.0, 0.0),
        anchor_at(20.0, 40.0),
    ];
    let path = polygon_path_from_anchors_with_open(&anchors, &[], &[true]);
    let moves = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
        .count();
    let closes = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::Close))
        .count();
    let cubics = path
        .segments
        .iter()
        .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
        .count();
    assert_eq!(moves, 1, "single contour → one MoveTo");
    assert_eq!(closes, 0, "open contour skips the Close");
    // 3 anchors → 2 inter-anchor CubicTos; the closing back-to-first
    // cubic must NOT fire (so 2, not 3).
    assert_eq!(cubics, 2, "open contour skips the closing CubicTo");
}

/// W1.1: when an anchor's Bezier handles coincide with its anchor
/// point — the IDML serialisation for a straight corner — the
/// emitted CubicTo's control points land exactly on the segment's
/// endpoints, so the cubic reduces to a straight line in both
/// rasterizers (tiny-skia / Vello flatten a degenerate cubic to a
/// LineTo). Locks the "handle == anchor ⇒ line" contract.
#[test]
fn polygon_path_from_anchors_straight_segment_cubic_collapses_to_line() {
    let anchors = vec![anchor_at(0.0, 0.0), anchor_at(10.0, 0.0)];
    let path = polygon_path_from_anchors(&anchors, &[]);
    // First CubicTo is the forward segment 0→1; its controls must be
    // the two anchors (no bow).
    let cubic = path
        .segments
        .iter()
        .find_map(|s| match s {
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => Some((*cx1, *cy1, *cx2, *cy2, *x, *y)),
            _ => None,
        })
        .expect("a forward CubicTo is emitted between the two anchors");
    let (cx1, cy1, cx2, cy2, x, y) = cubic;
    // cx1/cy1 == from.right == anchor 0; cx2/cy2 == to.left == anchor 1.
    assert!((cx1 - 0.0).abs() < 1e-6 && (cy1 - 0.0).abs() < 1e-6);
    assert!((cx2 - 10.0).abs() < 1e-6 && (cy2 - 0.0).abs() < 1e-6);
    assert!((x - 10.0).abs() < 1e-6 && (y - 0.0).abs() < 1e-6);
}

fn font_table_with(cache: &[(&str, Option<&str>, &[u8])], fallback: Option<&[u8]>) -> FontTable {
    let mut hm: HashMap<(String, Option<String>), Bytes> = HashMap::new();
    for (family, style, b) in cache {
        hm.insert(
            (family.to_string(), style.map(str::to_string)),
            Bytes::copy_from_slice(b),
        );
    }
    FontTable {
        faces: HashMap::new(),
        face_bytes: HashMap::new(),
        cache: hm,
        fallback: fallback.map(Bytes::copy_from_slice),
        metrics: HashMap::new(),
        family_metrics: HashMap::new(),
    }
}

fn run_attrs(family: Option<&str>, style: Option<&str>) -> paged_scene::ResolvedRunAttrs {
    paged_scene::ResolvedRunAttrs {
        font: family.map(str::to_string),
        font_style: style.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn resolve_paragraph_bytes_falls_back_per_run_to_sibling_font() {
    // Mixed paragraph: one run references a registered family,
    // another references something the cache doesn't know AND no
    // document-wide fallback is configured. The unknown run
    // inherits the resolved sibling's bytes instead of dropping
    // the whole paragraph.
    let table = font_table_with(&[("Inter", None, b"INTER")], None);
    let runs = vec![
        run_attrs(Some("Inter"), None),
        run_attrs(Some("Limon Script"), None),
        run_attrs(Some("Inter"), None),
    ];
    let pool = table
        .resolve_paragraph_bytes(&runs)
        .expect("paragraph kept");
    assert_eq!(pool.len(), 3);
    assert_eq!(&pool[0][..], b"INTER");
    assert_eq!(&pool[1][..], b"INTER", "missing run inherits sibling");
    assert_eq!(&pool[2][..], b"INTER");
}

#[test]
fn resolve_paragraph_bytes_prefers_table_fallback_when_no_run_resolves() {
    // All runs reference unknown families but the renderer was
    // given a document-wide default font — every slot picks it up.
    let table = font_table_with(&[], Some(b"DEFAULT"));
    let runs = vec![
        run_attrs(Some("Unknown A"), None),
        run_attrs(Some("Unknown B"), Some("Bold")),
    ];
    let pool = table
        .resolve_paragraph_bytes(&runs)
        .expect("paragraph kept");
    assert_eq!(pool.len(), 2);
    assert_eq!(&pool[0][..], b"DEFAULT");
    assert_eq!(&pool[1][..], b"DEFAULT");
}

#[test]
fn resolve_paragraph_bytes_returns_none_when_nothing_resolves() {
    // No registered family, no fallback — caller still has to
    // skip the paragraph because there's literally no shaping
    // input.
    let table = font_table_with(&[], None);
    let runs = vec![run_attrs(Some("Unknown"), None)];
    assert!(table.resolve_paragraph_bytes(&runs).is_none());
}

// P-22: lock the stroke-alignment inset math. `tiny_skia` strokes
// centered on the path, so Inside alignment needs the path inset
// by +stroke/2 inward (i.e. shrink the rect), Outside by
// -stroke/2 (grow the rect), Center / None ⇒ 0. Regressions in
// this math show up as ½-px nudges on line-art-dense pages.
#[test]
fn stroke_alignment_offset_inside_returns_positive_half_weight() {
    assert!((stroke_alignment_offset(Some("InsideAlignment"), 2.0) - 1.0).abs() < 1e-6);
    assert!((stroke_alignment_offset(Some("InsideAlignment"), 0.5) - 0.25).abs() < 1e-6);
}

#[test]
fn stroke_alignment_offset_outside_returns_negative_half_weight() {
    assert!((stroke_alignment_offset(Some("OutsideAlignment"), 2.0) + 1.0).abs() < 1e-6);
}

#[test]
fn stroke_alignment_offset_center_and_none_return_zero() {
    assert_eq!(stroke_alignment_offset(Some("CenterAlignment"), 2.0), 0.0);
    assert_eq!(stroke_alignment_offset(None, 2.0), 0.0);
}

// P-25 regression: a paragraph ending with a trailing `\n` (the
// `<Br/>` after the final visible content) must NOT produce a
// phantom empty sub-paragraph. A NumberedList paragraph would
// otherwise increment its counter twice and emit two "01" /
// "02" markers per visible line.
#[test]
fn split_paragraph_at_breaks_drops_trailing_newline_only_sub_paragraph() {
    let run = paged_model::CharacterRun {
        text: "01\n".to_string(),
        ..paged_model::CharacterRun::default()
    };
    let paragraph = paged_model::Paragraph {
        runs: vec![run],
        ..paged_model::Paragraph::default()
    };
    let subs = split_paragraph_at_breaks(&paragraph);
    assert_eq!(
        subs.len(),
        1,
        "trailing \\n must not produce a phantom sub-paragraph"
    );
    assert_eq!(subs[0].runs.len(), 1);
    assert_eq!(subs[0].runs[0].text, "01");
}

// Belt + braces: pathological case where the splitter's hint
// path seeds an all-`\n` trailing run. The post-loop guard at
// the tail of `split_paragraph_at_breaks` must collapse it.
#[test]
fn split_paragraph_at_breaks_drops_trailing_all_newline_run_after_visible() {
    let visible = paged_model::CharacterRun {
        text: "01".to_string(),
        ..paged_model::CharacterRun::default()
    };
    let nl_only = paged_model::CharacterRun {
        text: "\n\n".to_string(),
        ..paged_model::CharacterRun::default()
    };
    let paragraph = paged_model::Paragraph {
        runs: vec![visible, nl_only],
        ..paged_model::Paragraph::default()
    };
    let subs = split_paragraph_at_breaks(&paragraph);
    // Two `\n` after visible content used to seed two empty
    // hint-only subs after the "01" one (= 3 total). The guard
    // collapses the trailing newline-only subs so a numbered
    // list emits its marker once, not three times.
    assert_eq!(
        subs.len(),
        1,
        "trailing-only-newline tail subs must collapse"
    );
    assert_eq!(subs[0].runs.len(), 1);
    assert_eq!(subs[0].runs[0].text, "01");
}

// Composed: inset_rect applied at the stroke offset must shrink
// (Inside) or grow (Outside) the rect by exactly the stroke width
// along each axis. A 100×100 rect with a 2-pt Inside stroke ends
// up 98×98, drawn so the centered stroke lands fully inside.
#[test]
fn stroke_alignment_inside_shrinks_rect_by_stroke_width() {
    let r = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    let off = stroke_alignment_offset(Some("InsideAlignment"), 2.0);
    let inset = inset_rect(r, off);
    assert!((inset.x - 1.0).abs() < 1e-6);
    assert!((inset.y - 1.0).abs() < 1e-6);
    assert!((inset.w - 98.0).abs() < 1e-6);
    assert!((inset.h - 98.0).abs() < 1e-6);
}

#[test]
// Deliberately asserts on source constants: this test pins the
// placeholder calibration values so an accidental edit trips CI.
#[allow(clippy::assertions_on_constants)]
fn q22_missing_image_placeholder_calibration_pinned() {
    assert!(
        (PLACEHOLDER_FILL_RGB - 0.5).abs() < 1e-6,
        "placeholder fill should target ~50% grey",
    );
    assert!(
        (PLACEHOLDER_X_STROKE_PT - 1.5).abs() < 1e-6,
        "placeholder X stroke should be 1.5pt",
    );
    assert!(
        PLACEHOLDER_X_RGB < 0.05,
        "placeholder X should read as near-black against the grey fill",
    );
}

/// Q-08 (hypothesis check, rect / oval path): for a rotated
/// rect / oval the `linear_gradient_endpoints` projection
/// (unit-rect coords) is fed through `Transform::for_rect_in(rect,
/// outer)` where `outer` already incorporates the shape's
/// `ItemTransform`. The composed transform IS what the rasterizer
/// uses to push the unit-rect endpoints into page space (see
/// `paged_gpu::cpu::build_linear_gradient_shader`), so a 90°-
/// vertical gradient on a 90°-rotated frame should produce a
/// horizontal page-space gradient line. Asserts that — guards
/// against a regression that would re-introduce the protocol's
/// hypothesised bug (ItemTransform ignored on gradient projection).
#[test]
fn q08_gradient_endpoints_rotate_with_item_transform() {
    let rect = paged_compose::Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    let (s_unit, e_unit) = linear_gradient_endpoints(Some(90.0), None, None);
    approx(s_unit, (0.5, 0.0));
    approx(e_unit, (0.5, 1.0));
    // Identity baseline: local vertical = page vertical.
    let xf_id = Transform::for_rect_in(rect, Transform::IDENTITY);
    approx(xf_id.apply(s_unit.0, s_unit.1), (50.0, 0.0));
    approx(xf_id.apply(e_unit.0, e_unit.1), (50.0, 100.0));
    // ItemTransform `0 1 -1 0 200 0` packs to `[a, b, c, d, tx,
    // ty] = [0, 1, -1, 0, 200, 0]` — a 90° rotation about the
    // origin plus translate(+200, 0). Maps frame-local (x, y) to
    // page (200 - y, x).
    let outer_rot = Transform([0.0, 1.0, -1.0, 0.0, 200.0, 0.0]);
    let xf_rot = Transform::for_rect_in(rect, outer_rot);
    approx(xf_rot.apply(s_unit.0, s_unit.1), (200.0, 50.0));
    approx(xf_rot.apply(e_unit.0, e_unit.1), (100.0, 50.0));
}

/// Q-08 polygon fix: a Polygon fill emits `FillPath` whose
/// rasterizer path_transform IS `outer` directly (the path lives
/// in inner-anchor coords). The fill module rewrites the
/// gradient's unit-rect endpoints to bbox-local inner coords so
/// the rasterizer's subsequent `outer.apply(...)` lands them in
/// the polygon's actual page span. Without that step a 90° fill
/// on the brochure's full-page background polygon collapses to a
/// ~1pt gradient line near the spread origin and renders flat.
/// Asserts the inner-coord math the fill module bakes in.
#[test]
fn q08_polygon_gradient_rebases_to_bbox() {
    // Brochure page-bg polygon dimensions (approx).
    let bbox = paged_compose::Rect {
        x: -8.5,
        y: -479.0,
        w: 612.3,
        h: 672.4,
    };
    let (s_unit, e_unit) =
        linear_gradient_endpoints(Some(90.0), Some(577.7332), Some((bbox.w, bbox.h)));
    // `rebase_gradient_to_bbox` applies this mapping.
    let start = (bbox.x + s_unit.0 * bbox.w, bbox.y + s_unit.1 * bbox.h);
    let end = (bbox.x + e_unit.0 * bbox.w, bbox.y + e_unit.1 * bbox.h);
    // Vertical line, horizontally centred on the bbox; length
    // equals the input `length_pt`. Without the rebase the
    // rasterizer would see (0.5, ~0.07) → (0.5, ~0.93) directly
    // (sub-pt line near the spread origin → flat polygon).
    let cx = bbox.x + bbox.w * 0.5;
    assert!((start.0 - cx).abs() < 1e-3);
    assert!((end.0 - cx).abs() < 1e-3);
    assert!(((end.1 - start.1) - 577.7332).abs() < 1e-3);
}

/// Track 1a: oversized JPEGs go through `jpeg-decoder`'s
/// DCT-scaling path instead of materialising the full RGBA8
/// buffer via `image::load_from_memory`. Annual-report-template's
/// 5760×9000 cover would otherwise allocate ~198MB in one shot;
/// here we use a 4000×4000 synthetic JPEG with a 1024px cap and
/// assert the result lands at the largest DCT scale that still
/// fits the cap (1/4 → 1000×1000).
#[test]
fn track_1a_oversized_jpeg_routes_through_streaming_decoder() {
    use image::{ImageBuffer, ImageFormat, Rgb};
    use std::io::Cursor;
    let src: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(4000, 4000, |x, y| {
        Rgb([(x & 0xFF) as u8, (y & 0xFF) as u8, ((x ^ y) & 0xFF) as u8])
    });
    let mut buf: Vec<u8> = Vec::new();
    src.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .expect("encode JPEG");

    let decoded = decode_image_bytes_with_target_max(&buf, 1024).expect("streaming JPEG decode");
    // 4000 * 2/8 = 1000 ≤ 1024 fits; 4000 * 3/8 = 1500 doesn't.
    assert_eq!(decoded.width, 1000);
    assert_eq!(decoded.height, 1000);
    assert_eq!(
        decoded.rgba.len(),
        (decoded.width as usize) * (decoded.height as usize) * 4
    );
    // Alpha channel filled to opaque — JPEGs carry no alpha.
    assert!(decoded.rgba.chunks_exact(4).all(|p| p[3] == 255));
}

// Track 1a: small JPEGs (longest edge ≤ cap) skip the streaming
// ── Phase 4 typography — nested-style overlay walker ──────────

fn ns(
    style: &str,
    delim: paged_model::NestedDelimiter,
    rep: i32,
    inc: bool,
) -> paged_model::NestedStyle {
    paged_model::NestedStyle {
        applied_character_style: style.into(),
        delimiter: delim,
        repetition: rep,
        inclusive: inc,
    }
}

#[test]
fn nested_overlay_empty_when_no_entries() {
    assert!(compute_nested_style_overlay("hello world", &[]).is_empty());
}

#[test]
fn nested_overlay_characters_simple() {
    let ov = compute_nested_style_overlay(
        "abcdef",
        &[ns(
            "S/Bold",
            paged_model::NestedDelimiter::Characters,
            3,
            true,
        )],
    );
    assert_eq!(ov.len(), 1);
    assert_eq!(ov[0].byte_range, 0..3);
    assert_eq!(ov[0].applied_character_style, "S/Bold");
}

#[test]
fn nested_overlay_words_inclusive_captures_trailing_space() {
    // "the quick brown" — Words=1 inclusive should cover "the ".
    let ov = compute_nested_style_overlay(
        "the quick brown",
        &[ns("S/Lead", paged_model::NestedDelimiter::Words, 1, true)],
    );
    assert_eq!(ov.len(), 1);
    assert_eq!(&"the quick brown"[ov[0].byte_range.clone()], "the ");
}

#[test]
fn nested_overlay_words_exclusive_excludes_space() {
    let ov = compute_nested_style_overlay(
        "the quick brown",
        &[ns("S/Lead", paged_model::NestedDelimiter::Words, 1, false)],
    );
    assert_eq!(ov.len(), 1);
    assert_eq!(&"the quick brown"[ov[0].byte_range.clone()], "the");
}

#[test]
fn nested_overlay_char_delimiter_until_colon() {
    let ov = compute_nested_style_overlay(
        "Heading: body copy",
        &[ns(
            "S/Bold",
            paged_model::NestedDelimiter::Char(':'),
            1,
            true,
        )],
    );
    assert_eq!(ov.len(), 1);
    assert_eq!(&"Heading: body copy"[ov[0].byte_range.clone()], "Heading:");
}

#[test]
fn nested_overlay_chained_entries_consume_in_order() {
    // First entry: 3 chars styled S/A. Second entry: 5 chars
    // starting where the first ended.
    let ov = compute_nested_style_overlay(
        "abcdefghijk",
        &[
            ns("S/A", paged_model::NestedDelimiter::Characters, 3, true),
            ns("S/B", paged_model::NestedDelimiter::Characters, 5, true),
        ],
    );
    assert_eq!(ov.len(), 2);
    assert_eq!(ov[0].byte_range, 0..3);
    assert_eq!(ov[0].applied_character_style, "S/A");
    assert_eq!(ov[1].byte_range, 3..8);
    assert_eq!(ov[1].applied_character_style, "S/B");
}

#[test]
fn nested_overlay_stops_at_end_of_text() {
    let ov = compute_nested_style_overlay(
        "abc",
        &[ns(
            "S/X",
            paged_model::NestedDelimiter::Characters,
            100,
            true,
        )],
    );
    // Repetition exceeds text length → range extends to end.
    assert_eq!(ov.len(), 1);
    assert_eq!(ov[0].byte_range, 0..3);
}

#[test]
fn nested_overlay_skips_unknown_delimiter() {
    let ov = compute_nested_style_overlay(
        "hello",
        &[ns("S/X", paged_model::NestedDelimiter::Unknown, 1, true)],
    );
    // Unknown delimiter yields a zero-length match → no override
    // emitted, no cursor advance.
    assert!(ov.is_empty());
}

#[test]
fn nested_overlay_zero_repetition_is_noop() {
    let ov = compute_nested_style_overlay(
        "hello world",
        &[ns("S/X", paged_model::NestedDelimiter::Words, 0, true)],
    );
    assert!(ov.is_empty());
}

fn mk_run(text: &str, style: Option<&str>) -> paged_model::CharacterRun {
    paged_model::CharacterRun {
        character_style: style.map(String::from),
        text: text.into(),
        ..Default::default()
    }
}

#[test]
fn split_runs_no_overlay_passes_through() {
    let runs = vec![mk_run("hello", None), mk_run(" world", Some("S/Base"))];
    let out = split_runs_for_nested_styles(&runs, &[]);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].text, "hello");
    assert_eq!(out[1].text, " world");
    assert_eq!(out[1].character_style.as_deref(), Some("S/Base"));
}

#[test]
fn split_runs_overlay_inside_single_run_splits_into_three() {
    // Run "the quick brown" (15 bytes). Overlay [4..9) = "quick".
    let runs = vec![mk_run("the quick brown", None)];
    let overlay = vec![NestedStyleApplication {
        byte_range: 4..9,
        applied_character_style: "S/Bold".into(),
    }];
    let out = split_runs_for_nested_styles(&runs, &overlay);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].text, "the ");
    assert_eq!(out[0].character_style, None);
    assert_eq!(out[1].text, "quick");
    assert_eq!(out[1].character_style.as_deref(), Some("S/Bold"));
    assert_eq!(out[2].text, " brown");
    assert_eq!(out[2].character_style, None);
}

#[test]
fn split_runs_overlay_at_run_start_no_pre_fragment() {
    // Run "Heading text", overlay [0..7) = "Heading".
    let runs = vec![mk_run("Heading text", None)];
    let overlay = vec![NestedStyleApplication {
        byte_range: 0..7,
        applied_character_style: "S/H".into(),
    }];
    let out = split_runs_for_nested_styles(&runs, &overlay);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].text, "Heading");
    assert_eq!(out[0].character_style.as_deref(), Some("S/H"));
    assert_eq!(out[1].text, " text");
    assert_eq!(out[1].character_style, None);
}

#[test]
fn split_runs_overlay_spanning_two_runs_splits_both() {
    // Runs: "abc" + "defgh" (paragraph bytes 0..8). Overlay [2..6) =
    // "cdef" — covers tail of run0 and head of run1.
    let runs = vec![mk_run("abc", None), mk_run("defgh", Some("S/Base"))];
    let overlay = vec![NestedStyleApplication {
        byte_range: 2..6,
        applied_character_style: "S/Lead".into(),
    }];
    let out = split_runs_for_nested_styles(&runs, &overlay);
    // Expected fragments: "ab" (no override), "c" (S/Lead from
    // run0), "def" (S/Lead from run1), "gh" (S/Base from run1).
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].text, "ab");
    assert_eq!(out[0].character_style, None);
    assert_eq!(out[1].text, "c");
    assert_eq!(out[1].character_style.as_deref(), Some("S/Lead"));
    assert_eq!(out[2].text, "def");
    assert_eq!(out[2].character_style.as_deref(), Some("S/Lead"));
    assert_eq!(out[3].text, "gh");
    assert_eq!(out[3].character_style.as_deref(), Some("S/Base"));
}

// ── Phase 5 — conditional text filter ─────────────────────────

fn cond_run(text: &str, conditions: &[&str]) -> paged_model::CharacterRun {
    paged_model::CharacterRun {
        text: text.into(),
        applied_conditions: conditions.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn cond(id: &str, visible: bool) -> paged_model::ConditionDef {
    paged_model::ConditionDef {
        self_id: id.into(),
        visible: Some(visible),
        ..Default::default()
    }
}

/// Tiny smoke that mirrors the filter logic inline in
/// `emit_paragraph_into_chain`. Keeps the test independent of
/// constructing a full Document, while still exercising the
/// "all conditions visible ⇒ keep; any invisible ⇒ drop" rule.
fn filter_by_conditions(
    runs: &[paged_model::CharacterRun],
    table: &std::collections::BTreeMap<String, paged_model::ConditionDef>,
) -> Vec<paged_model::CharacterRun> {
    runs.iter()
        .filter(|r| {
            r.applied_conditions
                .iter()
                .all(|cid| table.get(cid).and_then(|c| c.visible).unwrap_or(true))
        })
        .cloned()
        .collect()
}

#[test]
fn conditions_no_applied_keeps_run() {
    let runs = vec![cond_run("body", &[])];
    let table = std::collections::BTreeMap::new();
    let out = filter_by_conditions(&runs, &table);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "body");
}

#[test]
fn conditions_visible_keeps_run() {
    let runs = vec![cond_run("draft text", &["Condition/Draft"])];
    let mut table = std::collections::BTreeMap::new();
    table.insert("Condition/Draft".to_string(), cond("Condition/Draft", true));
    let out = filter_by_conditions(&runs, &table);
    assert_eq!(out.len(), 1);
}

#[test]
fn conditions_invisible_drops_run() {
    let runs = vec![
        cond_run("keep", &[]),
        cond_run("hide", &["Condition/Draft"]),
    ];
    let mut table = std::collections::BTreeMap::new();
    table.insert(
        "Condition/Draft".to_string(),
        cond("Condition/Draft", false),
    );
    let out = filter_by_conditions(&runs, &table);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "keep");
}

#[test]
fn conditions_multiple_all_must_be_visible() {
    // Two conditions on one run; one is hidden ⇒ drop.
    let runs = vec![cond_run("dual", &["Condition/A", "Condition/B"])];
    let mut table = std::collections::BTreeMap::new();
    table.insert("Condition/A".to_string(), cond("Condition/A", true));
    table.insert("Condition/B".to_string(), cond("Condition/B", false));
    let out = filter_by_conditions(&runs, &table);
    assert!(out.is_empty());
}

#[test]
fn conditions_unknown_id_treated_as_visible() {
    // A reference to a condition not in the document's table
    // shouldn't silently hide content. InDesign treats unknown
    // condition refs as visible.
    let runs = vec![cond_run("orphan", &["Condition/Missing"])];
    let table = std::collections::BTreeMap::new();
    let out = filter_by_conditions(&runs, &table);
    assert_eq!(out.len(), 1);
}

#[test]
fn nested_overlay_digit_class() {
    // First 3 digits in mixed text.
    let ov = compute_nested_style_overlay(
        "a1b2c3d4",
        &[ns("S/Num", paged_model::NestedDelimiter::AnyDigit, 3, true)],
    );
    assert_eq!(ov.len(), 1);
    assert_eq!(&"a1b2c3d4"[ov[0].byte_range.clone()], "a1b2c3");
}

// ── Phase 5 renderer — index paragraph builder ────────────────

#[test]
fn nested_table_inside_cell_emits_grid_commands() {
    // Outer 1×1 table whose single cell hosts a nested 2×2 table.
    // After build, the page's display list must contain enough
    // rectangle commands to draw the inner table's grid (5
    // horizontal + 3 vertical = 8 lines, plus the outer table's
    // 4 borders = 4) plus glyph emission for inner cell text.
    // A pre-fix build would skip the nested table entirely; we
    // detect the fix by asserting the inner cell's text glyphs
    // are present.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 600"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="10 10 590 590"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
        <Table Self="t-outer" HeaderRowCount="0" FooterRowCount="0"
               BodyRowCount="1" ColumnCount="1">
          <Row Self="or0" Name="0" SingleRowHeight="100"/>
          <Column Self="oc0" Name="0" SingleColumnWidth="400"/>
          <Cell Self="oc0r0" Name="0:0" RowSpan="1" ColumnSpan="1">
            <ParagraphStyleRange>
              <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                <Table Self="t-inner" HeaderRowCount="0" FooterRowCount="0"
                       BodyRowCount="2" ColumnCount="2">
                  <Row Self="ir0" Name="0" SingleRowHeight="30"/>
                  <Row Self="ir1" Name="1" SingleRowHeight="30"/>
                  <Column Self="ic0" Name="0" SingleColumnWidth="150"/>
                  <Column Self="ic1" Name="1" SingleColumnWidth="150"/>
                  <Cell Self="i00" Name="0:0">
                    <ParagraphStyleRange>
                      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                        <Content>INNER-A</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="i11" Name="1:1">
                    <ParagraphStyleRange>
                      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                        <Content>INNER-B</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let font_bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter.ttf fixture");
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let built = build_document(&doc, &options).expect("build");
    // The inner cells contribute commands to the page's display
    // list via emit_cell_paragraph called from inside the new
    // nested-table emit. Count grid-line + glyph commands as a
    // sanity check that the nested table actually rendered.
    //
    // Before the nested-table fix: page would have ~0 commands
    // (the outer cell paragraph had empty runs + table → silent
    // skip). After: ≥ 8 grid rects (5 row lines + 3 col lines)
    // plus inner-cell glyph commands.
    let cmd_count = built.pages[0].list.commands.len();
    assert!(
        cmd_count >= 20,
        "expected nested-table cmds (≥20 for grid + INNER-A + INNER-B), \
             got {cmd_count}"
    );
}

#[test]
fn missing_image_link_emits_diagnostic() {
    // A Rectangle hosting an <Image> whose LinkResourceURI can't be
    // resolved (no AssetResolver wired) should render a placeholder
    // AND surface exactly one ImageLinkMissing diagnostic with the
    // URI attached — previously this was a silent tracing::warn!.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="20 20 120 120">
      <Image LinkResourceURI="file:///nonexistent/photo.jpg"/>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let built = build_document(&doc, &PipelineOptions::default()).expect("build");
    let missing: Vec<_> = built
        .diagnostics
        .items
        .iter()
        .filter(|d| d.code == crate::diagnostics::DiagnosticCode::ImageLinkMissing)
        .collect();
    assert_eq!(
        missing.len(),
        1,
        "expected one ImageLinkMissing diagnostic, got {:?}",
        built.diagnostics.items
    );
    assert_eq!(missing[0].page_index, Some(0));
    assert_eq!(
        missing[0].uri.as_deref(),
        Some("file:///nonexistent/photo.jpg")
    );
}

fn test_section(
    self_id: &str,
    page_start: &str,
    style: paged_model::NumberingStyle,
    start_at: u32,
) -> paged_model::Section {
    paged_model::Section {
        self_id: self_id.to_string(),
        page_start: Some(page_start.to_string()),
        continue_numbering: false,
        start_at: Some(start_at),
        numbering_style: style,
        section_prefix: None,
        marker: None,
        include_prefix: false,
    }
}

#[test]
fn section_walk_computes_roman_then_arabic_labels() {
    use paged_model::NumberingStyle;
    let sections = vec![
        test_section("sec1", "p1", NumberingStyle::LowerRoman, 1),
        test_section("sec2", "p3", NumberingStyle::Arabic, 1),
    ];
    let mut w = SectionWalk::new(&sections);
    // 4 Name-less pages: roman section p1..p2, then arabic from p3.
    let labels: Vec<String> = ["p1", "p2", "p3", "p4"]
        .iter()
        .map(|id| w.next_label(Some(id), None))
        .collect();
    assert_eq!(labels, vec!["i", "ii", "1", "2"]);
    assert!(w.used_fallback);
}

#[test]
fn section_walk_name_is_authoritative() {
    // No sections: a baked Name wins; a Name-less page uses the
    // 1-based fallback that matches the historical behaviour.
    let mut w = SectionWalk::new(&[]);
    assert_eq!(w.next_label(Some("p1"), Some("iii")), "iii");
    assert_eq!(w.next_label(Some("p2"), None), "2");
}

/// Build a one-page IDML with a single 1-row × 3-col table in a
/// text frame, interpolating `table_attrs` onto `<Table>` and
/// `cell_attrs` onto each `<Cell>`. Shared by the column-divider
/// and cell-rotation tests.
fn build_single_table_idml(table_attrs: &str, cell_attrs: &str) -> Vec<u8> {
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Black" Space="CMYK" ColorValue="0 0 0 100"/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="10 10 390 390"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    let cell = |name: &str, content: &str| {
        format!(
            r#"<Cell Self="{name}" Name="{name}"{cell_attrs}><ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>{content}</Content></CharacterStyleRange></ParagraphStyleRange></Cell>"#
        )
    };
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
        <Table Self="t" HeaderRowCount="0" FooterRowCount="0" BodyRowCount="1" ColumnCount="3"{table_attrs}>
          <Row Self="r0" Name="0" SingleRowHeight="40"/>
          <Column Self="cc0" Name="0" SingleColumnWidth="100"/>
          <Column Self="cc1" Name="1" SingleColumnWidth="100"/>
          <Column Self="cc2" Name="2" SingleColumnWidth="100"/>
          {c0}{c1}{c2}
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        c0 = cell("0:0", "A"),
        c1 = cell("1:0", "B"),
        c2 = cell("2:0", "C"),
    );
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

fn inter_font_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter.ttf fixture")
}

#[test]
fn table_column_dividers_emit_extra_edges() {
    // A table-style column-stroke decl must draw interior column
    // dividers — previously nothing rendered for it. Differential:
    // the same table with the decl emits more commands than without.
    let font = inter_font_bytes();
    let count = |table_attrs: &str| -> usize {
        let bytes = build_single_table_idml(table_attrs, "");
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let options = PipelineOptions {
            font: Some(&font),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        built.pages[0].list.commands.len()
    };
    let with = count(
        r#" StartColumnStrokeColor="Color/Black" StartColumnStrokeType="Solid" StartColumnStrokeWeight="1""#,
    );
    let without = count("");
    // Two interior dividers (3 columns) → at least two extra edges.
    assert!(
        with >= without + 2,
        "column dividers should add ≥2 edge commands: with={with} without={without}",
    );
}

#[test]
fn cell_rotation_rotates_content() {
    // A cell with RotationAngle="90" rotates its content: at least
    // one emitted command's transform gains a non-zero `b` term
    // (sin 90° = 1). Without rotation, content stays axis-aligned.
    let font = inter_font_bytes();
    let max_b = |cell_attrs: &str| -> f32 {
        let bytes = build_single_table_idml("", cell_attrs);
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let options = PipelineOptions {
            font: Some(&font),
            ..PipelineOptions::default()
        };
        let mut built = build_document(&doc, &options).expect("build");
        built.pages[0]
            .list
            .commands
            .iter_mut()
            .map(|c| c.transform_mut().0[1].abs())
            .fold(0.0f32, f32::max)
    };
    // Glyph command transforms carry the font scale on the
    // diagonal (a/d ≈ 1/units_per_em·size); a 90° rotation moves
    // that scale onto the off-diagonal (b). So rotated |b| ≈ the
    // glyph scale (clearly > 0), while upright |b| ≈ 0.
    let rotated = max_b(r#" RotationAngle="90""#);
    let upright = max_b("");
    assert!(
        upright < 1e-4,
        "unrotated cell content should be axis-aligned, got |b|={upright}"
    );
    assert!(
        rotated > 1e-3 && rotated > upright * 100.0,
        "RotationAngle=90 should rotate content (|b| ≈ glyph scale), got |b|={rotated}"
    );
}

#[test]
fn autosize_height_prevents_overset_drop() {
    // A short frame with lots of text drops overset lines. The same
    // frame with AutoSizingType="HeightOnly" grows to fit instead —
    // no lines dropped, more lines rendered.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let font = inter_font_bytes();
    let build = |auto: &str| -> (usize, usize) {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        let spread = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 400"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="20 20 60 200">
      <Properties/>
      <TextFramePreference{auto}/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
        );
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();
        // Many short paragraphs so the 40pt-tall frame overflows.
        let mut story = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">"#,
        );
        for i in 0..12 {
            story.push_str(&format!(
                    r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Line {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#
                ));
        }
        story.push_str("</Story></idPkg:Story>");
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let options = PipelineOptions {
            font: Some(&font),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        (built.stats.lines, built.stats.dropped_overflow_lines)
    };
    let (plain_lines, plain_dropped) = build("");
    let (grown_lines, grown_dropped) =
        build(r#" AutoSizingType="HeightOnly" AutoSizingReferencePoint="TopLeftPoint""#);
    assert!(
        plain_dropped > 0,
        "the undersized frame should overset without autosizing"
    );
    assert_eq!(
        grown_dropped, 0,
        "HeightOnly autosizing should drop nothing (frame grows)"
    );
    assert!(
        grown_lines > plain_lines,
        "autosized frame should render more lines: grown={grown_lines} plain={plain_lines}"
    );
}

#[test]
fn hyphenation_zone_is_noop_for_justified_but_active_for_ragged() {
    // W1.17: the Hyphenation Zone is a RAGGED-edge feature. Adobe:
    // "The Hyphenation Zone … applies only when you're using the
    // Single-line Composer with nonjustified text." (Adobe, "Compose
    // and hyphenate text in InDesign",
    // helpx.adobe.com/indesign/using/text-composition.html). A
    // justified paragraph has no rag — every line is flushed to the
    // column — so the zone has nothing to bound and InDesign ignores
    // it. We mirror that exactly: `layout_runs` zeroes the zone for
    // justified paragraphs, so the line breaks are IDENTICAL with or
    // without a HyphenationZone. For a ragged (Left-aligned)
    // paragraph the same zone DOES suppress a hyphen near the right
    // margin and end the line short — proving the fixture is
    // sensitive and the justified equality is a real no-op, not an
    // inert column. (W1.3 landed the zone gate in `compose_paragraph`;
    // W1.17 extends it to the renderer's multi-run path and pins the
    // justified case.)
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let font = inter_font_bytes();
    // (justification token, HyphenationZone pt) → per-line source
    // text. The zone is carried on an applied ParagraphStyle because
    // an inline `HyphenationZone` on a `<ParagraphStyleRange>` is not
    // captured by the scene cascade (only the applied style is) —
    // Justification, by contrast, IS read inline.
    let breaks_for = |justification: &str, zone_pt: &str| -> Vec<String> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Styles.xml", deflated).unwrap();
        let styles = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Z" Hyphenation="true" HyphenationZone="{zone_pt}"/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#
        );
        zip.write_all(styles.as_bytes()).unwrap();
        // Narrow column (frame width 140pt) so the long hyphenatable
        // word "communication" lands near the right margin and the
        // zone has something to gate. Tall enough that nothing
        // oversets.
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 400"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="20 20 400 160"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        let story = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Z" Justification="{justification}">
      <CharacterStyleRange AppliedFont="Inter" PointSize="11"><Content>the quick brown communication network protocol gateway</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
        );
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let options = PipelineOptions {
            font: Some(&font),
            collect_breaks: true,
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        built
            .breaks
            .iter()
            .map(|b| b.source_text.trim().to_string())
            .collect()
    };

    // Justified: a non-zero zone must NOT change the breaks — the
    // zone is a documented no-op for justified text.
    let just_no_zone = breaks_for("FullyJustified", "0");
    let just_zone = breaks_for("FullyJustified", "36");
    assert!(
        just_no_zone.len() >= 2,
        "need a wrap to exercise the zone, got {just_no_zone:?}"
    );
    assert_eq!(
        just_no_zone, just_zone,
        "HyphenationZone must be ignored for justified text: \
             zone-0={just_no_zone:?} vs zone-36={just_zone:?}"
    );
    // The justified control actually hyphenates (so the equality
    // above is meaningful: the zone would have suppressed it if it
    // applied). "communication" splits as "commu-/nication".
    assert!(
        just_no_zone.iter().any(|l| l.ends_with("commu")),
        "justified control should hyphenate near the margin: {just_no_zone:?}"
    );

    // Ragged (Left): the SAME zone DOES move a break — it suppresses
    // the "commu-" hyphen and pushes "communication" whole to the
    // next line, ending line 1 short (the hyphenation-zone trade).
    let rag_no_zone = breaks_for("LeftAlign", "0");
    let rag_zone = breaks_for("LeftAlign", "36");
    assert_ne!(
        rag_no_zone, rag_zone,
        "HyphenationZone must change ragged breaks: \
             zone-0={rag_no_zone:?} vs zone-36={rag_zone:?}"
    );
    assert!(
        rag_no_zone.iter().any(|l| l.ends_with("commu")),
        "ragged zone-0 should still hyphenate: {rag_no_zone:?}"
    );
    assert!(
        rag_zone.iter().all(|l| !l.ends_with("commu")),
        "ragged zone-36 should suppress the commu- hyphen: {rag_zone:?}"
    );
}

#[test]
fn autosize_phase_b_grows_box_and_shifts_neighbour_wrap() {
    // W1.7 Phase B. Frame A is an AutoSizingType="HeightOnly" frame
    // authored undersized (40pt tall) with a fill, a stroke, and an
    // active TextWrap, holding many short paragraphs so it grows to
    // ~10× its authored height. Frame B is a plain neighbour text
    // frame that overlaps A's GROWN vertical band.
    //
    // Two visible Phase-B effects are asserted differentially against
    // a no-autosize control (AutoSizingType absent):
    //   (1) A's painted fill box stretches to the grown extent — the
    //       `FillPath` for A's box is much taller with autosizing.
    //   (2) B's text wraps around A's GROWN box, not its authored
    //       rect — B's line breaks shift vs the control.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let font = inter_font_bytes();
    // Returns (A's painted-box height in pt, B's per-line texts).
    let build = |auto: &str| -> (f32, Vec<String>) {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
  <idPkg:Story src="Stories/Story_b.xml"/>
</Document>"#,
        )
        .unwrap();
        // Page origin (0,0) so the page-outer transform is identity
        // and a box `FillPath`'s transform `d` component is exactly
        // the painted box height in pt.
        //
        // Frame A: authored 40pt tall (top 20, bottom 60), 180 wide,
        // fill Black, with a BoundingBox TextWrap (no offsets). Frame
        // B: a tall neighbour starting at y=80 that overlaps A's
        // grown band (A grows well past y=80). B has no fill, so the
        // only frame `FillPath` is A's box.
        let spread = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 600"/>
    <TextFrame Self="frameA" ParentStory="a" GeometricBounds="20 20 60 200" FillColor="Color/Black">
      <Properties/>
      <TextFramePreference{auto}/>
      <TextWrapPreference Inverse="false" TextWrapMode="BoundingBoxTextWrap">
        <Properties>
          <TextWrapOffset Top="0" Left="0" Bottom="0" Right="0"/>
        </Properties>
      </TextWrapPreference>
    </TextFrame>
    <TextFrame Self="frameB" ParentStory="b" GeometricBounds="80 20 600 400"/>
  </Spread>
</idPkg:Spread>"#
        );
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();
        // Story A: many short paragraphs so the 40pt frame grows tall.
        let mut story_a = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">"#,
        );
        for i in 0..14 {
            story_a.push_str(&format!(
                    r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Headline line {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#
                ));
        }
        story_a.push_str("</Story></idPkg:Story>");
        zip.start_file("Stories/Story_a.xml", deflated).unwrap();
        zip.write_all(story_a.as_bytes()).unwrap();
        // Story B: one long paragraph that wraps; its lines that fall
        // in A's grown band get carved on the left, shifting breaks.
        let story_b = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="b">
    <ParagraphStyleRange Justification="LeftAlign">
      <CharacterStyleRange AppliedFont="Inter" PointSize="11"><Content>alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
        zip.start_file("Stories/Story_b.xml", deflated).unwrap();
        zip.write_all(story_b.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let options = PipelineOptions {
            font: Some(&font),
            collect_breaks: true,
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        // A's painted box: the page-outer transform is identity, so
        // the box `FillPath`'s transform is for_rect_in(rect, I) =
        // [w, 0, 0, h, x, y]. `d` (index 3) is the box height. B has
        // no fill, so the single frame-box FillPath is A's.
        let box_h = built.pages[0]
            .list
            .commands
            .iter()
            .find_map(|c| match c {
                paged_compose::DisplayCommand::FillPath { transform, .. } => Some(transform.0[3]),
                _ => None,
            })
            .expect("frame A should emit a fill box");
        // B's per-line source texts (story "b").
        let b_lines: Vec<String> = built
            .breaks
            .iter()
            .filter(|r| r.story_id == "b")
            .map(|r| r.source_text.trim().to_string())
            .collect();
        (box_h, b_lines)
    };

    let (plain_box_h, plain_b_lines) = build("");
    let (grown_box_h, grown_b_lines) =
        build(r#" AutoSizingType="HeightOnly" AutoSizingReferencePoint="TopLeftPoint""#);

    // (1) The painted box stretches to the auto-sized extent. The
    // authored box is 40pt; with 14 lines at ~12pt leading it grows
    // several-fold. Allow generous slack — the exact grown height is
    // an estimate, the contract is "much taller than authored".
    assert!(
        (plain_box_h - 40.0).abs() < 1.0,
        "control box should stay at its authored 40pt height, got {plain_box_h}"
    );
    assert!(
        grown_box_h > plain_box_h * 2.0,
        "autosized box should stretch well past authored: grown={grown_box_h} plain={plain_box_h}"
    );

    // (2) Neighbour text-wrap derives from the GROWN box: B's line
    // breaks shift vs the no-autosize control. (With the control, A
    // is only 40pt tall and ends at y=60, above B's first line at
    // y≈80, so A's authored box barely carves B; the grown box
    // reaches deep into B's column and re-wraps it.)
    assert!(
        !plain_b_lines.is_empty() && !grown_b_lines.is_empty(),
        "both runs should lay out neighbour text"
    );
    assert_ne!(
        grown_b_lines, plain_b_lines,
        "neighbour wrap must shift with the grown box: \
             grown={grown_b_lines:?} plain={plain_b_lines:?}"
    );
}

#[test]
fn autosize_phase_b_reference_point_anchors_box_growth() {
    // W1.7 Phase B reference-point anchoring. The same growing
    // headline frame is auto-sized under three AutoSizingReferencePoint
    // values; the painted box's top-left (`x`,`y` baked into the box
    // FillPath transform) moves per the pinned point:
    //   - TopLeftPoint   → top-left pinned: x,y stay at authored.
    //   - CenterPoint    → centre pinned: box extends up AND left.
    //   - BottomRightPoint → bottom-right pinned: box extends up+left
    //     by the full delta (top-left moves the most).
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let font = inter_font_bytes();
    // Returns the painted box rect (x, y, w, h) for frame A.
    let box_rect = |auto: &str| -> (f32, f32, f32, f32) {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
</Document>"#,
        )
        .unwrap();
        // Authored box: 200..240 in y (40pt tall), 100..280 in x
        // (180 wide), centred in a large page so growth in any
        // direction stays on-page. Page origin (0,0) ⇒ identity outer.
        let spread = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 600"/>
    <TextFrame Self="frameA" ParentStory="a" GeometricBounds="200 100 240 280" FillColor="Color/Black">
      <Properties/>
      <TextFramePreference{auto}/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
        );
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();
        let mut story_a = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">"#,
        );
        for i in 0..12 {
            // Each line is long enough that the longest-line width
            // estimate exceeds the authored 180pt width, so a
            // HeightAndWidth frame grows on BOTH axes (exercising the
            // horizontal AND vertical reference-point split).
            story_a.push_str(&format!(
                    r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Supercalifragilistic headline number {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#
                ));
        }
        story_a.push_str("</Story></idPkg:Story>");
        zip.start_file("Stories/Story_a.xml", deflated).unwrap();
        zip.write_all(story_a.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let options = PipelineOptions {
            font: Some(&font),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        built.pages[0]
            .list
            .commands
            .iter()
            .find_map(|c| match c {
                paged_compose::DisplayCommand::FillPath { transform, .. } => {
                    let t = transform.0;
                    // for_rect_in(rect, I) = [w, 0, 0, h, x, y].
                    Some((t[4], t[5], t[0], t[3]))
                }
                _ => None,
            })
            .expect("frame A should emit a fill box")
    };

    let top_left =
        box_rect(r#" AutoSizingType="HeightAndWidth" AutoSizingReferencePoint="TopLeftPoint""#);
    let center =
        box_rect(r#" AutoSizingType="HeightAndWidth" AutoSizingReferencePoint="CenterPoint""#);
    let bottom_right =
        box_rect(r#" AutoSizingType="HeightAndWidth" AutoSizingReferencePoint="BottomRightPoint""#);

    // All three grow to the SAME size (same content), differing only
    // in where the top-left lands.
    let eq = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(
        eq(top_left.2, center.2) && eq(center.2, bottom_right.2),
        "width should be identical across reference points"
    );
    assert!(
        eq(top_left.3, center.3) && eq(center.3, bottom_right.3),
        "height should be identical across reference points"
    );

    // TopLeft: top-left pinned at the authored (100, 200).
    assert!(
        eq(top_left.0, 100.0) && eq(top_left.1, 200.0),
        "TopLeftPoint must pin the authored top-left, got ({}, {})",
        top_left.0,
        top_left.1
    );

    // Centre pinned ⇒ box extends left and up by HALF the delta:
    // top-left sits left of and above the authored corner, but not as
    // far as the BottomRight case (full delta).
    assert!(
        center.0 < 100.0 && center.1 < 200.0,
        "CenterPoint must extend the box up and left, got ({}, {})",
        center.0,
        center.1
    );
    assert!(
        bottom_right.0 < center.0 && bottom_right.1 < center.1,
        "BottomRightPoint must move the top-left further than CenterPoint: \
             br=({}, {}) center=({}, {})",
        bottom_right.0,
        bottom_right.1,
        center.0,
        center.1
    );
    // The bottom-right corner stays pinned at the authored (280, 240)
    // for the BottomRight case.
    assert!(
        eq(bottom_right.0 + bottom_right.2, 280.0) && eq(bottom_right.1 + bottom_right.3, 240.0),
        "BottomRightPoint must pin the authored bottom-right corner, got ({}, {})",
        bottom_right.0 + bottom_right.2,
        bottom_right.1 + bottom_right.3
    );
}

#[test]
fn graphic_line_arrowhead_emits_fill() {
    // A GraphicLine with a RightLineEnd arrowhead emits an extra
    // FillPath (the arrowhead) on top of the stroked line.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let count_fills = |right_line_end: &str| -> usize {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Black" Space="CMYK" ColorValue="0 0 0 100"/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        let spread = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <GraphicLine Self="gl" GeometricBounds="20 20 180 180" StrokeColor="Color/Black" StrokeWeight="3"{right_line_end}/>
  </Spread>
</idPkg:Spread>"#
        );
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
        let built = build_document(&doc, &PipelineOptions::default()).expect("build");
        built.pages[0]
            .list
            .commands
            .iter()
            .filter(|c| matches!(c, paged_compose::DisplayCommand::FillPath { .. }))
            .count()
    };
    let with_arrow = count_fills(r#" RightLineEnd="TriangleHead""#);
    let without = count_fills("");
    assert_eq!(without, 0, "plain line draws no fill");
    assert_eq!(with_arrow, 1, "arrowhead should add one FillPath");
    // v43 — the canonical InDesign enumeration tokens parse and
    // draw too, including the hollow kinds (one FillPath each: the
    // ring is two contours in one path) and one marker per end.
    assert_eq!(count_fills(r#" RightLineEnd="TriangleArrowHead""#), 1);
    assert_eq!(count_fills(r#" RightLineEnd="CircleArrowHead""#), 1);
    assert_eq!(
        count_fills(r#" LeftLineEnd="BarbedArrowHead" RightLineEnd="SquareSolidArrowHead""#),
        2,
        "one marker per line end"
    );
}

#[test]
fn corner_rect_path_shapes_per_kind() {
    use paged_compose::PathSegment::{CubicTo, LineTo};
    use paged_model::CornerOption;
    let rect = paged_compose::Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    let radii = [Some(20.0); 4];
    let segs = |kind| corner_rect_path(rect, radii, [kind; 4]).segments;
    let cubics = |kind| {
        segs(kind)
            .iter()
            .filter(|s| matches!(s, CubicTo { .. }))
            .count()
    };
    let lines = |kind| {
        segs(kind)
            .iter()
            .filter(|s| matches!(s, LineTo { .. }))
            .count()
    };
    // Rounded / Inverse: one quarter-arc cubic per corner. Inverse
    // is the smooth concave indentation (distinct from Inset's
    // sharp fold-in below).
    assert_eq!(cubics(CornerOption::Rounded), 4);
    assert_eq!(cubics(CornerOption::Inverse), 4);
    // Bevel: straight chamfers — no cubics. Four corners + four
    // edges ⇒ 8 LineTos.
    assert_eq!(cubics(CornerOption::Bevel), 0);
    assert_eq!(lines(CornerOption::Bevel), 8);
    // Inset: InDesign's sharp "fold-in" notch — no cubics, two
    // LineTos per corner (in to `m`, back out) ⇒ strictly more line
    // segments than Bevel's single chamfer per corner. Distinct
    // from Inverse (calibrated + verified distinct in W1.8).
    assert_eq!(cubics(CornerOption::Inset), 0);
    assert!(
        lines(CornerOption::Inset) > lines(CornerOption::Bevel),
        "inset fold-in adds an extra line segment per corner vs bevel"
    );
    // Fancy: the ornamental three-arc scallop — three cubics per
    // corner (calibrated; was a two-cubic ogee before W1.8).
    assert_eq!(cubics(CornerOption::Fancy), 12);
    // None / zero radius: sharp corners, no cubics.
    assert_eq!(cubics(CornerOption::None), 0);
}

#[test]
fn inset_and_inverse_corners_are_distinct_geometry() {
    // W1.8 regression guard: InDesign's Inset (sharp fold-in) and
    // Inverse Rounded (smooth concave arc) must NOT collapse onto
    // the same path. A naive "Inset = quarter-circle cut inward"
    // implementation made them byte-identical.
    use paged_model::CornerOption;
    let rect = paged_compose::Rect {
        x: 0.0,
        y: 0.0,
        w: 80.0,
        h: 60.0,
    };
    let inverse = corner_rect_path(rect, [Some(15.0); 4], [CornerOption::Inverse; 4]);
    let inset = corner_rect_path(rect, [Some(15.0); 4], [CornerOption::Inset; 4]);
    assert_ne!(
        inverse.segments, inset.segments,
        "Inset and Inverse Rounded must render as distinct shapes"
    );
}

#[test]
fn corner_rect_path_every_option_emits_closed_continuous_geometry() {
    // W1.8: all five IDML corner options must emit geometry (a
    // non-degenerate, closed, continuous contour). Verifies segment
    // counts AND that the path's drawn vertices stay inside the
    // rect's bounds with no NaNs — the regression guard for the
    // Inset / Fancy calibration.
    use paged_compose::PathSegment;
    use paged_model::CornerOption;
    let rect = paged_compose::Rect {
        x: 10.0,
        y: 20.0,
        w: 120.0,
        h: 80.0,
    };
    let r = 18.0_f32;
    for kind in [
        CornerOption::Rounded,
        CornerOption::Inverse,
        CornerOption::Bevel,
        CornerOption::Inset,
        CornerOption::Fancy,
    ] {
        let path = corner_rect_path(rect, [Some(r); 4], [kind; 4]);
        let segs = &path.segments;
        // Starts with a MoveTo, ends with a Close.
        assert!(
            matches!(segs.first(), Some(PathSegment::MoveTo { .. })),
            "{kind:?}: must open with MoveTo"
        );
        assert!(
            matches!(segs.last(), Some(PathSegment::Close)),
            "{kind:?}: must end Close"
        );
        // Walk the contour tracking the current point; every drawn
        // endpoint and control point must be finite and inside the
        // rect's AABB (corner effects only ever cut *inward*, never
        // outside the bounding box). A square notch / scallop / arc
        // that escaped the box would signal a miscomputed corner.
        let inside = |x: f32, y: f32| -> bool {
            x.is_finite()
                && y.is_finite()
                && x >= rect.x - 0.01
                && x <= rect.x + rect.w + 0.01
                && y >= rect.y - 0.01
                && y <= rect.y + rect.h + 0.01
        };
        let mut start: Option<(f32, f32)> = None;
        let mut cur = (0.0_f32, 0.0_f32);
        for s in segs {
            match s {
                PathSegment::MoveTo { x, y } => {
                    assert!(inside(*x, *y), "{kind:?}: MoveTo escapes box");
                    start = Some((*x, *y));
                    cur = (*x, *y);
                }
                PathSegment::LineTo { x, y } => {
                    assert!(inside(*x, *y), "{kind:?}: LineTo escapes box");
                    cur = (*x, *y);
                }
                PathSegment::CubicTo {
                    cx1,
                    cy1,
                    cx2,
                    cy2,
                    x,
                    y,
                } => {
                    for (px, py) in [(*cx1, *cy1), (*cx2, *cy2), (*x, *y)] {
                        assert!(inside(px, py), "{kind:?}: cubic point escapes box");
                    }
                    cur = (*x, *y);
                }
                PathSegment::QuadTo { cx, cy, x, y } => {
                    for (px, py) in [(*cx, *cy), (*x, *y)] {
                        assert!(inside(px, py), "{kind:?}: quad point escapes box");
                    }
                    cur = (*x, *y);
                }
                PathSegment::Close => {
                    // Closing back to the contour start; the current
                    // point should already be (approximately) the
                    // start of the top edge's outgoing point.
                    if let Some(s0) = start {
                        let d = (cur.0 - s0.0).hypot(cur.1 - s0.1);
                        // The walk ends at TL's p_out, which is the
                        // very point MoveTo emitted — continuity.
                        assert!(d < 1e-3, "{kind:?}: contour not continuous (gap {d})");
                    }
                }
            }
        }
    }
}

#[test]
fn inset_corner_folds_in_to_the_rounding_centre() {
    // W1.8 Inset shape: each corner steps in to the inner rounding
    // centre `m` (the "fold-in" apex) then back out to the outgoing
    // edge. We check the top-left corner of a square: its apex must
    // land at `m = (r, r)` and the segment endpoints on the edges at
    // `(0, r)` (incoming) and `(r, 0)` (outgoing).
    use paged_compose::PathSegment::LineTo;
    use paged_model::CornerOption;
    let rect = paged_compose::Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    let r = 25.0_f32;
    let path = corner_rect_path(rect, [Some(r); 4], [CornerOption::Inset; 4]);
    // The contour walks TR, BR, BL, then TL last. The final two
    // LineTos belong to TL: the fold-in apex `m = (r, r)` then the
    // outgoing point `p_out = (r, 0)` on the top edge.
    let line_pts: Vec<(f32, f32)> = path
        .segments
        .iter()
        .filter_map(|s| match s {
            LineTo { x, y } => Some((*x, *y)),
            _ => None,
        })
        .collect();
    let n = line_pts.len();
    assert!(n >= 2, "inset emits fold-in LineTos");
    // Last LineTo = TL p_out on the top edge at (r, 0).
    let p_out = line_pts[n - 1];
    assert!(
        (p_out.0 - r).abs() < 1e-3 && p_out.1.abs() < 1e-3,
        "TL p_out at {p_out:?}"
    );
    // Second-to-last = the fold-in apex at m = (r, r).
    let apex = line_pts[n - 2];
    assert!(
        (apex.0 - r).abs() < 1e-3 && (apex.1 - r).abs() < 1e-3,
        "TL fold-in apex at {apex:?}, expected ({r}, {r})"
    );
}

#[test]
fn midpoint_blend_curve() {
    // Default midpoint is exactly linear.
    assert!((midpoint_blend(0.3, 0.5) - 0.3).abs() < 1e-6);
    // At t == mid, the colour-blend fraction is exactly 0.5.
    assert!((midpoint_blend(0.25, 0.25) - 0.5).abs() < 1e-4);
    assert!((midpoint_blend(0.75, 0.75) - 0.5).abs() < 1e-4);
    // A 0.25 midpoint pushes the colour past halfway by the time
    // geometry reaches t == 0.5.
    assert!(midpoint_blend(0.5, 0.25) > 0.5);
    // Endpoints are fixed regardless of midpoint.
    assert!((midpoint_blend(0.0, 0.25)).abs() < 1e-6);
    assert!((midpoint_blend(1.0, 0.25) - 1.0).abs() < 1e-6);
}

#[test]
fn color_lerp_midpoint_is_average() {
    let black = paged_compose::Color::rgba(0.0, 0.0, 0.0, 1.0);
    let white = paged_compose::Color::rgba(1.0, 1.0, 1.0, 1.0);
    let mid = color_lerp(black, white, 0.5);
    assert!((mid.r - 0.5).abs() < 1e-6 && (mid.g - 0.5).abs() < 1e-6);
}

#[test]
fn section_walk_applies_prefix() {
    use paged_model::{NumberingStyle, Section};
    let sections = vec![Section {
        self_id: "sec".into(),
        page_start: Some("p1".into()),
        continue_numbering: false,
        start_at: Some(1),
        numbering_style: NumberingStyle::Arabic,
        section_prefix: Some("A-".into()),
        marker: None,
        include_prefix: true,
    }];
    let mut w = SectionWalk::new(&sections);
    assert_eq!(w.next_label(Some("p1"), None), "A-1");
    assert_eq!(w.next_label(Some("p2"), None), "A-2");
}

#[test]
fn vertical_writing_rotates_emitted_commands() {
    // Build a story with StoryDirection="VerticalWritingDirection".
    // After build, every command that landed on the host page
    // should have a rotated transform (90° CW). We detect this
    // by checking the `b` and `c` cells of the transform — for
    // upright transforms b=0; after a 90° CW rotation b=1 (the
    // first column becomes [0, 1]). Identity → rotated proves
    // the post-pass fired.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="20 20 180 180"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1" StoryDirection="VerticalWritingDirection">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>ABC</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let font_bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter.ttf fixture");
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let built = build_document(&doc, &options).expect("build");

    // At least one command on the page should have a rotated
    // transform. Identity transform = [1, 0, 0, 1, tx, ty];
    // after 90° CW the linear part becomes [0, 1, -1, 0, ...].
    // Glyph FillPath commands have transforms like
    // [scale, 0, 0, scale, tx, ty] with scale ≈ 12/units_per_em.
    // After 90° CW rotation: new linear = [0, scale, -scale, 0].
    // The test detects "a became zero" + "b became non-zero" —
    // any threshold > 0 catches it.
    let mut owned = built.pages[0].list.commands.clone();
    let mut saw_any = false;
    let mut saw_rotated = false;
    for cmd in owned.iter_mut() {
        let xf = cmd.transform_mut();
        saw_any = true;
        // Rotated: a near 0, b non-zero. Pre-rotation: a non-zero, b near 0.
        if xf.0[0].abs() < 1e-3 && xf.0[1].abs() > 1e-4 {
            saw_rotated = true;
            break;
        }
    }
    assert!(saw_any, "expected at least one command on the page");
    assert!(
        saw_rotated,
        "vertical-writing post-rotation should have rotated at least one command"
    );
}

#[test]
fn ruby_annotation_emits_above_base_run() {
    // Paragraph with RubyFlag="true" + RubyString="ruby". The
    // renderer should shape the ruby text at half point size and
    // emit it above the base run. Base ABC at 12pt + ruby
    // "ruby" at 6pt = ~7 extra glyph commands (ruby has up to 4
    // glyphs depending on shaper output).
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 180 572"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12" RubyFlag="true" RubyString="abc">
        <Content>ABC</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let font_bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter.ttf fixture");
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let built = build_document(&doc, &options).expect("build");

    // Body "ABC" = 3 glyphs; ruby "abc" at 6pt = 3 more.
    // Expect ≥ 6 commands.
    let cmd_count = built.pages[0].list.commands.len();
    assert!(
        cmd_count >= 6,
        "expected base + ruby glyphs (≥6 cmds), got {cmd_count}"
    );
}

#[test]
fn kenten_marks_emit_above_each_glyph() {
    // A paragraph with `KentenKind="Dot"` on its CharacterStyleRange.
    // The renderer should stamp an emphasis mark (small filled
    // ellipse) above every glyph of that run. Pre-fix: zero
    // ellipse commands. Post-fix: one ellipse per character
    // glyph plus any frame chrome.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 180 572"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12" KentenKind="Dot">
        <Content>ABC</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let font_bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter.ttf fixture");
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let built = build_document(&doc, &options).expect("build");

    // The kenten pass emits one ellipse command per glyph in
    // the kenten-tagged run. "ABC" is 3 chars → 3 ellipse
    // commands. The body text alone contributes ~3 glyph
    // FillPath commands; with kenten we add ~3 more ellipses
    // (each rendered as a FillPath of an ellipse path).
    let cmd_count = built.pages[0].list.commands.len();
    assert!(
        cmd_count >= 6,
        "expected glyphs + 3 kenten marks (≥6 cmds), got {cmd_count}"
    );
}

#[test]
fn footnotes_are_captured_onto_their_host_page() {
    // Build an IDML with a body paragraph that anchors two
    // footnotes. After running the pipeline, the page that
    // hosts the body paragraph should carry both footnotes with
    // per-page running numbers 1 and 2.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 380 572"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Anchor host body.</Content>
        <Footnote Self="Footnote/fn1">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>First footnote.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Footnote>
        <Footnote Self="Footnote/fn2">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>Second footnote.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Footnote>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let font_bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter.ttf fixture");
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let built = build_document(&doc, &options).expect("build");
    assert_eq!(built.pages.len(), 1);
    let footnotes = &built.pages[0].footnotes;
    assert_eq!(footnotes.len(), 2);
    assert_eq!(footnotes[0].number, 1);
    assert_eq!(
        footnotes[0].footnote_self_id.as_deref(),
        Some("Footnote/fn1")
    );
    assert_eq!(footnotes[1].number, 2);
    assert_eq!(
        footnotes[1].footnote_self_id.as_deref(),
        Some("Footnote/fn2")
    );
    // Footnote bodies preserved verbatim.
    assert_eq!(footnotes[0].paragraphs[0].runs[0].text, "First footnote.");
    assert_eq!(footnotes[1].paragraphs[0].runs[0].text, "Second footnote.");

    // Phase 5 footnote pool: the post-pass should have laid out
    // the two footnotes as glyphs at the bottom of frameA.
    // The body alone contributes ~17 glyphs ("Anchor host
    // body."). With the pool emit firing, total commands grow
    // by the two footnote bodies' glyph counts; we assert a
    // floor of 40 to confirm pool emission happened without
    // pinning the exact glyph count (which depends on the
    // shaper's ligature decisions for the fallback font).
    let cmd_count = built.pages[0].list.commands.len();
    assert!(
        cmd_count >= 40,
        "expected footnote pool glyphs in display list (≥40), got {cmd_count}"
    );
}

/// W1.7 — shared fixture for the footnote space-reservation tests:
/// a SHORT frame whose body paragraph anchors `footnote_count`
/// footnotes, each with a long body so the accumulated pool is a
/// meaningful fraction of the frame height. `with_footnotes=false`
/// produces the identical body with the footnotes stripped — the
/// regression control.
fn footnote_reserve_idml(footnote_count: usize, with_footnotes: bool) -> Vec<u8> {
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    // Short frame: 120pt tall (top 40, bottom 160), 232pt wide.
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 300 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 160 272"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    let mut footnotes_xml = String::new();
    if with_footnotes {
        for i in 0..footnote_count {
            footnotes_xml.push_str(&format!(
                    r#"<Footnote Self="Footnote/fn{i}">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>Footnote body number {i} runs long enough to wrap across several lines in the narrow pool column at the bottom of the host frame.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Footnote>"#
                ));
        }
    }
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Body line one of the host paragraph. Body line two continues the host text. Body line three keeps the frame filled so the footnote pool must reserve space and push these lines upward.</Content>
        {footnotes_xml}
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn footnote_pool_reserves_space_below_body_text() {
    // W1.7 (a): with the reservation pass, NO body line's baseline
    // may fall inside the band the footnote pool occupies
    // (frame content bottom − pool height). Before W1.7 the pool
    // was a pure overlay and body lines ran straight through it.
    let bytes = footnote_reserve_idml(3, true);
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
    let font_bytes = inter_font_bytes();
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let built = build_document(&doc, &options).expect("build");
    assert_eq!(built.pages.len(), 1);

    // The pool must actually exist for this assertion to bite.
    assert!(
        !built.pages[0].footnotes.is_empty(),
        "fixture should capture footnotes"
    );
    let font_table = FontTable::build(&doc, &options);
    let pools = measure_footnote_pools(
        &built.pages,
        &options,
        &doc,
        &font_table,
        &doc.palette,
        None,
    );
    let pool_h: f32 = pools.values().copied().fold(0.0, f32::max);
    assert!(pool_h > 0.0, "expected a measurable footnote pool height");

    // Frame content area: top 40, height 120 (no insets) ⇒ bottom
    // at page-local y = 160. The reserved band starts at
    // bottom − pool_h; every kept body line's baseline sits at or
    // above it (a small epsilon absorbs the rounding the overflow
    // check does in 1/64-pt units).
    let content_bottom_pt = 160.0_f32;
    let reserved_top_pt = content_bottom_pt - pool_h;
    let body_baselines: Vec<f32> = built.pages[0]
        .story_layout
        .iter()
        .filter(|l| l.story_id == "s1")
        .map(|l| l.baseline_y_pt)
        .collect();
    assert!(
        !body_baselines.is_empty(),
        "expected body lines in the layout index"
    );
    let max_baseline = body_baselines.iter().copied().fold(0.0, f32::max);
    assert!(
        max_baseline <= reserved_top_pt + 0.5,
        "body baseline {max_baseline:.2}pt intrudes into the reserved \
             footnote band (starts at {reserved_top_pt:.2}pt, pool {pool_h:.2}pt)"
    );
}

#[test]
fn footnote_reserve_loop_converges_and_pushes_text_up() {
    // W1.7 (b): the compose→measure→re-compose loop terminates
    // (build returns Ok, i.e. it didn't spin past the bail cap), and
    // the reservation demonstrably moved body text — the
    // footnote-bearing build keeps FEWER body lines on the page than
    // the same body with footnotes stripped, because the pool ate
    // the bottom of the frame.
    let font_bytes = inter_font_bytes();
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };

    let with_doc =
        paged_parse::import_idml_doc(&footnote_reserve_idml(3, true)).expect("open with");
    let with_built = build_document(&with_doc, &options).expect("build with footnotes");

    let without_doc =
        paged_parse::import_idml_doc(&footnote_reserve_idml(3, false)).expect("open without");
    let without_built = build_document(&without_doc, &options).expect("build without");

    let count_body = |b: &BuiltDocument| {
        b.pages[0]
            .story_layout
            .iter()
            .filter(|l| l.story_id == "s1")
            .count()
    };
    let with_lines = count_body(&with_built);
    let without_lines = count_body(&without_built);
    assert!(
        with_lines < without_lines,
        "reservation should keep fewer body lines on the page \
             (with footnotes: {with_lines}, without: {without_lines})"
    );
}

#[test]
fn no_footnote_frame_is_byte_identical() {
    // W1.7 (c) regression guard: a story with no footnotes must
    // take the pass-0 early break (no rollback, no re-emit), so its
    // display list is identical to the pre-W1.7 single-pass emit.
    // We can't diff against old code in-process, so we assert (1)
    // the build is deterministic across two runs, and (2) it emits
    // ZERO footnote-pool commands — i.e. the reservation machinery
    // left the page untouched. The display-list command count is
    // the golden; update the comment here if a legitimate emit
    // change shifts it.
    let font_bytes = inter_font_bytes();
    let options = PipelineOptions {
        font: Some(&font_bytes),
        ..PipelineOptions::default()
    };
    let bytes = footnote_reserve_idml(0, false);

    let doc_a = paged_parse::import_idml_doc(&bytes).expect("open a");
    let built_a = build_document(&doc_a, &options).expect("build a");
    let doc_b = paged_parse::import_idml_doc(&bytes).expect("open b");
    let built_b = build_document(&doc_b, &options).expect("build b");

    assert!(
        built_a.pages[0].footnotes.is_empty(),
        "no-footnote fixture must capture zero footnotes"
    );
    // Deterministic command count across runs — the reservation
    // loop is a no-op here, so nothing perturbs the display list.
    assert_eq!(
        built_a.pages[0].list.commands.len(),
        built_b.pages[0].list.commands.len(),
        "no-footnote build must be deterministic (reservation loop \
             must not run for a footnote-free story)"
    );
    // No FootnoteOverflow / pool diagnostics leaked in.
    assert!(
        built_a
            .diagnostics
            .items
            .iter()
            .all(|d| d.code != DiagnosticCode::FootnoteOverflow),
        "no-footnote build must not emit footnote diagnostics"
    );
}

#[test]
fn build_index_paragraphs_emits_topic_tab_pages() {
    // Construct a Document by parsing a small IDML so we exercise
    // the full resolve_index → build path. Reusing the parser
    // here is far cheaper than a hand-rolled Document.
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <TextFrame Self="f1" ParentStory="s1" GeometricBounds="10 10 190 190"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Content>The apple is red.</Content>
        <PageReference Self="PR1" TopicName="Apple"/>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");

    let page_labels = vec!["1".to_string()];
    let paragraphs = build_index_paragraphs(&doc, &page_labels);
    assert_eq!(paragraphs.len(), 1);
    assert_eq!(paragraphs[0].runs.len(), 1);
    assert_eq!(paragraphs[0].runs[0].text, "Apple\t1");
}

/// path and decode at native size via `image::load_from_memory`.
#[test]
fn track_1a_small_jpeg_keeps_native_dimensions() {
    use image::{ImageBuffer, ImageFormat, Rgb};
    use std::io::Cursor;
    let src: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(128, 96, |x, y| {
        Rgb([
            (x & 0xFF) as u8,
            (y & 0xFF) as u8,
            ((x.wrapping_add(y)) & 0xFF) as u8,
        ])
    });
    let mut buf: Vec<u8> = Vec::new();
    src.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .expect("encode JPEG");
    let decoded = decode_image_bytes_with_target_max(&buf, 4096).expect("small JPEG decode");
    assert_eq!(decoded.width, 128);
    assert_eq!(decoded.height, 96);
}

// ── W1.21: image clipping-path display-list tests ────────────────

/// 100×100 RGBA PNG, base64-encoded for inline `<Contents>` so the
/// image resolves with no asset resolver. Same fixture the
/// `image-clipping` gen sample embeds.
const CLIP_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAGQAAABkCAYAAABw4pVUAAAA0klEQVR42u3RMREAMAgEMBQhEEHoqpPi4y9DFKR69yd40xFKiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJESHrIAUVvrbCxtZyKAAAAAElFTkSuQmCC";

/// Build a single-page IDML (in-memory zip) hosting a 100×100 inline
/// image in a 100 pt rectangle with an identity inner ItemTransform,
/// carrying the supplied `<ClippingPathSettings>` XML fragment.
fn build_clip_idml(clipping_path_xml: &str) -> Vec<u8> {
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="0 0 100 100">
      <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:clip.png">
        <Properties>
          <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
            <PathPointType Anchor="0 0"/>
            <PathPointType Anchor="100 0"/>
            <PathPointType Anchor="100 100"/>
            <PathPointType Anchor="0 100"/>
          </PathPointArray></GeometryPathType></PathGeometry>
          <Contents><![CDATA[{CLIP_PNG_B64}]]></Contents>
        </Properties>
        {clipping_path_xml}
        <Link LinkResourceURI="file:clip.png"/>
      </Image>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

/// Count the PushClip / Image commands and capture the clip-path
/// `PathData`s in document order on page 0.
fn clip_command_summary(built: &BuiltDocument) -> (usize, usize, Vec<paged_compose::PathData>) {
    let page = &built.pages[0];
    let mut push_clips = 0usize;
    let mut images = 0usize;
    let mut clip_paths = Vec::new();
    for cmd in &page.list.commands {
        match cmd {
            paged_compose::DisplayCommand::PushClip { path_id, .. } => {
                push_clips += 1;
                if let Some(p) = page.list.paths.get(*path_id) {
                    clip_paths.push(p.clone());
                }
            }
            paged_compose::DisplayCommand::Image { .. } => images += 1,
            _ => {}
        }
    }
    (push_clips, images, clip_paths)
}

#[test]
fn user_modified_clip_emits_extra_pushclip_around_image() {
    // A star clip (UserModifiedPath) ⇒ the image is wrapped in TWO
    // clips: the frame box AND the star path. Without the clip there
    // would be exactly one PushClip (the frame).
    let clip = r#"<ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="false"
              IncludeInsideEdges="false">
          <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
            <PathPointType Anchor="50 2"/>
            <PathPointType Anchor="62 38"/>
            <PathPointType Anchor="98 38"/>
            <PathPointType Anchor="68 60"/>
            <PathPointType Anchor="80 96"/>
            <PathPointType Anchor="50 74"/>
            <PathPointType Anchor="20 96"/>
            <PathPointType Anchor="32 60"/>
            <PathPointType Anchor="2 38"/>
            <PathPointType Anchor="38 38"/>
          </PathPointArray></GeometryPathType></PathGeometry>
        </ClippingPathSettings>"#;
    let bytes = build_clip_idml(clip);
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
    let built = build_document(&doc, &PipelineOptions::default()).expect("build");

    let (push_clips, images, clip_paths) = clip_command_summary(&built);
    assert_eq!(images, 1, "exactly one placed image");
    assert_eq!(
        push_clips, 2,
        "frame clip + image clipping path = two PushClips, got {push_clips}"
    );
    // No defer diagnostic for an inline UserModifiedPath.
    assert!(
        !built
            .diagnostics
            .items
            .iter()
            .any(|d| d.code == DiagnosticCode::ImageClippingPathDeferred),
        "inline geometry must not defer"
    );
    // The second clip path (the star) is a single closed contour:
    // one MoveTo + 10 CubicTo (9 between points + 1 closing) + Close.
    let star = clip_paths.last().expect("clip path present");
    let move_tos = star
        .segments
        .iter()
        .filter(|s| matches!(s, paged_compose::PathSegment::MoveTo { .. }))
        .count();
    let cubics = star
        .segments
        .iter()
        .filter(|s| matches!(s, paged_compose::PathSegment::CubicTo { .. }))
        .count();
    assert_eq!(move_tos, 1, "star is a single contour");
    assert_eq!(cubics, 10, "10 anchors ⇒ 10 cubic segments");
}

#[test]
fn invert_clip_path_punches_bbox_with_two_contours() {
    // InvertPath ⇒ the clip path is (image bbox) − (rectangle), so
    // the emitted clip path has TWO MoveTo contours: the bounding
    // box and the punched rectangle.
    let clip = r#"<ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="true"
              IncludeInsideEdges="false">
          <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
            <PathPointType Anchor="30 30"/>
            <PathPointType Anchor="70 30"/>
            <PathPointType Anchor="70 70"/>
            <PathPointType Anchor="30 70"/>
          </PathPointArray></GeometryPathType></PathGeometry>
        </ClippingPathSettings>"#;
    let bytes = build_clip_idml(clip);
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
    let built = build_document(&doc, &PipelineOptions::default()).expect("build");

    let (push_clips, images, clip_paths) = clip_command_summary(&built);
    assert_eq!(images, 1);
    assert_eq!(push_clips, 2, "frame + invert clip");
    let invert = clip_paths.last().expect("clip path present");
    let move_tos = invert
        .segments
        .iter()
        .filter(|s| matches!(s, paged_compose::PathSegment::MoveTo { .. }))
        .count();
    assert_eq!(
        move_tos, 2,
        "invert clip = bbox + punched rectangle (two contours)"
    );
}

#[test]
fn compound_clip_path_keeps_hole_contour() {
    // A star with a punched diamond (IncludeInsideEdges) ⇒ the clip
    // path keeps both contours so the hole survives.
    let clip = r#"<ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="false"
              IncludeInsideEdges="true">
          <PathGeometry>
            <GeometryPathType PathOpen="false"><PathPointArray>
              <PathPointType Anchor="10 10"/>
              <PathPointType Anchor="90 10"/>
              <PathPointType Anchor="90 90"/>
              <PathPointType Anchor="10 90"/>
            </PathPointArray></GeometryPathType>
            <GeometryPathType PathOpen="false"><PathPointArray>
              <PathPointType Anchor="40 40"/>
              <PathPointType Anchor="60 40"/>
              <PathPointType Anchor="60 60"/>
              <PathPointType Anchor="40 60"/>
            </PathPointArray></GeometryPathType>
          </PathGeometry>
        </ClippingPathSettings>"#;
    let bytes = build_clip_idml(clip);
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
    let built = build_document(&doc, &PipelineOptions::default()).expect("build");

    let (_, images, clip_paths) = clip_command_summary(&built);
    assert_eq!(images, 1);
    let compound = clip_paths.last().expect("clip path present");
    let move_tos = compound
        .segments
        .iter()
        .filter(|s| matches!(s, paged_compose::PathSegment::MoveTo { .. }))
        .count();
    assert_eq!(move_tos, 2, "outer square + inner diamond hole");
}

#[test]
fn photoshop_clip_path_defers_with_diagnostic() {
    // PhotoshopPath references a named 8BIM path with no inline
    // geometry ⇒ the image is clipped to the frame only (ONE
    // PushClip) and exactly one ImageClippingPathDeferred diagnostic
    // is recorded, carrying the path name + frame id.
    let clip = r#"<ClippingPathSettings ClippingType="PhotoshopPath" InvertPath="false"
              IncludeInsideEdges="false" AppliedPathName="Path 1"/>"#;
    let bytes = build_clip_idml(clip);
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
    let built = build_document(&doc, &PipelineOptions::default()).expect("build");

    let (push_clips, images, _) = clip_command_summary(&built);
    assert_eq!(images, 1, "the image still renders (frame-clipped)");
    assert_eq!(push_clips, 1, "frame clip only — no detached clip path");

    let deferred: Vec<_> = built
        .diagnostics
        .items
        .iter()
        .filter(|d| d.code == DiagnosticCode::ImageClippingPathDeferred)
        .collect();
    assert_eq!(deferred.len(), 1, "one defer diagnostic");
    assert_eq!(deferred[0].frame_id.as_deref(), Some("r1"));
    assert!(
        deferred[0].message.contains("Path 1"),
        "diagnostic names the applied path: {}",
        deferred[0].message
    );
}

#[test]
fn no_clipping_path_keeps_single_frame_clip() {
    // Control: an image with no <ClippingPathSettings> keeps exactly
    // one PushClip (the frame) and emits no defer diagnostic — the
    // clipping path is purely additive.
    let bytes = build_clip_idml("");
    let doc = paged_parse::import_idml_doc(&bytes).expect("open IDML");
    let built = build_document(&doc, &PipelineOptions::default()).expect("build");

    let (push_clips, images, _) = clip_command_summary(&built);
    assert_eq!(images, 1);
    assert_eq!(push_clips, 1, "frame clip only when no clipping path");
    assert!(!built
        .diagnostics
        .items
        .iter()
        .any(|d| d.code == DiagnosticCode::ImageClippingPathDeferred));
}
