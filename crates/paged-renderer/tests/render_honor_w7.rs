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

//! FINDING #7 — render-honor probes. Each of these instance-level
//! overrides round-trips on the editor's wire but, before the fixes in
//! this change, painted ZERO pixel delta in the op-sandbox. The tests
//! here assert at the DISPLAY-LIST level (the W1.x style: commands
//! appear / change) that the render path now honours each one:
//!
//!   1. `characterSkew`        → glyph affine `c` (x-shear) term.
//!   2. `paragraphLeftIndent`  → all glyphs shift right; column narrows.
//!      `paragraphRightIndent` → column narrows (earlier wrap).
//!   3. `paragraphRuleAbove`   → a FillPath rule bar appears (instance
//!      rule was dropped by the resolver pre-fix).
//!   4. `frameOuterGlowEnabled`→ OuterGlow command's default colour is
//!      WHITE (black under the default Screen blend is a no-op).
//!
//! (`frameStrokeGapColor` → see `stroke_styles.rs`; `ImageContentTransform`
//! → see `pipeline_lib.rs`.)

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

fn put(zip: &mut ZipWriter<std::io::Cursor<Vec<u8>>>, path: &str, body: &[u8]) {
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(path, deflated).unwrap();
    zip.write_all(body).unwrap();
}

const DESIGNMAP: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#;

const GRAPHIC_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Black" Name="Black" Space="CMYK" ColorValue="0 0 0 100"/>
  </Graphic>
</idPkg:Graphic>"#;

const SPREAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="20 20 380 380" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;

/// Build a single-paragraph IDML. `para_attrs` go on the
/// `<ParagraphStyleRange>`, `char_attrs` on the `<CharacterStyleRange>`.
fn build(para_attrs: &str, char_attrs: &str, content: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange{para_attrs}>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24"{char_attrs}>
        <Content>{content}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    put(&mut zip, "designmap.xml", DESIGNMAP);
    put(&mut zip, "Resources/Graphic.xml", GRAPHIC_XML);
    put(&mut zip, "Spreads/Spread_sp1.xml", SPREAD);
    put(&mut zip, "Stories/Story_u10.xml", story.as_bytes());
    zip.finish().unwrap().into_inner()
}

fn opts(resolver: &BytesResolver) -> PipelineOptions<'_> {
    PipelineOptions {
        assets: Some(resolver),
        ..PipelineOptions::default()
    }
}

/// Per-glyph FillPath affines, returned as the raw 6-tuple. Filters out
/// non-glyph fills (the frame fill / rule bars) by requiring a y-flip
/// (`d < 0`) with a positive x-scale — the glyph emit shape. Skewed
/// glyphs (`c != 0`) still match (unlike `text_glyph_level::glyph_xys`,
/// which rejects non-zero off-diagonals).
fn glyph_affines(cmds: &[DisplayCommand]) -> Vec<[f32; 6]> {
    cmds.iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0),
            _ => None,
        })
        .filter(|m| m[0] > 0.0 && m[3] < 0.0)
        .collect()
}

fn build_page_commands(resolver: &BytesResolver, bytes: &[u8]) -> Vec<DisplayCommand> {
    let document = Document::open(bytes).unwrap();
    let built = pipeline::build_document(&document, &opts(resolver)).unwrap();
    built.pages[0].list.commands.clone()
}

fn resolver() -> BytesResolver {
    let mut r = BytesResolver::new();
    r.add_font("Inter", None, read_font("Inter.ttf"));
    r
}

