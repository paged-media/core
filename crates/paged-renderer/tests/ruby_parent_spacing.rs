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

//! Ruby wider than its base, set as InDesign 20.0.1 sets it
//! (`tools/indesign-export/ruby-parent-spacing-probe`, Kozuka Mincho at
//! 30 pt with 15 pt ruby, read from its PDF), and kana set at the
//! proportional widths Metrics kerning gives them (`kana-spacing-probe`).
//!
//! The ruby first overhangs the characters beside its base — kana, never
//! kanji, never past a line's start or end — by up to one ruby character
//! each side by default; what it still needs spaces the base out by
//! `RubyParentSpacing`. Noto Sans JP stands in for Kozuka: its kanji are a
//! full em, so every kanji position InDesign measured holds exactly. Its
//! kana take its own proportional widths, so kana cases are checked by
//! how the base sits against its neighbours.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One frame at x 60, 440 pt wide, one paragraph under the Japanese
/// Paragraph Composer: `runs` are `(attributes, text)` in Noto Sans JP at
/// 30 pt.
fn build(runs: &[(&str, &str)]) -> paged_renderer::BuiltPage {
    let runs: String = runs
        .iter()
        .map(|(attrs, text)| {
            format!(
                r#"<CharacterStyleRange AppliedFont="Noto Sans JP" FontStyle="Regular" PointSize="30" {attrs}><Content>{text}</Content></CharacterStyleRange>"#
            )
        })
        .collect();
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    let mut put = |path: &str, body: &str| {
        zip.start_file(path, deflated).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put(
        "designmap.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<idPkg:Spread src="Spreads/Spread_sp1.xml"/>
<idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    );
    put(
        "Spreads/Spread_sp1.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="sp1">
<Page Self="p1" GeometricBounds="0 0 841.89 595.276"/>
<TextFrame Self="tf1" ParentStory="s1" GeometricBounds="50 60 125 500"/>
</Spread>
</idPkg:Spread>"#,
    );
    put(
        "Stories/Story_s1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><Story Self="s1"><ParagraphStyleRange Composer="HL Composer J">{runs}</ParagraphStyleRange></Story></idPkg:Story>"#
        ),
    );
    let idml = zip.finish().unwrap().into_inner();
    let fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    let mut resolver = BytesResolver::new();
    resolver.add_font(
        "Noto Sans JP",
        None,
        std::fs::read(fonts.join("NotoSansJP-VF.ttf")).unwrap(),
    );
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let doc = idml_import::import_idml_doc(&idml).unwrap();
    pipeline::build_document(&doc, &opts)
        .unwrap()
        .pages
        .remove(0)
}

/// Glyph origins `(x, horizontal size)` of one vertical size, left to
/// right (a glyph's horizontal size carries its horizontal scale).
fn glyphs(page: &paged_renderer::BuiltPage, size: f32) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = page
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0),
            _ => None,
        })
        .filter(|t| t[1].abs() < 1e-5 && t[2].abs() < 1e-5 && t[3] < 0.0)
        .filter(|t| (-t[3] * 1000.0 - size).abs() < 0.01)
        .map(|t| (t[4], t[0] * 1000.0))
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

fn xs(page: &paged_renderer::BuiltPage, size: f32) -> Vec<f32> {
    glyphs(page, size).into_iter().map(|g| g.0).collect()
}

fn assert_xs(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: {got:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 0.02, "{what}: {got:?}, InDesign {want:?}");
    }
}

const RUBY6: &str = r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじかんじ""#;

/// `ruby` over the 3rd and 4th of "山川漢字木林" with `attrs` added.
fn kanji_case(attrs: &str) -> paged_renderer::BuiltPage {
    let ruby = format!("{RUBY6} {attrs}");
    build(&[("", "山川"), (&ruby, "漢字"), ("", "木林")])
}

