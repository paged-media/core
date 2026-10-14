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

//! A footnote's paragraphs set as InDesign sets them, and the paragraph
//! rules the same probe pinned in body text.
//!
//! Every expectation is InDesign 20.0.1's own layout of the sample
//! `tools/indesign-export/footnote-paragraphs-probe.jsx` builds: per line,
//! its baseline below the frame's top and the x of its first and last
//! glyph on the page. Each test rebuilds one of the probe's frames (360 pt
//! wide at x 57.638, top 40, 160 pt tall; body 12 pt and notes 10 pt Open
//! Sans) in the spelling InDesign wrote it.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const LONG: &str = "Footnote text that runs long enough to wrap onto a second and a third line, \
                    so that alignment, indents, leading and spacing all show where InDesign puts \
                    each line of the note.";
const SHORT: &str = "A second, shorter note.";
const FRAME_TOP: f32 = 40.0;

/// A run in Open Sans, as InDesign writes one.
fn run(attrs: &str, properties: &str, content: &str) -> String {
    format!(
        r#"<CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]" FontStyle="Regular" {attrs}><Properties>{properties}<AppliedFont type="string">Open Sans</AppliedFont></Properties>{content}</CharacterStyleRange>"#
    )
}

/// A note: its paragraphs in one range (`<Br/>` between them) in
/// paragraph style `style` with `attrs` and the typed `properties`, the
/// first opening with the reference character and the tab InDesign types;
/// its run in 10 pt with `run_properties`.
fn note(
    style: &str,
    attrs: &str,
    properties: &str,
    run_properties: &str,
    texts: &[&str],
) -> String {
    let content = texts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let lead = if i == 0 { "<?ACE 4?>\t" } else { "" };
            format!("<Content>{lead}{t}</Content>")
        })
        .collect::<Vec<_>>()
        .join("<Br/>");
    let properties = if properties.is_empty() {
        String::new()
    } else {
        format!("<Properties>{properties}</Properties>")
    };
    format!(
        r#"<Footnote><ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/{style}" {attrs}>{properties}{}</ParagraphStyleRange></Footnote>"#,
        run(r#"PointSize="10""#, run_properties, &content)
    )
}

