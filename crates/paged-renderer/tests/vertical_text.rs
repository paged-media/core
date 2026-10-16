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

//! Vertical text (`StoryOrientation="Vertical"`), set as InDesign 20.0.1
//! sets it (`tools/indesign-export/vertical-text-probe`, Kozuka Mincho
//! 20 pt in a 200 × 300 pt frame, read from its PDF; Paged matched every
//! glyph to 0.02 pt): the horizontal layout of the frame turned on its
//! side, turned a quarter clockwise. Lines become columns from the right,
//! a column's length is the frame's height, and its first column's em box
//! stands where the first line's would against the frame's top. Kanji,
//! kana and CJK punctuation stand upright in their vertical forms; Latin
//! and digits turn with the column. Ruby and kenten sit to the right
//! (below: to the left), upright.
//!
//! Noto Sans JP stands in for Kozuka: its em box is Kozuka's and its
//! kanji and kana a full em, so with kerning off every position holds.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A frame at x 200..400, y 100..400 holding `runs` (`(attributes,
/// text)`) in Noto Sans JP 20 pt under the Japanese Paragraph Composer,
/// kerning off; `vertical` turns the story vertical. `frame` and `para`
/// add attributes to the frame's `<TextFramePreference>` and the
/// paragraph.
fn build(
    vertical: bool,
    frame: &str,
    para: &str,
    runs: &[(&str, &str)],
) -> paged_renderer::BuiltPage {
    let runs: String = runs
        .iter()
        .map(|(attrs, text)| {
            format!(
                r#"<CharacterStyleRange AppliedFont="Noto Sans JP" FontStyle="Regular" PointSize="20" KerningMethod="$ID/None" {attrs}><Content>{text}</Content></CharacterStyleRange>"#
            )
        })
        .collect();
    let orientation = if vertical { "Vertical" } else { "Horizontal" };
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
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="sp1">
<Page Self="p1" GeometricBounds="0 0 841.89 595.276"/>
<TextFrame Self="tf1" ParentStory="s1" GeometricBounds="100 200 400 400"><TextFramePreference {frame}/></TextFrame>
</Spread>
</idPkg:Spread>"#
        ),
    );
    put(
        "Stories/Story_s1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><Story Self="s1"><StoryPreference StoryOrientation="{orientation}"/><ParagraphStyleRange Composer="HL Composer J" {para}>{runs}</ParagraphStyleRange></Story></idPkg:Story>"#
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

/// A glyph as drawn: its transform's linear part and where it is drawn
/// from (pt, page space), in drawing order.
#[derive(Debug, Clone, Copy)]
struct Drawn {
    linear: [f32; 4],
    x: f32,
    y: f32,
}

impl Drawn {
    /// Drawn upright: scaled and flipped, not turned.
    fn upright(&self) -> bool {
        self.linear[1].abs() < 1e-5 && self.linear[2].abs() < 1e-5 && self.linear[0] > 0.0
    }
    /// Turned a quarter clockwise, as Latin is down a column.
    fn turned(&self) -> bool {
        self.linear[0].abs() < 1e-5 && self.linear[3].abs() < 1e-5 && self.linear[1] > 0.0
    }
    /// Its vertical size (pt): the font size it is drawn at.
    fn size(&self) -> f32 {
        if self.turned() {
            self.linear[1] * 1000.0
        } else {
            -self.linear[3] * 1000.0
        }
    }
}

fn drawn(page: &paged_renderer::BuiltPage) -> Vec<Drawn> {
    page.list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => {
                let [a, b, c, d, x, y] = transform.0;
                Some(Drawn {
                    linear: [a, b, c, d],
                    x,
                    y,
                })
            }
            _ => None,
        })
        .collect()
}

fn near(got: f32, want: f32, what: &str) {
    assert!((got - want).abs() < 0.05, "{what}: {got}, InDesign {want}");
}

const TEXT: &str = "縦書きの文章です。漢字、かな、カタカナ「括弧」とーを含みます。ABCと123も入れます。日本語の組版を確かめる。";

/// The first column, kerning off, as InDesign set it: its glyphs upright
/// and a full em apart down the frame's height, drawn from 17.6 pt below
/// each em top (their face's vertical origin), the column's em box
/// against the frame's right edge (Em Box Height: the first line's em
/// box against its top), the next column one leading (24 pt) to the left.
#[test]
fn lines_turn_into_columns_from_the_right() {
    let page = build(
        true,
        r#"FirstBaselineOffset="EmBoxHeight""#,
        "",
        &[("", TEXT)],
    );
    let glyphs: Vec<Drawn> = drawn(&page)
        .into_iter()
        .filter(|g| g.upright() && (g.size() - 20.0).abs() < 0.01)
        .collect();
    // 縦書きの文章です。漢字、かな、 — fifteen characters, 300 pt.
    for (i, g) in glyphs.iter().take(15).enumerate() {
        near(g.x, 380.0, "first column, a glyph's left");
        near(g.y, 117.6 + 20.0 * i as f32, "down the column");
    }
    near(glyphs[15].x, 356.0, "second column, one leading left");
    near(
        glyphs[15].y,
        117.6,
        "the second column starts at the top again",
    );
}

