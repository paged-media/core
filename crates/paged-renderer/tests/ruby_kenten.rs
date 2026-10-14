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

/// What the page draws: glyphs as `(size, x, y)` and marks as
/// `(size, centre x, centre y)`, in drawing order.
struct Drawn {
    glyphs: Vec<(f32, f32, f32)>,
    marks: Vec<(f32, f32, f32)>,
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
            d.glyphs.push((a * upem, tx, ty));
        } else {
            d.marks.push((a, tx, ty));
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