/// The probe's body line with `first` after "Body" and `last` at its end
/// (either may be empty).
fn noted_story(first: &str, last: &str) -> String {
    let reference = |n: &str| {
        if n.is_empty() {
            String::new()
        } else {
            run(r#"PointSize="12" Position="Superscript""#, "", n)
        }
    };
    format!(
        r#"<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/$ID/NormalParagraphStyle">{}{}{}{}</ParagraphStyleRange>"#,
        run(r#"PointSize="12""#, "", "<Content>Body</Content>"),
        reference(first),
        run(
            r#"PointSize="12""#,
            "",
            "<Content> line that carries the note.</Content>"
        ),
        reference(last),
    )
}

/// One A4 page, one frame holding `story` (its `<ParagraphStyleRange>`s),
/// its styles based on InDesign's `[No paragraph style]` as the probe's
/// InDesign wrote it (its composition attributes and German hyphenation:
/// either changes where lines break).
fn build(story: &str) -> paged_renderer::BuiltPage {
    let styles = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<RootParagraphStyleGroup Self="psg">
<ParagraphStyle Self="ParagraphStyle/$ID/[No paragraph style]" Name="$ID/[No paragraph style]" AppliedLanguage="$ID/de_DE_2006" KerningMethod="$ID/Metrics" Ligatures="true" Tracking="0" Composer="HL Composer" Hyphenation="true" HyphenateLadderLimit="3" HyphenateAfterFirst="2" HyphenateBeforeLast="2" HyphenateCapitalizedWords="true" HyphenateWordsLongerThan="5" HyphenationZone="36" HyphenWeight="5" HyphenateLastWord="true" MinimumWordSpacing="80" DesiredWordSpacing="100" MaximumWordSpacing="133" MinimumLetterSpacing="0" DesiredLetterSpacing="0" MaximumLetterSpacing="0" MinimumGlyphScaling="100" DesiredGlyphScaling="100" MaximumGlyphScaling="100" Justification="LeftAlign"/>
<ParagraphStyle Self="ParagraphStyle/$ID/NormalParagraphStyle" Name="$ID/NormalParagraphStyle"><Properties><BasedOn type="string">$ID/[No paragraph style]</BasedOn></Properties></ParagraphStyle>
<ParagraphStyle Self="ParagraphStyle/Note" Name="Note" FontStyle="Regular" PointSize="10" LeftIndent="24" Justification="RightAlign"><Properties><BasedOn type="string">$ID/[No paragraph style]</BasedOn><Leading type="unit">15</Leading><AppliedFont type="string">Open Sans</AppliedFont></Properties></ParagraphStyle>
</RootParagraphStyleGroup>
</idPkg:Styles>"#;
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
<idPkg:Styles src="Resources/Styles.xml"/>
<idPkg:Spread src="Spreads/Spread_sp1.xml"/>
<idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    );
    put("Resources/Styles.xml", styles);
    put(
        "Spreads/Spread_sp1.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="sp1">
<Page Self="p1" GeometricBounds="0 0 841.89 595.276"/>
<TextFrame Self="tf1" ParentStory="s1" GeometricBounds="40 57.638 200 417.638"/>
</Spread>
</idPkg:Spread>"#,
    );
    put(
        "Stories/Story_s1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><Story Self="s1">{story}</Story></idPkg:Story>"#
        ),
    );
    let idml = zip.finish().unwrap().into_inner();
    let fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    let mut resolver = BytesResolver::new();
    resolver.add_font(
        "Open Sans",
        None,
        std::fs::read(fonts.join("OpenSans.ttf")).unwrap(),
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

/// The glyphs on the page, line by line: each baseline (pt below the
/// frame's top) with its glyphs' x, left to right.
fn lines(page: &paged_renderer::BuiltPage) -> Vec<(f32, Vec<f32>)> {
    let mut rows: Vec<(f32, Vec<f32>)> = Vec::new();
    for c in &page.list.commands {
        let DisplayCommand::FillPath { transform, .. } = c else {
            continue;
        };
        let [a, b, c2, d, tx, ty] = transform.0;
        if !(b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0) {
            continue;
        }
        let y = ty - FRAME_TOP;
        match rows.iter_mut().find(|(r, _)| (r - y).abs() < 0.01) {
            Some((_, xs)) => xs.push(tx),
            None => rows.push((y, vec![tx])),
        }
    }
    for (_, xs) in &mut rows {
        xs.sort_by(f32::total_cmp);
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows
}

/// InDesign's lines: `(baseline below the frame's top, first glyph x,
/// last character's x, hyphenated)` — a hyphenated line draws a hyphen
/// after its last character, a glyph InDesign's text does not hold.
type Line = (f32, f32, f32, bool);

/// The engine's lines from `from_y` down against InDesign's, to 0.06 pt
/// (InDesign reports thousandths; the engine sets 1/64 pt).
fn assert_lines(page: &paged_renderer::BuiltPage, from_y: f32, want: &[Line]) {
    let got: Vec<(f32, Vec<f32>)> = lines(page)
        .into_iter()
        .filter(|(y, _)| *y > from_y)
        .collect();
    assert_eq!(
        got.len(),
        want.len(),
        "lines at {:?}, InDesign {want:?}",
        got.iter().map(|l| l.0).collect::<Vec<_>>()
    );
    for ((y, xs), &(wy, first, last, hyphenated)) in got.iter().zip(want) {
        let drawn_last = xs[xs.len() - 1 - usize::from(hyphenated)];
        assert!(
            (y - wy).abs() < 0.06
                && (xs[0] - first).abs() < 0.06
                && (drawn_last - last).abs() < 0.06,
            "line at {y} from {} to {drawn_last}, InDesign at {wy} from {first} to {last}",
            xs[0]
        );
    }
}

/// The note lines, below the body's one line.
fn assert_note_lines(page: &paged_renderer::BuiltPage, want: &[Line]) {
    assert_lines(page, 20.0, want);
}

const NORMAL: &str = "$ID/NormalParagraphStyle";

/// The probe's long note, its paragraph given `attrs` and `properties`,
/// its run `run_properties`.
fn one_note(attrs: &str, properties: &str, run_properties: &str) -> paged_renderer::BuiltPage {
    build(&noted_story(
        "",
        &note(NORMAL, attrs, properties, run_properties, &[LONG]),
    ))
}

#[test]
fn a_note_takes_its_paragraphs_alignment() {
    // The number stays at the line's start: only the text after the tab
    // is centred or set flush right.
    assert_note_lines(
        &one_note(r#"Justification="CenterAlign""#, "", ""),
        &[
            (136.0, 57.638, 402.037, false),
            (148.0, 61.144, 404.782, true),
            (160.0, 159.923, 312.726, false),
        ],
    );
    assert_note_lines(
        &one_note(r#"Justification="RightAlign""#, "", ""),
        &[
            (136.0, 57.638, 412.081, false),
            (148.0, 64.650, 408.287, true),
            (160.0, 262.208, 415.011, false),
        ],
    );
    assert_note_lines(
        &one_note(r#"Justification="LeftJustified""#, "", ""),
        &[
            (136.0, 57.638, 411.520, false),
            (148.0, 57.638, 411.505, false),
            (160.0, 57.638, 176.119, false),
        ],
    );
}

#[test]
fn a_note_takes_its_paragraphs_indents_and_tab_stops() {
    // A hanging first line: its tab stops at the left indent.
    assert_note_lines(
        &one_note(
            r#"LeftIndent="24" FirstLineIndent="-12" RightIndent="36""#,
            "",
            "",
        ),
        &[
            (136.0, 69.638, 371.277, false),
            (148.0, 81.638, 366.457, false),
            (160.0, 81.638, 302.424, false),
        ],
    );
    let tab_at_20 = concat!(
        r#"<TabList type="list"><ListItem type="record">"#,
        r#"<Alignment type="enumeration">LeftAlign</Alignment>"#,
        r#"<AlignmentCharacter type="string">.</AlignmentCharacter>"#,
        r#"<Leader type="string"></Leader><Position type="unit">20</Position>"#,
        r#"</ListItem></TabList>"#,
    );
    assert_note_lines(
        &one_note("", tab_at_20, ""),
        &[
            (136.0, 57.638, 400.455, false),
            (148.0, 57.638, 407.975, false),
            (160.0, 57.638, 176.119, false),
        ],
    );
}

#[test]
fn a_note_takes_its_leading_spacing_and_style() {
    // Each line one leading below the last, the last on the frame's foot.
    assert_note_lines(
        &one_note("", "", r#"<Leading type="unit">18</Leading>"#),
        &[
            (124.0, 57.638, 391.992, false),
            (142.0, 57.638, 401.276, true),
            (160.0, 57.638, 210.441, false),
        ],
    );
    // Word spacing, letter spacing and glyph scaling as in body text.
    assert_note_lines(
        &one_note(
            concat!(
                r#"MinimumWordSpacing="80" DesiredWordSpacing="150" MaximumWordSpacing="200" "#,
                r#"MinimumLetterSpacing="0" DesiredLetterSpacing="10" MaximumLetterSpacing="20" "#,
                r#"MinimumGlyphScaling="85" DesiredGlyphScaling="90" MaximumGlyphScaling="100""#
            ),
            "",
            "",
        ),
        &[
            (136.0, 57.638, 390.679, false),
            (148.0, 57.638, 398.707, true),
            (160.0, 57.638, 209.889, false),
        ],
    );
    // Through a paragraph style: LeftIndent 24, RightAlign, Leading 15.
    let page = build(&noted_story("", &note("Note", "", "", "", &[LONG])));
    assert_note_lines(
        &page,
        &[
            (130.0, 81.638, 412.081, false),
            (145.0, 85.875, 408.805, true),
            (160.0, 240.983, 415.011, false),
        ],
    );
}

#[test]
fn a_notes_space_before_and_after_count_only_between_its_own_paragraphs() {
    let spaced = r#"SpaceBefore="12" SpaceAfter="9""#;
    // Two one-paragraph notes asking for space: one leading apart.
    let page = build(&noted_story(
        &note(NORMAL, spaced, "", "", &[SHORT]),
        &note(NORMAL, spaced, "", "", &[SHORT]),
    ));
    assert_note_lines(
        &page,
        &[
            (148.0, 57.638, 199.375, false),
            (160.0, 57.638, 199.375, false),
        ],
    );
    // A note of two paragraphs: 12 + 9 pt more than a leading between
    // them, nothing more before the next note.
    let page = build(&noted_story(
        &note(
            NORMAL,
            spaced,
            "",
            "",
            &["The first paragraph of a note.", "Its second paragraph."],
        ),
        &note(NORMAL, "", "", "", &[SHORT]),
    ));
    assert_note_lines(
        &page,
        &[
            (115.0, 57.638, 227.095, false),
            (148.0, 57.638, 156.036, false),
            (160.0, 57.638, 199.375, false),
        ],
    );
}

/// Body text: one range of `texts` (a `<Br/>` between each two), in
/// 12 pt (10 pt with `run_attrs` naming it) Open Sans.
fn body(paragraph_attrs: &str, run_attrs: &str, texts: &[&str]) -> paged_renderer::BuiltPage {
    let content = texts
        .iter()
        .map(|t| format!("<Content>{t}</Content>"))
        .collect::<Vec<_>>()
        .join("<Br/>");
    build(&format!(
        r#"<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/$ID/NormalParagraphStyle" {paragraph_attrs}>{}</ParagraphStyleRange>"#,
        run(run_attrs, "", &content)
    ))
}

#[test]
fn every_paragraph_of_a_range_keeps_its_space_before_and_after() {
    // InDesign writes two paragraphs set alike as one range: the second
    // sits the leading plus 9 pt after and 12 pt before below the first,
    // and an empty paragraph between them takes the same.
    let spaced = r#"SpaceBefore="12" SpaceAfter="9""#;
    let baselines = |page: &paged_renderer::BuiltPage| -> Vec<f32> {
        lines(page).into_iter().map(|(y, _)| y).collect()
    };
    let two = baselines(&body(
        spaced,
        r#"PointSize="12""#,
        &["First body paragraph.", "Second body paragraph."],
    ));
    let three = baselines(&body(
        spaced,
        r#"PointSize="12""#,
        &["First body paragraph.", "", "Third body paragraph."],
    ));
    for (got, want) in [(two, vec![12.826, 48.226]), (three, vec![12.826, 83.626])] {
        assert_eq!(got.len(), want.len(), "{got:?}");
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() < 0.01, "baselines {got:?}, InDesign {want:?}");
        }
    }
}

#[test]
fn a_flush_right_or_centred_line_aligns_only_its_text_after_the_last_tab() {
    let text = format!("1\t{LONG}");
    assert_lines(
        &body(
            r#"Justification="RightAlign""#,
            r#"PointSize="10""#,
            &[&text],
        ),
        0.0,
        &[
            (10.688, 57.638, 412.081, false),
            (22.688, 64.650, 408.287, true),
            (34.688, 262.208, 415.011, false),
        ],
    );
    assert_lines(
        &body(
            r#"Justification="CenterAlign""#,
            r#"PointSize="10""#,
            &[&text],
        ),
        0.0,
        &[
            (10.688, 57.638, 402.037, false),
            (22.688, 61.144, 404.782, true),
            (34.688, 159.923, 312.726, false),
        ],
    );
}
