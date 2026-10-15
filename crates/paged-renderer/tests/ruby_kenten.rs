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

//! Ruby and emphasis marks set where, and as big as, InDesign 20.0.1 sets
//! them by default (`tools/indesign-export/ruby-kenten-probe`, read from
//! its PDF).
//!
//! InDesign's sample is Kozuka Mincho at 30 pt under the Japanese
//! Paragraph Composer. Noto Sans JP stands in for it here: its em box is
//! Kozuka's (`BASE` ideographic baseline -120), and its kanji and kana
//! are a full em wide as Kozuka's are, so every position InDesign
//! measured holds for it. Positions are page x and offsets from the base
//! line's baseline: the probe's frames start at x 60.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One frame at x 60, 440 pt wide, holding one paragraph under the
/// Japanese Paragraph Composer: `runs` are `(attributes, text)` in
/// `family` at 30 pt.
fn build(family: &str, runs: &[(&str, &str)]) -> paged_renderer::BuiltPage {
    let runs: String = runs
        .iter()
        .map(|(attrs, text)| {
            format!(
                r#"<CharacterStyleRange AppliedFont="{family}" FontStyle="Regular" PointSize="30" {attrs}><Content>{text}</Content></CharacterStyleRange>"#
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
    for (family, file) in [
        ("Noto Sans JP", "NotoSansJP-VF.ttf"),
        ("Open Sans", "OpenSans.ttf"),
    ] {
        resolver.add_font(family, None, std::fs::read(fonts.join(file)).unwrap());
    }
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

/// What the page draws: glyphs as `(size, x, y, vertical size)` and
/// marks as `(width, centre x, centre y, height)`, in drawing order (a
/// glyph's size is its horizontal one, its scale folded in).
struct Drawn {
    glyphs: Vec<(f32, f32, f32, f32)>,
    marks: Vec<(f32, f32, f32, f32)>,
}

fn drawn(page: &paged_renderer::BuiltPage, upem: f32) -> Drawn {
    let mut d = Drawn {
        glyphs: Vec::new(),
        marks: Vec::new(),
    };
    for c in &page.list.commands {
        let DisplayCommand::FillPath { transform, .. } = c else {
            continue;
        };
        let [a, b, c2, dd, tx, ty] = transform.0;
        if b.abs() > 1e-5 || c2.abs() > 1e-5 || a <= 0.0 {
            continue;
        }
        if dd < 0.0 {
            d.glyphs.push((a * upem, tx, ty, -dd * upem));
        } else {
            d.marks.push((a, tx, ty, dd));
        }
    }
    d
}

impl Drawn {
    /// The glyphs of one size, left to right.
    fn at_size(&self, size: f32) -> Vec<(f32, f32)> {
        let mut v: Vec<(f32, f32)> = self
            .glyphs
            .iter()
            .filter(|g| (g.0 - size).abs() < 0.01)
            .map(|g| (g.1, g.2))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }
}

fn assert_near(got: f32, want: f32, what: &str) {
    assert!((got - want).abs() < 0.02, "{what}: {got}, InDesign {want}");
}

const KANJI: &str = "漢字漢字";

/// The 30 pt base, its ruby at 15 pt: InDesign's ruby x and the ruby
/// baseline 28.2 pt above the base's (0.88 × 30 + 0.12 × 15).
fn assert_ruby(page: &paged_renderer::BuiltPage, base_x: &[f32], ruby_x: &[f32]) {
    let d = drawn(page, 1000.0);
    let base = d.at_size(30.0);
    let ruby = d.at_size(15.0);
    assert_eq!(base.len(), base_x.len(), "{base:?}");
    assert_eq!(ruby.len(), ruby_x.len(), "{ruby:?}");
    for (g, &x) in base.iter().zip(base_x) {
        assert_near(g.0, x, "base character");
    }
    for (g, &x) in ruby.iter().zip(ruby_x) {
        assert_near(g.0, x, "ruby character");
        assert_near(base[0].1 - g.1, 28.2, "ruby baseline above the base's");
    }
}

#[test]
fn group_ruby_spreads_over_its_base_one_two_one() {
    // かんじ (45 pt) over 漢字 (60 pt): 2.5 pt before, 5 between, 2.5 after.
    let page = build(
        "Noto Sans JP",
        &[
            ("", KANJI),
            (
                r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじ""#,
                "漢字",
            ),
        ],
    );
    assert_ruby(
        &page,
        &[60.0, 90.0, 120.0, 150.0, 180.0, 210.0],
        &[182.5, 202.5, 222.5],
    );
}

#[test]
fn per_character_ruby_puts_each_part_over_its_character() {
    // かん over 漢 fills it; じ over 字 is centred, 7.5 pt either side.
    let page = build(
        "Noto Sans JP",
        &[
            ("", KANJI),
            (
                r#"RubyFlag="1" RubyType="PerCharacterRuby" RubyString="かん じ""#,
                "漢字",
            ),
        ],
    );
    assert_ruby(
        &page,
        &[60.0, 90.0, 120.0, 150.0, 180.0, 210.0],
        &[180.0, 195.0, 217.5],
    );
    // No space to split it: the whole string over the first character,
    // which is spaced out to its 45 pt, the next one moving along.
    let page = build(
        "Noto Sans JP",
        &[
            ("", KANJI),
            (
                r#"RubyFlag="1" RubyType="PerCharacterRuby" RubyString="かんじ""#,
                "漢字",
            ),
        ],
    );
    assert_ruby(
        &page,
        &[60.0, 90.0, 120.0, 150.0, 187.5, 225.0],
        &[180.0, 195.0, 210.0],
    );
}

#[test]
fn ruby_wider_than_its_base_spaces_the_base_out() {
    // かんじかん (75 pt) over one 30 pt 漢: the character takes 75 pt,
    // centred in it, and the rest of the line moves 45 pt along.
    let page = build(
        "Noto Sans JP",
        &[
            ("", "漢字"),
            (
                r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじかん""#,
                "漢",
            ),
            ("", "字漢字"),
        ],
    );
    assert_ruby(
        &page,
        &[60.0, 90.0, 142.5, 195.0, 225.0, 255.0],
        &[120.0, 135.0, 150.0, 165.0, 180.0],
    );
}

#[test]
fn kenten_stand_on_the_em_box_at_half_the_size() {
    // A 15 pt mark centred on each character, its em box on the base's:
    // its centre 0.88 × 30 + 7.5 = 33.9 pt above the baseline.
    let page = build(
        "Noto Sans JP",
        &[(r#"KentenKind="KentenSesameDot""#, "漢字"), ("", KANJI)],
    );
    let d = drawn(&page, 1000.0);
    let baseline = d.at_size(30.0)[0].1;
    assert_eq!(d.marks.len(), 2, "one mark a character");
    for (mark, x) in d.marks.iter().zip([75.0, 105.0]) {
        assert_near(mark.0, 15.0, "mark size");
        assert_near(mark.1, x, "mark centre x");
        assert_near(baseline - mark.2, 33.9, "mark centre above the baseline");
    }
}

#[test]
fn a_latin_face_centres_its_em_box_on_half_its_cap_height() {
    // Open Sans has no BASE table: its em box is centred on half its
    // cap height (0.714 em), 0.857 em above the baseline. InDesign: the
    // kenten over A and B centred 33.207 pt above it, the group ruby xy
    // over CD at x 104.674 with its baseline 27.854 pt above.
    let page = build(
        "Open Sans",
        &[
            (r#"KentenKind="KentenSesameDot""#, "AB"),
            (r#"RubyFlag="1" RubyType="GroupRuby" RubyString="xy""#, "CD"),
            ("", "EF"),
        ],
    );
    let d = drawn(&page, 2048.0);
    let base = d.at_size(30.0);
    let ruby = d.at_size(15.0);
    let baseline = base[0].1;
    for (mark, x) in d.marks.iter().zip([69.485, 88.66]) {
        assert_near(mark.1, x, "mark centre x");
        assert_near(baseline - mark.2, 33.207, "mark centre above the baseline");
    }
    assert_eq!(ruby.len(), 2, "{ruby:?}");
    assert_near(ruby[0].0, 104.674, "ruby x");
    assert_near(
        baseline - ruby[0].1,
        27.854,
        "ruby baseline above the base's",
    );
}

/// One frame of Noto Sans JP 30 pt: "漢字漢字" then 漢字 with group ruby
/// かんじ and `ruby_attrs` — InDesign's probe frame, its ruby over x 180
/// to 240.
fn ruby_frame(ruby_attrs: &str) -> Drawn {
    let attrs = format!(r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじ" {ruby_attrs}"#);
    let page = build("Noto Sans JP", &[("", KANJI), (&attrs, "漢字")]);
    drawn(&page, 1000.0)
}

/// The ruby glyphs (`size` wide) as `(x, height above the base's
/// baseline)`.
fn ruby_of(d: &Drawn, size: f32) -> Vec<(f32, f32)> {
    let baseline = d.at_size(30.0)[0].1;
    d.at_size(size)
        .into_iter()
        .map(|(x, y)| (x, baseline - y))
        .collect()
}

fn assert_ruby_at(got: &[(f32, f32)], xs: &[f32], up: f32, what: &str) {
    assert_eq!(got.len(), xs.len(), "{what}: {got:?}");
    for (&(x, y), &want) in got.iter().zip(xs) {
        assert_near(x, want, what);
        assert_near(y, up, what);
    }
}

#[test]
fn ruby_takes_its_size_scale_offsets_and_position() {
    // InDesign 20.0.1 (ruby-kenten-probe), かんじ over 漢字 at x 180.
    // RubyFontSize 12: 36 pt of ruby spread 1:2:1, its em box on the
    // base's: 0.88 × 30 + 0.12 × 12 = 27.84 pt up.
    assert_ruby_at(
        &ruby_of(&ruby_frame(r#"RubyFontSize="12""#), 12.0),
        &[184.0, 204.0, 224.0],
        27.84,
        "RubyFontSize 12",
    );
    // RubyXScale 50: half-width glyphs, spread from their scaled width.
    assert_ruby_at(
        &ruby_of(&ruby_frame(r#"RubyXScale="50""#), 7.5),
        &[186.25, 206.25, 226.25],
        28.2,
        "RubyXScale 50",
    );
    // RubyYScale 200: twice as tall, grown up from the same baseline.
    let tall = ruby_frame(r#"RubyYScale="200""#);
    assert!(tall
        .glyphs
        .iter()
        .any(|g| (g.0 - 15.0).abs() < 0.01 && (g.3 - 30.0).abs() < 0.01));
    assert_ruby_at(
        &ruby_of(&tall, 15.0),
        &[182.5, 202.5, 222.5],
        28.2,
        "RubyYScale 200",
    );
    // RubyXOffset / RubyYOffset: right and up.
    assert_ruby_at(
        &ruby_of(&ruby_frame(r#"RubyXOffset="5""#), 15.0),
        &[187.5, 207.5, 227.5],
        28.2,
        "RubyXOffset 5",
    );
    assert_ruby_at(
        &ruby_of(&ruby_frame(r#"RubyYOffset="5""#), 15.0),
        &[182.5, 202.5, 222.5],
        33.2,
        "RubyYOffset 5",
    );
    // BelowLeft: the ruby's em box hangs from the base's, 16.8 pt down;
    // its offset still moves it up.
    assert_ruby_at(
        &ruby_of(&ruby_frame(r#"RubyPosition="BelowLeft""#), 15.0),
        &[182.5, 202.5, 222.5],
        -16.8,
        "BelowLeft",
    );
    assert_ruby_at(
        &ruby_of(
            &ruby_frame(r#"RubyPosition="BelowLeft" RubyYOffset="5""#),
            15.0,
        ),
        &[182.5, 202.5, 222.5],
        -11.8,
        "BelowLeft, RubyYOffset 5",
    );
}

#[test]
fn ruby_spreads_by_its_alignment() {
    // 15 pt to spare over 漢字 (x 180 to 240).
    for (alignment, xs) in [
        ("RubyLeft", [180.0, 195.0, 210.0]),
        ("RubyCenter", [187.5, 202.5, 217.5]),
        ("RubyRight", [195.0, 210.0, 225.0]),
        ("RubyFullJustify", [180.0, 202.5, 225.0]),
        ("RubyEqualAki", [183.75, 202.5, 221.25]),
        ("Ruby1Aki", [187.5, 202.5, 217.5]),
    ] {
        let d = ruby_frame(&format!(r#"RubyAlignment="{alignment}""#));
        assert_ruby_at(&ruby_of(&d, 15.0), &xs, 28.2, alignment);
    }
    // 75 pt to spare over four characters (x 90 to 210), where JIS, 1-aki
    // and equal aki part ways.
    for (alignment, xs) in [
        ("RubyJIS", [102.5, 142.5, 182.5]),
        ("Ruby1Aki", [105.0, 142.5, 180.0]),
        ("RubyEqualAki", [108.75, 142.5, 176.25]),
        ("RubyFullJustify", [90.0, 142.5, 195.0]),
    ] {
        let attrs = format!(
            r#"RubyFlag="1" RubyType="GroupRuby" RubyString="かんじ" RubyAlignment="{alignment}""#
        );
        let page = build(
            "Noto Sans JP",
            &[("", "漢"), (&attrs, "字漢字漢"), ("", "字")],
        );
        let d = drawn(&page, 1000.0);
        assert_ruby_at(&ruby_of(&d, 15.0), &xs, 28.2, alignment);
    }
}

/// One frame of Noto Sans JP 30 pt with sesame kenten and `attrs` on
/// its first two characters (x 60 to 120): the marks' `(width, centre x,
/// centre height above the baseline, height)`.
fn kenten_frame(attrs: &str) -> Vec<(f32, f32, f32, f32)> {
    let kenten = format!(r#"KentenKind="KentenSesameDot" {attrs}"#);
    let d = drawn(
        &build("Noto Sans JP", &[(&kenten, "漢字"), ("", KANJI)]),
        1000.0,
    );
    let baseline = d.at_size(30.0)[0].1;
    d.marks
        .iter()
        .map(|m| (m.0, m.1, baseline - m.2, m.3))
        .collect()
}

#[test]
fn kenten_take_their_size_scale_placement_position_and_alignment() {
    // InDesign 20.0.1 (ruby-kenten-probe): a mark's em box on the base's,
    // its centre half the mark above that (33.9 pt at 15 pt).
    let cases: [(&str, f32, f32, [f32; 2], f32); 7] = [
        // attrs, width, height, centres x, centre height
        (r#"KentenFontSize="12""#, 12.0, 12.0, [75.0, 105.0], 32.4),
        (r#"KentenXScale="50""#, 7.5, 15.0, [75.0, 105.0], 33.9),
        // A vertical scale grows the mark up from its baseline.
        (r#"KentenYScale="200""#, 15.0, 30.0, [75.0, 105.0], 39.6),
        (r#"KentenPlacement="5""#, 15.0, 15.0, [75.0, 105.0], 38.9),
        // Below: hanging from the base's em box, 16.8 pt down to its
        // baseline; KentenPlacement moves it up there too.
        (
            r#"KentenPosition="BelowLeft""#,
            15.0,
            15.0,
            [75.0, 105.0],
            -11.1,
        ),
        (
            r#"KentenPosition="BelowLeft" KentenPlacement="5""#,
            15.0,
            15.0,
            [75.0, 105.0],
            -6.1,
        ),
        // AlignKentenLeft: the mark's em box on the character's left.
        (
            r#"KentenAlignment="AlignKentenLeft""#,
            15.0,
            15.0,
            [67.5, 97.5],
            33.9,
        ),
    ];
    for (attrs, width, height, xs, up) in cases {
        let marks = kenten_frame(attrs);
        assert_eq!(marks.len(), 2, "{attrs}: {marks:?}");
        for (m, x) in marks.iter().zip(xs) {
            assert_near(m.0, width, attrs);
            assert_near(m.3, height, attrs);
            assert_near(m.1, x, attrs);
            assert_near(m.2, up, attrs);
        }
    }
}

#[test]
fn a_custom_kenten_prints_its_character_on_the_em_box() {
    // KentenKind="Custom": the character, half size, in the base's face,
    // its em box on the base's (InDesign: Kozuka's ★ 28.2 pt up, its
    // advance centred on the character).
    let page = build(
        "Noto Sans JP",
        &[
            (r#"KentenKind="Custom" KentenCustomCharacter="★""#, "漢字"),
            ("", KANJI),
        ],
    );
    let d = drawn(&page, 1000.0);
    let baseline = d.at_size(30.0)[0].1;
    let marks = d.at_size(15.0);
    assert_eq!(marks.len(), 2, "{marks:?}");
    for (x, y) in marks {
        assert_near(baseline - y, 28.2, "custom kenten baseline");
        assert!((60.0..120.0).contains(&x), "over its character: {x}");
    }
    assert!(d.marks.is_empty(), "no drawn mark for a custom kind");
}