#[test]
fn character_skew_shears_glyph_affines() {
    // FINDING #7.1 — `Skew` reaches the glyph affine's `c` term. Upright
    // glyphs have c == 0; skewed glyphs have c != 0 (every glyph leans).
    let r = resolver();
    let upright = glyph_affines(&build_page_commands(&r, &build("", "", "Skew")));
    let skewed = glyph_affines(&build_page_commands(
        &r,
        &build("", r#" Skew="15""#, "Skew"),
    ));
    assert!(
        !upright.is_empty() && !skewed.is_empty(),
        "glyphs must emit"
    );
    assert!(
        upright.iter().all(|m| m[2].abs() < 1e-6),
        "upright glyphs must keep c == 0"
    );
    assert!(
        skewed.iter().all(|m| m[2] > 1e-4),
        "every skewed glyph must carry a positive c (right lean): {skewed:?}"
    );
    // Scale terms (a, d) are untouched by skew.
    assert!((skewed[0][0] - upright[0][0]).abs() < 1e-4, "a unchanged");
    assert!((skewed[0][3] - upright[0][3]).abs() < 1e-4, "d unchanged");
}

#[test]
fn paragraph_left_indent_shifts_all_glyphs_right() {
    // FINDING #7.2 — `LeftIndent` shifts every glyph right by the indent.
    // The instance field reaches `ResolvedParagraphAttrs` (was dropped
    // pre-fix: the resolver captured only `first_line_indent`).
    let r = resolver();
    let plain = glyph_affines(&build_page_commands(&r, &build("", "", "Indent me")));
    let indented = glyph_affines(&build_page_commands(
        &r,
        &build(r#" LeftIndent="72""#, "", "Indent me"),
    ));
    assert!(!plain.is_empty() && plain.len() == indented.len());
    // Every glyph's tx (index 4) shifts right by ~72pt.
    for (p, q) in plain.iter().zip(indented.iter()) {
        let dx = q[4] - p[4];
        assert!(
            (dx - 72.0).abs() < 1.0,
            "left indent must shift glyph x by 72pt, got {dx}"
        );
    }
}

#[test]
fn paragraph_right_indent_narrows_column_and_wraps_earlier() {
    // FINDING #7.2 — `RightIndent` narrows the composed column, so a
    // line that fit on one row before now wraps onto more rows. We count
    // distinct glyph baselines (ty) as a proxy for line count.
    let r = resolver();
    let line_count = |bytes: &[u8]| -> usize {
        let affines = glyph_affines(&build_page_commands(&r, bytes));
        let mut ys: Vec<i64> = affines
            .iter()
            .map(|m| (m[5] * 16.0).round() as i64)
            .collect();
        ys.sort_unstable();
        ys.dedup();
        ys.len()
    };
    let text = "one two three four five six seven eight nine ten eleven twelve";
    let wide = line_count(&build("", "", text));
    let narrow = line_count(&build(r#" RightIndent="280""#, "", text));
    assert!(
        narrow > wide,
        "right indent must narrow the column → more lines: wide={wide}, narrow={narrow}"
    );
}

#[test]
fn paragraph_rule_above_instance_paints_a_bar() {
    // FINDING #7.3 — an INSTANCE `RuleAbove` (no style rule) now paints.
    // Pre-fix `ResolvedParagraphAttrs::from_paragraph` set the rule to
    // `Default::default()` and never read the paragraph's own rule, so an
    // instance-only rule painted nothing. The rule bar is a thin
    // horizontal FillPath (NOT a glyph: d > 0, a > 0, wide & short).
    let r = resolver();
    let plain = build_page_commands(&r, &build("", "", "Ruled"));
    let ruled = build_page_commands(
        &r,
        &build(
            r#" RuleAbove="true" RuleAboveLineWeight="3" RuleAboveColor="Color/Black""#,
            "",
            "Ruled",
        ),
    );
    // Count non-glyph FillPaths (the rule bar is one). A glyph affine has
    // d < 0 (y-flip); a rule rect does not.
    let bars = |cmds: &[DisplayCommand]| -> usize {
        cmds.iter()
            .filter(|c| {
                matches!(c, DisplayCommand::FillPath { transform, .. }
                    if transform.0[3] >= 0.0)
            })
            .count()
    };
    assert!(
        bars(&ruled) > bars(&plain),
        "instance RuleAbove must add a rule FillPath: plain={}, ruled={}",
        bars(&plain),
        bars(&ruled)
    );
}

/// Build a Rectangle-frame IDML carrying an OuterGlow effect (no text).
/// `applied` toggles `Applied`; `effect_color` (when non-empty) splices
/// an `EffectColor` attr.
fn build_glow_rect(applied: bool, effect_color: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    let designmap = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#;
    let color_attr = if effect_color.is_empty() {
        String::new()
    } else {
        format!(r#" EffectColor="{effect_color}""#)
    };
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="60 60 140 140" FillColor="Color/Black" StrokeWeight="0">
      <Properties>
        <OuterGlowSetting Applied="{applied}" Size="6" Opacity="75"{color_attr}/>
      </Properties>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#
    );
    put(&mut zip, "designmap.xml", designmap);
    put(&mut zip, "Resources/Graphic.xml", GRAPHIC_XML);
    put(&mut zip, "Spreads/Spread_sp1.xml", spread.as_bytes());
    zip.finish().unwrap().into_inner()
}

#[test]
fn frame_outer_glow_default_color_is_white_not_black() {
    // FINDING #7.4 — the emit/raster path was already wired (the effect
    // flows from the frame instance through `emit_effects_pre_fill`).
    // The zero pixel delta came from the *default colour*: the
    // `default_outer_glow()` struct (and IDML that omits `EffectColor`)
    // resolved to BLACK, and a black glow under the default `Screen`
    // blend is a no-op (`screen(base, 0) = base`). The fix defaults
    // glows to WHITE.
    let bytes = build_glow_rect(true, "");
    let document = Document::open(&bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    let glow = built.pages[0].list.commands.iter().find_map(|c| match c {
        DisplayCommand::OuterGlow { params, .. } => Some(*params),
        _ => None,
    });
    let params = glow.expect("Applied outer glow must emit an OuterGlow command");
    let c = params.color;
    assert!(
        c.r > 0.9 && c.g > 0.9 && c.b > 0.9,
        "default glow colour must be white (visible under Screen), got {c:?}"
    );

    // Sanity: a disabled glow emits no command at all.
    let off = build_glow_rect(false, "");
    let off_doc = Document::open(&off).unwrap();
    let off_built = pipeline::build_document(&off_doc, &PipelineOptions::default()).unwrap();
    assert!(
        !off_built.pages[0]
            .list
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::OuterGlow { .. })),
        "disabled glow must emit no OuterGlow command"
    );
}
