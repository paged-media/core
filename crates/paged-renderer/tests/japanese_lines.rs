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

//! Japanese lines as InDesign 20.0.1 composes them
//! (`tools/indesign-export/vertical-text-probe`, its horizontal controls):
//! a paragraph breaks between any two Japanese characters whatever its
//! hyphenation, a kinsoku set only taking positions away, and `Nothing`
//! (InDesign's default) none; under a Japanese composer a `LeadingOffset`
//! frame puts the em box bottom, not the baseline, one leading down; an
//! emphasis mark is not set on punctuation.

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
    build_with(vertical, frame, para, "", runs)
}

/// [`build`] with `props` (a paragraph `<Properties>` element) in the
/// range; `para` may name its own `Composer`.
fn build_with(
    vertical: bool,
    frame: &str,
    para: &str,
    props: &str,
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
    let composer = if para.contains("Composer=") {
        ""
    } else {
        r#"Composer="HL Composer J""#
    };
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
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><Story Self="s1"><StoryPreference StoryOrientation="{orientation}"/><ParagraphStyleRange {composer} {para}>{props}{runs}</ParagraphStyleRange></Story></idPkg:Story>"#
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

/// The glyphs drawn at `size`, grouped into lines by their y.
fn lines(page: &paged_renderer::BuiltPage, size: f32) -> Vec<Vec<Drawn>> {
    let mut out: Vec<Vec<Drawn>> = Vec::new();
    for g in drawn(page)
        .into_iter()
        .filter(|g| g.upright() && (g.size() - size).abs() < 0.01)
    {
        match out.last_mut() {
            Some(line) if (line[0].y - g.y).abs() < 0.5 => line.push(g),
            _ => out.push(vec![g]),
        }
    }
    out
}

const NINE_AND_STOP: &str = "漢字漢字漢字漢字漢字。漢字";

/// Ten 20 pt kanji fill the 200 pt frame; the full stop after them starts
/// the next line when the paragraph has no kinsoku set (InDesign 20.0.1
/// set `。` at a column's head in exactly this case), hyphenation on.
#[test]
fn japanese_breaks_between_characters_with_no_kinsoku_set() {
    for set in [
        "",
        r#"<Properties><KinsokuSet type="enumeration">Nothing</KinsokuSet></Properties>"#,
    ] {
        let page = build_with(
            false,
            "",
            r#"KinsokuType="KinsokuPushInFirst" Hyphenation="true""#,
            set,
            &[("", NINE_AND_STOP)],
        );
        let lines = lines(&page, 20.0);
        assert_eq!(lines.len(), 2, "{set}: {lines:?}");
        assert_eq!(lines[0].len(), 10, "ten kanji on the first line");
        assert!(lines[0].iter().all(|g| g.x < 400.0), "inside the frame");
    }
}

/// A kinsoku set that names a table keeps the full stop off a line's head.
#[test]
fn a_kinsoku_set_keeps_the_full_stop_off_a_lines_head() {
    let set = r#"<Properties><KinsokuSet type="enumeration">HardKinsoku</KinsokuSet></Properties>"#;
    let page = build_with(
        false,
        "",
        r#"KinsokuType="KinsokuPushInFirst""#,
        set,
        &[("", NINE_AND_STOP)],
    );
    let lines = lines(&page, 20.0);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_ne!(
        lines[1].len(),
        3,
        "the stop does not begin the second line: {lines:?}"
    );
}

/// `LeadingOffset`, 30 pt leading, 20 pt: a Japanese composer sets the first
/// baseline 27.6 pt down (the em box bottom, 0.12 em under it, one leading
/// down), the Paragraph Composer 30 (InDesign 20.0.1, Kozuka Mincho).
#[test]
fn a_japanese_composer_puts_the_em_box_bottom_one_leading_down() {
    for (composer, want) in [("HL Composer J", 127.6), ("HL Composer", 130.0)] {
        let page = build_with(
            false,
            r#"FirstBaselineOffset="LeadingOffset""#,
            &format!(r#"Composer="{composer}""#),
            "",
            &[(r#"Leading="30""#, "漢字")],
        );
        let first = drawn(&page).into_iter().find(|g| g.upright()).unwrap();
        near(first.y, want, composer);
    }
}

/// Punctuation takes no emphasis mark: of 漢。字、 marked, two marks.
#[test]
fn emphasis_marks_skip_punctuation() {
    let page = build(
        false,
        "",
        "",
        &[(r#"KentenKind="KentenBlackCircle""#, "漢。字、")],
    );
    let marks = drawn(&page)
        .into_iter()
        .filter(|g| g.linear[1].abs() < 1e-5 && g.linear[3] > 0.0)
        .count();
    assert_eq!(marks, 2);
}