/// Latin and digits turn with the column, their baseline 0.12 em in from
/// its em box's left edge (InDesign: A B C at x 360.56 in the column whose
/// kanji are drawn from 358.16 — the Ascent case, two columns in).
#[test]
fn latin_turns_with_the_column() {
    let page = build(
        true,
        r#"FirstBaselineOffset="EmBoxHeight""#,
        "",
        &[("", "漢字ABC")],
    );
    let all = drawn(&page);
    let turned: Vec<&Drawn> = all.iter().filter(|g| g.turned()).collect();
    assert_eq!(turned.len(), 3, "A B C turned: {all:?}");
    let kanji = all.iter().find(|g| g.upright()).unwrap();
    for g in &turned {
        near(
            g.x,
            kanji.x + 2.4,
            "Latin baseline, 0.12 em in from the em box's left",
        );
    }
    // Down the column after 漢字: A starts where the second kanji ends.
    near(turned[0].y, 100.0 + 40.0, "A's start");
}

/// The frame's insets stay on their sides: the right one (40) before the
/// first column, the top one (10) above each column.
#[test]
fn insets_stay_on_their_sides() {
    let page = build(
        true,
        r#"FirstBaselineOffset="EmBoxHeight" InsetSpacing="10 20 30 40""#,
        "",
        &[("", TEXT)],
    );
    let first = drawn(&page).into_iter().find(|g| g.upright()).unwrap();
    near(first.x, 340.0, "40 pt in from the right");
    near(first.y, 127.6, "10 pt down from the top");
}

/// Ruby stands upright to the right of its base, spread 1-2-1 down it
/// (InDesign: たてが over 縦書 at x 402.16, from y 110.47 every 13.33);
/// below, to the left.
#[test]
fn ruby_stands_to_the_right_upright() {
    let ruby = r#"RubyFlag="1" RubyType="GroupRuby" RubyString="たてが""#;
    for (attrs, x) in [("", 400.0), (r#"RubyPosition="BelowLeft""#, 370.0)] {
        let r = format!("{ruby} {attrs}");
        let page = build(
            true,
            r#"FirstBaselineOffset="EmBoxHeight""#,
            "",
            &[(&r, "縦書"), ("", "きの")],
        );
        let marks: Vec<Drawn> = drawn(&page)
            .into_iter()
            .filter(|g| (g.size() - 10.0).abs() < 0.01)
            .collect();
        assert_eq!(marks.len(), 3, "{attrs}: {marks:?}");
        for (i, g) in marks.iter().enumerate() {
            assert!(g.upright(), "ruby upright");
            near(g.x, x, attrs);
            near(g.y, 110.47 + 13.33 * i as f32, "spread 1-2-1 down the base");
        }
    }
}

/// Kenten stand upright to the right, centred on their characters; a
/// mark's shape is not turned with the column.
#[test]
fn kenten_stand_upright_on_their_characters() {
    let page = build(
        true,
        r#"FirstBaselineOffset="EmBoxHeight""#,
        "",
        &[(r#"KentenKind="KentenBlackTriangle""#, "縦書")],
    );
    // A drawn mark's path is in page orientation (y down): upright after
    // the column turn means a plain positive scale, x' = w·x, y' = h·y.
    let all = drawn(&page);
    let marks: Vec<&Drawn> = all
        .iter()
        .filter(|g| g.linear[1].abs() < 1e-5 && g.linear[2].abs() < 1e-5 && g.linear[3] > 0.0)
        .collect();
    assert_eq!(marks.len(), 2, "two upright marks: {all:?}");
    for (i, m) in marks.iter().enumerate() {
        near(m.x, 405.0, "beside the column's em box");
        near(m.y, 110.0 + 20.0 * i as f32, "centred on its character");
    }
}

/// Vertically justified to the centre, a Japanese composer's block ends
/// at its last line's em box bottom, not its baseline (InDesign 20.0.1
/// moved the first column to x 327.08, kanji drawn from there).
#[test]
fn centred_columns_end_at_the_em_box() {
    let page = build(
        true,
        r#"VerticalJustification="CenterAlign""#,
        "",
        &[("", TEXT)],
    );
    let first = drawn(&page).into_iter().find(|g| g.upright()).unwrap();
    // The same frame top-aligned: its columns, and the last one's
    // baseline as a distance in from the right edge (a column's baseline
    // is 0.12 em in from its em box's left, 17.6 pt from its right).
    let top = drawn(&build(true, "", "", &[("", TEXT)]));
    let mut columns: Vec<f32> = top.iter().filter(|g| g.upright()).map(|g| g.x).collect();
    columns.sort_by(|a, b| b.total_cmp(a));
    columns.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    let last_baseline = 400.0 - columns.last().unwrap() - 20.0 + 17.6;
    // Centring takes half of what the frame leaves past the last column's
    // em box bottom, 0.12 em beyond its baseline.
    let slack = 200.0 - (last_baseline + 2.4);
    near(columns[0] - first.x, slack / 2.0, "moved by half the slack");
}