/// Kanji beside the base: nothing to overhang, so each spacing mode shows
/// whole. かんじかんじ (90 pt) over 漢字 (60 pt) leaves 30 pt.
#[test]
fn each_parent_spacing_spaces_the_base_as_indesign_does() {
    for (spacing, base) in [
        ("", [127.5, 172.5, 210.0]),
        (
            r#"RubyParentSpacing="RubyParent121Aki""#,
            [127.5, 172.5, 210.0],
        ),
        (
            r#"RubyParentSpacing="RubyParentBothSides""#,
            [135.0, 165.0, 210.0],
        ),
        (
            r#"RubyParentSpacing="RubyParentEqualAki""#,
            [130.0, 170.0, 210.0],
        ),
        (
            r#"RubyParentSpacing="RubyParentFullJustify""#,
            [120.0, 180.0, 210.0],
        ),
    ] {
        let page = kanji_case(spacing);
        let all = xs(&page, 30.0);
        assert_xs(&all[2..5], &base, spacing);
        let ruby: Vec<f32> = (0..6).map(|i| 120.0 + 15.0 * i as f32).collect();
        assert_xs(&xs(&page, 15.0), &ruby, spacing);
    }
}

/// `RubyParentNoAdjustment`: the base stays solid and the ruby is
/// centred over it, overhanging the kanji beside it.
#[test]
fn no_adjustment_centres_the_ruby_over_a_solid_base() {
    let page = kanji_case(r#"RubyParentSpacing="RubyParentNoAdjustment""#);
    assert_xs(&xs(&page, 30.0)[2..5], &[120.0, 150.0, 180.0], "base");
    assert_xs(&xs(&page, 15.0)[..1], &[105.0], "ruby");
}

/// One base character: full justify, equal aki and both sides all put
/// half the room on each side.
#[test]
fn one_base_character_takes_the_room_on_both_sides() {
    for spacing in [
        "RubyParentFullJustify",
        "RubyParentEqualAki",
        "RubyParentBothSides",
    ] {
        let ruby = format!(
            r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじ" RubyParentSpacing="{spacing}""#
        );
        let page = build(&[("", "山川"), (&ruby, "漢"), ("", "木林")]);
        assert_xs(&xs(&page, 30.0)[2..4], &[127.5, 165.0], spacing);
    }
}

/// The ruby narrows before it spaces the base (`RubyAutoScaling`): to
/// the base's width when the least scale allows (66 % by default), else
/// to that least scale, and the rest spaces the base.
#[test]
fn auto_scaling_narrows_the_ruby_first() {
    let page = kanji_case(r#"RubyAutoScaling="true""#);
    assert_xs(
        &xs(&page, 30.0)[2..5],
        &[120.0, 150.0, 180.0],
        "fits at 2/3",
    );
    let ruby = glyphs(&page, 15.0);
    assert!(
        (ruby[0].1 - 10.0).abs() < 0.02,
        "ruby at 2/3 width: {ruby:?}"
    );
    assert_xs(&xs(&page, 15.0)[..2], &[120.0, 130.0], "ruby");

    let page = kanji_case(r#"RubyAutoScaling="true" RubyParentScalingPercent="80""#);
    assert_xs(
        &xs(&page, 30.0)[2..5],
        &[123.0, 159.0, 192.0],
        "80 % then 1-2-1",
    );
    assert!((glyphs(&page, 15.0)[0].1 - 12.0).abs() < 0.02);
}

/// Kana beside the base: the ruby overhangs each by up to one ruby
/// character, so a ruby 30 pt wider than its base leaves it solid, with
/// its first character a ruby em before the base. With no overhang, the
/// base is spaced 1-2-1 instead.
#[test]
fn ruby_overhangs_kana_beside_its_base() {
    let page = build(&[("", "あい"), (RUBY6, "漢字"), ("", "うえ")]);
    let base = xs(&page, 30.0);
    let ruby = xs(&page, 15.0);
    assert!(
        (base[3] - base[2] - 30.0).abs() < 0.02,
        "base solid: {base:?}"
    );
    assert!(
        (base[2] - 15.0 - ruby[0]).abs() < 0.02,
        "one ruby em over い: {ruby:?}"
    );

    let none = format!(r#"{RUBY6} RubyParentOverhangAmount="None""#);
    let page = build(&[("", "あい"), (&none, "漢字"), ("", "うえ")]);
    let base = xs(&page, 30.0);
    let ruby = xs(&page, 15.0);
    assert!(
        (base[3] - base[2] - 45.0).abs() < 0.02,
        "15 pt between: {base:?}"
    );
    assert!(
        (base[2] - 7.5 - ruby[0]).abs() < 0.02,
        "7.5 pt before: {ruby:?}"
    );
}

/// A kanji on one side, kana on the other: the kana side takes its
/// overhang, the rest is spaced 1-2-1 (3.75, 7.5, 3.75).
#[test]
fn only_the_kana_side_is_overhung() {
    let page = build(&[("", "山川"), (RUBY6, "漢字"), ("", "うえ")]);
    assert_xs(&xs(&page, 30.0)[2..4], &[123.75, 161.25], "base");
    assert_xs(
        &xs(&page, 15.0)[..1],
        &[120.0],
        "ruby from the kanji's edge",
    );
}

/// A base starting a line overhangs nothing before it and drops its share
/// before: the room left goes between (two shares) and after (one).
#[test]
fn a_base_starting_its_line_drops_its_share_before() {
    let page = build(&[(RUBY6, "漢字"), ("", "うえお")]);
    assert_xs(&xs(&page, 30.0)[..2], &[60.0, 100.0], "base");
    assert_xs(&xs(&page, 15.0)[..1], &[60.0], "ruby");
}

/// Ruby kana stand a full ruby em apart, while Metrics kerning sets body
/// kana at their proportional widths, and kerning off sets them full.
#[test]
fn kana_take_proportional_widths_under_metrics_kerning_ruby_does_not() {
    let metrics = build(&[("", "あいうえお")]);
    let off = build(&[(r#"KerningMethod="$ID/None""#, "あいうえお")]);
    // Proportional metrics set them as Metrics kerning does, kerning off.
    let proportional = build(&[(
        r#"KerningMethod="$ID/None" OTFProportionalMetrics="true""#,
        "あいうえお",
    )]);
    assert_xs(
        &xs(&proportional, 30.0),
        &xs(&metrics, 30.0),
        "OTFProportionalMetrics",
    );
    let full: Vec<f32> = (0..5).map(|i| 60.0 + 30.0 * i as f32).collect();
    assert_xs(&xs(&off, 30.0), &full, "kerning off");
    assert!(
        xs(&metrics, 30.0)
            .iter()
            .zip(&full)
            .any(|(g, f)| (g - f).abs() > 0.5),
        "Metrics kerning sets kana proportionally: {:?}",
        xs(&metrics, 30.0)
    );
    let page = kanji_case(r#"RubyParentOverhangAmount="None""#);
    let ruby = xs(&page, 15.0);
    for w in ruby.windows(2) {
        assert!(
            (w[1] - w[0] - 15.0).abs() < 0.02,
            "ruby full width: {ruby:?}"
        );
    }
}

/// A neighbour narrower than the overhang allowed is overhung by no more
/// than its width (InDesign's `latin` case: Kozuka's 14.34 pt `c` takes
/// that much of one ruby em), and what it leaves spaces the base 1-2-1.
/// `b`, wider than a ruby em, takes the whole em.
#[test]
fn a_neighbour_is_overhung_no_further_than_it_is_wide() {
    let i = {
        let page = build(&[("", "i山")]);
        let x = xs(&page, 30.0);
        x[1] - x[0]
    };
    assert!(i < 15.0, "Noto Sans JP's i is narrower than a ruby em: {i}");
    let page = build(&[("", "ab"), (RUBY6, "漢字"), ("", "i山")]);
    let base = xs(&page, 30.0);
    let ruby = xs(&page, 15.0);
    let rest = 15.0 - i;
    let start = ruby[0] + 15.0;
    assert_xs(
        &base[2..5],
        &[
            start + rest / 4.0,
            start + 30.0 + rest * 3.0 / 4.0,
            start + 60.0 + rest,
        ],
        "base",
    );
}

/// A kana is overhung by up to its whole em, however `palt` narrows it
/// (InDesign's `wide-onechar`: 30 pt over い): one base character of
/// overhang each side leaves a ruby 60 pt wider than its base solid.
#[test]
fn a_kana_is_overhung_by_its_whole_em() {
    let ruby8 = r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじかんじかん" RubyParentOverhangAmount="RubyOverhangOneChar""#;
    let page = build(&[("", "あい"), (ruby8, "漢字"), ("", "うえ")]);
    let base = xs(&page, 30.0);
    let ruby = xs(&page, 15.0);
    assert!(
        (base[3] - base[2] - 30.0).abs() < 0.02,
        "base solid: {base:?}"
    );
    assert!(
        (base[2] - 30.0 - ruby[0]).abs() < 0.02,
        "a whole em over い: {ruby:?}, base {base:?}"
    );
}
