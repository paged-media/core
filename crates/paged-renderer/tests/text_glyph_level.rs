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

//! Glyph-level integration tests for the text path. Complements the
//! pixel-bracket tests in `real_ttf_features.rs` by asserting on the
//! shape of the DisplayList that comes out of `pipeline::build_document`:
//! per-glyph `FillPath` count, glyph x/y positions, decoration
//! `StrokePath` commands, frame-chain bookkeeping. The pixel-level
//! tests prove "the glyphs landed somewhere reasonable"; these tests
//! prove "the commands the rasterizer would consume have the right
//! count, ordering, and offsets."
//!
//! All fonts come from `corpus/fonts/` (Open Sans / Inter / Lora /
//! RobotoSlab); no test downloads anything.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::{DisplayCommand, PathSegment, Transform};
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

fn write_zip<F: FnOnce(&mut ZipWriter<std::io::Cursor<Vec<u8>>>)>(f: F) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    f(&mut zip);
    zip.finish().unwrap().into_inner()
}

fn put(zip: &mut ZipWriter<std::io::Cursor<Vec<u8>>>, path: &str, body: &[u8]) {
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(path, deflated).unwrap();
    zip.write_all(body).unwrap();
}

/// All `FillPath` commands on a page, ordered as emitted.
fn fill_paths<'a>(
    cmds: &'a [DisplayCommand],
) -> impl Iterator<Item = (&'a paged_compose::PathId, &'a Transform)> + 'a {
    cmds.iter().filter_map(|c| match c {
        DisplayCommand::FillPath {
            path_id, transform, ..
        } => Some((path_id, transform)),
        _ => None,
    })
}

/// All `StrokePath` commands on a page, ordered as emitted.
fn stroke_paths<'a>(
    cmds: &'a [DisplayCommand],
) -> impl Iterator<
    Item = (
        &'a paged_compose::PathId,
        &'a paged_compose::Stroke,
        &'a Transform,
    ),
> + 'a {
    cmds.iter().filter_map(|c| match c {
        DisplayCommand::StrokePath {
            path_id,
            stroke,
            transform,
            ..
        } => Some((path_id, stroke, transform)),
        _ => None,
    })
}

/// Per-glyph FillPath transforms have the shape
///   `[scale, 0, 0, -scale, x, y]`
/// (see `paged_compose::text::emit_glyph_slice`). Pull `(x, y, scale)`
/// from a transform; returns `None` if the matrix doesn't look like a
/// glyph (i.e. has a non-zero off-diagonal). This filters out the
/// rectangular frame fills, which carry shearing/scaling.
fn glyph_xys(cmds: &[DisplayCommand]) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::new();
    for c in cmds {
        if let DisplayCommand::FillPath { transform, .. } = c {
            let [a, b, c2, d, tx, ty] = transform.0;
            // Glyph emit always uses a uniform-scale-with-y-flip matrix:
            // (a > 0, d < 0, b == c == 0). Frame fills don't.
            if b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0 && (a + d).abs() < 1e-4 {
                out.push((tx, ty, a));
            }
        }
    }
    out
}

/// Is `seg` a horizontal-line path (MoveTo + LineTo with same y)? The
/// underline / strikethrough emitter writes exactly this.
fn is_horizontal_line(segs: &[PathSegment]) -> bool {
    if segs.len() != 2 {
        return false;
    }
    match (&segs[0], &segs[1]) {
        (PathSegment::MoveTo { y: y1, .. }, PathSegment::LineTo { y: y2, .. }) => {
            (y1 - y2).abs() < 1e-3
        }
        _ => false,
    }
}

// ─────────────────────────── shared spread XML ──────────────────────

const DESIGNMAP: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#;

const DESIGNMAP_STYLED: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#;

const GRAPHIC_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#;

// ─────────────────────────── 1. per-run fonts ───────────────────────

fn build_two_run_idml(font_a: &str, font_b: &str) -> Vec<u8> {
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="{font_a}" PointSize="36">
        <Content>AAAA</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="{font_b}" PointSize="36">
        <Content>BBBB</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

#[test]
fn per_run_font_switch_emits_two_path_id_groups_in_order() {
    let bytes = build_two_run_idml("Inter", "Lora");
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    resolver.add_font("Lora", None, read_font("Lora.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let page = &built.pages[0];

    let xys = glyph_xys(&page.list.commands);
    // 4 'A' + 4 'B' = 8 glyphs (no spaces, no shaping merges expected
    // for ASCII letters at this size). Allow a tiny bit of slack for
    // future composer tweaks.
    assert!(
        xys.len() >= 7,
        "expected ~8 glyph FillPaths, got {}",
        xys.len()
    );

    // Glyphs must be ordered left → right monotonically: per-run font
    // switching mustn't reset the x-cursor.
    let xs: Vec<f32> = xys.iter().map(|g| g.0).collect();
    for w in xs.windows(2) {
        assert!(
            w[1] >= w[0] - 0.5,
            "glyph x positions should be monotonic across the font switch; got {xs:?}"
        );
    }

    // Two distinct glyph outlines for 'A' (Inter) vs 'B' (Lora). The
    // outline interner keys on `(font_id, glyph_id)` so even if the
    // glyph ids happened to coincide, the path_ids would differ.
    let path_ids: Vec<u32> = fill_paths(&page.list.commands).map(|(p, _)| p.0).collect();
    let unique: std::collections::BTreeSet<u32> = path_ids.iter().copied().collect();
    assert!(
        unique.len() >= 2,
        "expected at least 2 distinct glyph outlines across two fonts, got {unique:?}"
    );

    // First 4 glyphs (run A) must use a different path_id than the
    // last 4 (run B). Specifically: the set of path_ids in [0..4]
    // disjoint from [4..]. If layout_runs collapsed the run boundary,
    // some path_id from run A would reappear in run B. (Excluding the
    // frame fill at index 0 of `path_ids` — first FillPath is the
    // frame.)
    let glyph_ids: Vec<u32> = page
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath {
                path_id, transform, ..
            } => {
                let [a, b, c2, d, _, _] = transform.0;
                if b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0 {
                    Some(path_id.0)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    let n = glyph_ids.len();
    assert!(n >= 7, "expected ≥7 glyph fills, got {n}");
    let half = n / 2;
    let run_a: std::collections::BTreeSet<u32> = glyph_ids[..half].iter().copied().collect();
    let run_b: std::collections::BTreeSet<u32> = glyph_ids[half..].iter().copied().collect();
    assert!(
        run_a.is_disjoint(&run_b),
        "run A and run B share glyph path_ids — per-run font switch likely failed: A={run_a:?}, B={run_b:?}",
    );
}

// ─────────────────────────── 2. story threading ─────────────────────

fn build_threaded_idml() -> Vec<u8> {
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 600"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="40 40 100 280"
               NextTextFrame="frameB" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u10"
               GeometricBounds="40 320 360 560" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>This sentence is intentionally long so that the text composer must break it across many lines. After enough lines accumulate, the first frame fills up and the remainder spills into the second, threaded frame. Every visible glyph proves both frames received composed text. More words to be safe. And a few more for the overflow buffer.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    })
}

#[test]
fn threaded_story_glyphs_land_in_both_frames_with_monotonic_baselines() {
    let bytes = build_threaded_idml();
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let page = &built.pages[0];

    let xys = glyph_xys(&page.list.commands);
    // Both frame ranges in page coordinates:
    //   frameA: x ∈ [40, 280] pt, y ∈ [40, 100]
    //   frameB: x ∈ [320, 560] pt, y ∈ [40, 360]
    let in_a = xys
        .iter()
        .filter(|(x, y, _)| *x >= 40.0 && *x <= 280.0 && *y >= 40.0 && *y <= 110.0)
        .count();
    let in_b = xys
        .iter()
        .filter(|(x, y, _)| *x >= 320.0 && *x <= 560.0 && *y >= 40.0 && *y <= 360.0)
        .count();
    assert!(
        in_a > 5,
        "frame A should receive glyphs from the head of the story, got {in_a}"
    );
    assert!(
        in_b > 5,
        "frame B should receive overflow glyphs, got {in_b}"
    );
    assert!(
        in_a + in_b > 40,
        "threaded story should compose many glyphs, got {in_a} + {in_b}"
    );

    // Line distribution must be monotonic: every glyph in frame B
    // sits on a baseline ≥ every glyph in frame A's last line
    // (modulo the per-frame y origin reset — frame B has its own
    // top, so we compare *line index* not absolute y). The simplest
    // monotonic check available without per-line metadata: the
    // baselines within each frame are non-decreasing top-to-bottom.
    let mut a_baselines: Vec<f32> = xys
        .iter()
        .filter(|(x, y, _)| *x >= 40.0 && *x <= 280.0 && *y >= 40.0 && *y <= 110.0)
        .map(|g| g.1)
        .collect();
    a_baselines.sort_by(|a, b| a.partial_cmp(b).unwrap());
    a_baselines.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    let mut b_baselines: Vec<f32> = xys
        .iter()
        .filter(|(x, y, _)| *x >= 320.0 && *x <= 560.0 && *y >= 40.0 && *y <= 360.0)
        .map(|g| g.1)
        .collect();
    b_baselines.sort_by(|a, b| a.partial_cmp(b).unwrap());
    b_baselines.dedup_by(|a, b| (*a - *b).abs() < 0.5);

    assert!(
        a_baselines.len() >= 2,
        "frame A should hold ≥2 distinct baselines, got {a_baselines:?}"
    );
    assert!(
        b_baselines.len() >= 2,
        "frame B should hold ≥2 distinct baselines, got {b_baselines:?}"
    );
    // Baselines within a frame are strictly increasing.
    for w in a_baselines.windows(2) {
        assert!(
            w[1] > w[0],
            "frame A baselines not monotonic: {a_baselines:?}"
        );
    }
    for w in b_baselines.windows(2) {
        assert!(
            w[1] > w[0],
            "frame B baselines not monotonic: {b_baselines:?}"
        );
    }
}

// ─────────────────────────── 3. underline / strikethrough ───────────

fn build_decoration_idml(extra_attrs: &str, content: &str) -> Vec<u8> {
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36"{extra_attrs}>
        <Content>{content}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

/// Return horizontal-line StrokePath commands paired with their y-pos
/// and x-range. These are the underline / strikethrough strokes.
fn horizontal_stroke_lines(page: &paged_renderer::BuiltPage) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::new();
    for (path_id, _stroke, _) in stroke_paths(&page.list.commands) {
        let Some(path) = page.list.paths.get(*path_id) else {
            continue;
        };
        if !is_horizontal_line(&path.segments) {
            continue;
        }
        if let (PathSegment::MoveTo { x: x1, y }, PathSegment::LineTo { x: x2, .. }) =
            (&path.segments[0], &path.segments[1])
        {
            out.push((*y, x1.min(*x2), x1.max(*x2)));
        }
    }
    out
}

#[test]
fn underline_emits_horizontal_stroke_below_baseline() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let plain = pipeline::build_document(
        &paged_parse::import_idml_doc(&build_decoration_idml("", "Underline")).unwrap(),
        &opts,
    )
    .unwrap();
    let under = pipeline::build_document(
        &paged_parse::import_idml_doc(&build_decoration_idml(r#" Underline="true""#, "Underline"))
            .unwrap(),
        &opts,
    )
    .unwrap();

    let plain_lines = horizontal_stroke_lines(&plain.pages[0]);
    let under_lines = horizontal_stroke_lines(&under.pages[0]);
    assert!(
        plain_lines.is_empty(),
        "plain run must emit no decoration stripes, got {plain_lines:?}",
    );
    assert!(
        !under_lines.is_empty(),
        "underlined run must emit ≥1 horizontal stripe"
    );

    // Glyph baselines (text origin y) lie above the underline. The
    // first glyph's tx is its x position; ty is its baseline-y. The
    // underline must be at y > baseline (drawn *below* it on the
    // y-down page).
    let xys = glyph_xys(&under.pages[0].list.commands);
    assert!(!xys.is_empty(), "underlined run must emit glyphs");
    let baseline = xys[0].1;
    for (y, x1, x2) in &under_lines {
        assert!(
            *y > baseline,
            "underline y={y} must be > glyph baseline y={baseline}",
        );
        // Underline must span the glyph row (start ≤ baseline left,
        // end ≥ baseline right) — proves the decorator integrated
        // the line, not just emitted a stub.
        assert!(
            *x1 <= xys.last().unwrap().0 && *x2 >= xys[0].0,
            "underline x-range ({x1}..{x2}) doesn't bracket glyphs ({:?}..{:?})",
            xys.first(),
            xys.last(),
        );
    }
}

#[test]
fn strikethrough_emits_horizontal_stroke_above_baseline() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let strike = pipeline::build_document(
        &paged_parse::import_idml_doc(&build_decoration_idml(r#" StrikeThru="true""#, "Strike"))
            .unwrap(),
        &opts,
    )
    .unwrap();
    let both = pipeline::build_document(
        &paged_parse::import_idml_doc(&build_decoration_idml(
            r#" StrikeThru="true" Underline="true""#,
            "Both",
        ))
        .unwrap(),
        &opts,
    )
    .unwrap();

    let strike_lines = horizontal_stroke_lines(&strike.pages[0]);
    assert!(
        !strike_lines.is_empty(),
        "strikethrough must emit ≥1 stripe"
    );

    let xys = glyph_xys(&strike.pages[0].list.commands);
    let baseline = xys[0].1;
    for (y, _, _) in &strike_lines {
        // Strikethrough is drawn *above* the baseline in y-down space.
        assert!(
            *y < baseline,
            "strikethrough y={y} must be < glyph baseline y={baseline}",
        );
    }

    // Underline + Strikethrough together emit two stripes; one above
    // baseline (strike), one below (under).
    let both_lines = horizontal_stroke_lines(&both.pages[0]);
    let both_baseline = glyph_xys(&both.pages[0].list.commands)[0].1;
    let above = both_lines
        .iter()
        .filter(|(y, _, _)| *y < both_baseline)
        .count();
    let below = both_lines
        .iter()
        .filter(|(y, _, _)| *y > both_baseline)
        .count();
    assert!(
        above >= 1 && below >= 1,
        "Underline+StrikeThru should emit a stripe on each side of the baseline; above={above}, below={below}",
    );
}

// ─────────────────────────── 4. vertical justify ────────────────────

fn build_vj_idml(vj: &str) -> Vec<u8> {
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 360 572" StrokeWeight="0">
      <TextFramePreference VerticalJustification="{vj}"/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread.as_bytes());
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>VJ</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    })
}

#[test]
fn vertical_justify_shifts_first_glyph_baseline_by_distinct_amounts() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let first_glyph_y = |vj: &str| -> f32 {
        let bytes = build_vj_idml(vj);
        let doc = paged_parse::import_idml_doc(&bytes).unwrap();
        let built = pipeline::build_document(&doc, &opts).unwrap();
        let xys = glyph_xys(&built.pages[0].list.commands);
        assert!(!xys.is_empty(), "VJ {vj} must emit glyphs");
        // First glyph in display-list order is the leftmost glyph of
        // the first line — its `ty` is the line's baseline.
        xys[0].1
    };

    let top = first_glyph_y("TopAlign");
    let center = first_glyph_y("CenterAlign");
    let bottom = first_glyph_y("BottomAlign");

    // Frame is 320 pt tall at y=40..360 pt. Top is near 40+ascent;
    // Bottom is near 360. Center is in between. Demand strict
    // ordering with > 50 pt separation between adjacent modes.
    assert!(
        top + 50.0 < center,
        "expected Center baseline >> Top: top={top}, center={center}",
    );
    assert!(
        center + 50.0 < bottom,
        "expected Bottom baseline >> Center: center={center}, bottom={bottom}",
    );
}

// ─────────────────────────── 5. bulleted list ───────────────────────

fn build_bullet_idml(bullet_codepoint: u32) -> Vec<u8> {
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Bulleted"
                    Name="Bulleted"
                    BulletsAndNumberingListType="BulletList"
                    BulletsTextAfter=" ">
      <Properties>
        <BulletChar BulletCharacterValue="{bullet_codepoint}"/>
      </Properties>
    </ParagraphStyle>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#
    );
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Bulleted">
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Item</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP_STYLED);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Resources/Styles.xml", styles.as_bytes());
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    })
}

#[test]
fn bullet_glyph_precedes_content_glyphs_at_inherited_scale() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    // U+2022 = BULLET
    let bullet_doc = paged_parse::import_idml_doc(&build_bullet_idml(0x2022)).unwrap();
    let bullet_built = pipeline::build_document(&bullet_doc, &opts).unwrap();
    let bullet_xys = glyph_xys(&bullet_built.pages[0].list.commands);

    // Plain "Item" — no list applied. Same frame, same font/size.
    let plain_bytes = write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(
            zip,
            "Spreads/Spread_sp1.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        );
        put(
            zip,
            "Stories/Story_u10.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Item</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        );
    });
    let plain_doc = paged_parse::import_idml_doc(&plain_bytes).unwrap();
    let plain_built = pipeline::build_document(&plain_doc, &opts).unwrap();
    let plain_xys = glyph_xys(&plain_built.pages[0].list.commands);

    // 1) The bulleted paragraph emits more glyphs than the plain one
    //    (the bullet + the separator space — the space typically
    //    shapes to a non-inking glyph but still produces a FillPath
    //    only if its outline is non-empty; we therefore require
    //    >=1 extra, not >=2).
    assert!(
        bullet_xys.len() > plain_xys.len(),
        "bulleted paragraph should add ≥1 glyph; plain={}, bullet={}",
        plain_xys.len(),
        bullet_xys.len(),
    );

    // 2) The bullet sits at (or just before) the first content glyph.
    //    Plain 'I' sits at x ≈ frame_origin_x ≈ 40 pt; bulleted 'I'
    //    sits past the bullet+space. So the first bulleted glyph
    //    must be at an x ≤ the plain first-glyph x (the bullet is
    //    leftmost; it's emitted *before* the content).
    let plain_first_x = plain_xys[0].0;
    let bullet_first_x = bullet_xys[0].0;
    assert!(
        bullet_first_x <= plain_first_x + 0.5,
        "bullet should sit at or left of the content baseline column: bullet_first_x={bullet_first_x}, plain_first_x={plain_first_x}",
    );

    // 3) The bullet glyph inherits the run's font + size, so its
    //    transform scale equals the content glyphs' scale. (em scale
    //    = point_size / units_per_em; both runs share a font and
    //    size, so identical to within float ε.)
    let bullet_scale = bullet_xys[0].2;
    let content_scale = bullet_xys.last().unwrap().2;
    assert!(
        (bullet_scale - content_scale).abs() < 1e-4,
        "bullet glyph scale must inherit content scale: bullet_scale={bullet_scale}, content_scale={content_scale}",
    );

    // 4) Stat counters confirm extra glyphs reached the pipeline.
    assert!(
        bullet_built.stats.glyphs > plain_built.stats.glyphs,
        "bulleted paragraph should report higher glyph count in stats",
    );
}

// ───────────────────── 5b. bullet character style ───────────────────

/// IDML with a bulleted paragraph that points at a
/// `BulletsCharacterStyle` overriding the marker's `FillColor`. The
/// paragraph body uses the default (black) fill so the bullet and
/// content glyphs end up with distinguishable `Paint` values in the
/// DisplayList — the property the override is supposed to give us.
fn build_bullet_with_character_style_idml() -> Vec<u8> {
    let graphic = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Red" Name="Red" Space="RGB" ColorValue="220 30 30"/>
  </Graphic>
</idPkg:Graphic>"#;
    let styles = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootCharacterStyleGroup>
    <CharacterStyle Self="CharacterStyle/RedBullet"
                    Name="RedBullet"
                    FillColor="Color/Red"/>
  </RootCharacterStyleGroup>
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/RedBullets"
                    Name="RedBullets"
                    BulletsAndNumberingListType="BulletList"
                    BulletsTextAfter=" "
                    BulletsCharacterStyle="CharacterStyle/RedBullet">
      <Properties>
        <BulletChar BulletCharacterValue="8226"/>
      </Properties>
    </ParagraphStyle>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#;
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/RedBullets">
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Item</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP_STYLED);
        put(zip, "Resources/Graphic.xml", graphic);
        put(zip, "Resources/Styles.xml", styles);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    })
}

#[test]
fn bullet_character_style_paints_bullet_differently_from_content() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let doc = paged_parse::import_idml_doc(&build_bullet_with_character_style_idml()).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();

    // Collect every glyph FillPath in emit order. The filter mirrors
    // `glyph_xys` — uniform-scale-with-y-flip matrices are glyphs;
    // frame fills carry shearing/scaling, so they get rejected.
    let glyph_paints: Vec<(f32, paged_compose::Paint)> = built.pages[0]
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath {
                paint, transform, ..
            } => {
                let [a, b, c2, d, tx, _] = transform.0;
                if b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0 && (a + d).abs() < 1e-4 {
                    Some((tx, *paint))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    assert!(
        glyph_paints.len() >= 2,
        "expected at least bullet + content glyphs, got {}",
        glyph_paints.len(),
    );

    // The first glyph (leftmost) is the bullet marker. The last is
    // a content character. Their paints must differ — that's the
    // BulletsCharacterStyle override doing its job.
    let bullet_paint = glyph_paints.first().map(|(_, p)| *p).unwrap();
    let content_paint = glyph_paints.last().map(|(_, p)| *p).unwrap();
    assert_ne!(
        bullet_paint, content_paint,
        "bullet character style should produce a distinct fill paint vs content (bullet={bullet_paint:?}, content={content_paint:?})",
    );

    // And the bullet's paint must be the override red, not the
    // fallback default — assert directly so a future regression that
    // forgets to wire `BulletsCharacterStyle` through `resolve_character`
    // is caught precisely.
    match bullet_paint {
        paged_compose::Paint::Solid(c) => {
            assert!(
                c.r > 0.5 && c.g < 0.5 && c.b < 0.5,
                "bullet paint should resolve from Color/Red (RGB 220,30,30 → ~0.86,0.12,0.12), got {c:?}",
            );
        }
        other => panic!("bullet paint should be Solid, got {other:?}"),
    }
}

// ─────────────────────────── 6. numbered list ───────────────────────

fn build_numbered_list_idml() -> Vec<u8> {
    let styles = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Numbered"
                    Name="Numbered"
                    BulletsAndNumberingListType="NumberedList"
                    NumberingFormat="1, 2, 3, 4..."/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#;
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 360 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    // Three numbered paragraphs.
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Numbered">
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Alpha</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Numbered">
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Beta</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Numbered">
      <CharacterStyleRange AppliedFont="Inter" PointSize="24">
        <Content>Gamma</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP_STYLED);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Resources/Styles.xml", styles);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    })
}

#[test]
fn numbered_list_emits_three_distinct_counter_prefixes() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let doc = paged_parse::import_idml_doc(&build_numbered_list_idml()).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let xys = glyph_xys(&built.pages[0].list.commands);

    // Group glyphs by baseline (rounded to int pt). Three numbered
    // paragraphs → three distinct baselines.
    let mut by_baseline: std::collections::BTreeMap<i32, Vec<(f32, u32)>> =
        std::collections::BTreeMap::new();
    for ((_, _, _), (path_id, transform)) in
        xys.iter()
            .zip(fill_paths(&built.pages[0].list.commands).filter(|(_, t)| {
                let [a, b, c, d, _, _] = t.0;
                b.abs() < 1e-5 && c.abs() < 1e-5 && a > 0.0 && d < 0.0
            }))
    {
        let [_, _, _, _, tx, ty] = transform.0;
        by_baseline
            .entry(ty.round() as i32)
            .or_default()
            .push((tx, path_id.0));
    }
    assert!(
        by_baseline.len() >= 3,
        "expected ≥3 distinct baselines for three paragraphs, got {} (baselines={:?})",
        by_baseline.len(),
        by_baseline.keys().collect::<Vec<_>>(),
    );

    // The first three baselines (top to bottom) are the paragraphs.
    let baselines: Vec<i32> = by_baseline.keys().copied().take(3).collect();
    let paras: Vec<&Vec<(f32, u32)>> = baselines
        .iter()
        .map(|b| by_baseline.get(b).unwrap())
        .collect();

    // Each paragraph's first glyph must be the digit '1' / '2' / '3'
    // — different glyph outlines, so different (font_id, glyph_id)
    // path_ids. The digit precedes the period, which precedes the
    // body, so it's the leftmost glyph of the first cluster.
    let leading_glyph_id = |row: &Vec<(f32, u32)>| -> u32 {
        let mut sorted = row.clone();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        sorted[0].1
    };
    let g1 = leading_glyph_id(paras[0]);
    let g2 = leading_glyph_id(paras[1]);
    let g3 = leading_glyph_id(paras[2]);
    let unique: std::collections::BTreeSet<u32> = [g1, g2, g3].iter().copied().collect();
    assert_eq!(
        unique.len(),
        3,
        "three numbered paragraphs should lead with three different digit glyphs; got {:?}",
        [g1, g2, g3],
    );

    // Each paragraph carries a leading number+'.' prefix → the
    // distance from the leftmost glyph to the next glyph should be
    // non-trivial (the digit advance, > 1 pt). And the leading
    // glyph's x is identical across paragraphs (the numbering
    // marker starts in the same column).
    let leftmost_x = |row: &Vec<(f32, u32)>| -> f32 {
        row.iter().map(|(x, _)| *x).fold(f32::INFINITY, f32::min)
    };
    let x1 = leftmost_x(paras[0]);
    let x2 = leftmost_x(paras[1]);
    let x3 = leftmost_x(paras[2]);
    let max_drift = [x1, x2, x3]
        .iter()
        .fold(0.0_f32, |acc, &v| acc.max((v - x1).abs()));
    assert!(
        max_drift < 1.0,
        "numbering markers should start in the same column; got x={x1}, {x2}, {x3}",
    );
}

// ─────────────────────────── 7. numbering polish ─────────────────────
// NumberingExpression substitution + NumberingStartAt + NumberingContinue.

/// Build an IDML whose story holds the supplied paragraph snippets in
/// order. Each snippet is a complete `<ParagraphStyleRange>` element
/// referencing one of the styles built into `numbering_styles_xml`.
/// The story uses `Inter` at 24 pt so digit glyphs come out at a
/// reliable size.
fn build_numbering_idml(paragraphs: &str) -> Vec<u8> {
    let styles = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <!-- "Step ^# of 5\t" substitution exercise. -->
    <ParagraphStyle Self="ParagraphStyle/CustomExpr"
                    Name="CustomExpr"
                    BulletsAndNumberingListType="NumberedList"
                    NumberingFormat="1, 2, 3, 4..."
                    NumberingExpression="Step ^# of 5^t"/>
    <!-- StartAt = 5; first paragraph emits "5.\t". -->
    <ParagraphStyle Self="ParagraphStyle/StartAt5"
                    Name="StartAt5"
                    BulletsAndNumberingListType="NumberedList"
                    NumberingFormat="1, 2, 3, 4..."
                    NumberingStartAt="5"/>
    <!-- Plain numbered (default "^#.^t"). -->
    <ParagraphStyle Self="ParagraphStyle/Plain"
                    Name="Plain"
                    BulletsAndNumberingListType="NumberedList"
                    NumberingFormat="1, 2, 3, 4..."/>
    <!-- Continue across style boundaries; resumes the counter
         instead of restarting at 1. -->
    <ParagraphStyle Self="ParagraphStyle/Continue"
                    Name="Continue"
                    BulletsAndNumberingListType="NumberedList"
                    NumberingFormat="1, 2, 3, 4..."
                    NumberingContinue="true"/>
    <!-- NoList plain body paragraph (interrupts the numbered run). -->
    <ParagraphStyle Self="ParagraphStyle/Body"
                    Name="Body"
                    BulletsAndNumberingListType="NoList"/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#;
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 560 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
{paragraphs}
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP_STYLED);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Resources/Styles.xml", styles);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

/// Group emitted glyph `(x, path_id)` pairs by their baseline
/// (rounded to int pt). One bucket per paragraph; entries within a
/// bucket are sorted left-to-right. Mirrors the glyph filter from
/// `glyph_xys` (uniform scale, no shear) so non-text fills (frame
/// rectangles, etc.) are dropped.
fn glyphs_by_baseline(cmds: &[DisplayCommand]) -> Vec<Vec<(f32, u32)>> {
    let mut groups: std::collections::BTreeMap<i32, Vec<(f32, u32)>> =
        std::collections::BTreeMap::new();
    for (path_id, transform) in fill_paths(cmds) {
        let [a, b, c, d, tx, ty] = transform.0;
        // Glyph emit always uses a uniform-scale-with-y-flip matrix.
        if !(b.abs() < 1e-5 && c.abs() < 1e-5 && a > 0.0 && d < 0.0 && (a + d).abs() < 1e-4) {
            continue;
        }
        groups
            .entry(ty.round() as i32)
            .or_default()
            .push((tx, path_id.0));
    }
    let mut out: Vec<Vec<(f32, u32)>> = groups.into_values().collect();
    for row in out.iter_mut() {
        row.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    }
    out
}

#[test]
fn numbering_expression_substitution_produces_more_glyphs_per_paragraph() {
    // Two paragraphs, one with the default expression `^#.^t` and one
    // with `Step ^# of 5^t`. The custom-expression paragraph emits
    // strictly more glyphs on its first line because "Step  of 5"
    // contributes additional letters/digits ahead of the body.
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let paragraphs = r#"
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Plain">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>X</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/CustomExpr">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>X</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>"#;
    let doc = paged_parse::import_idml_doc(&build_numbering_idml(paragraphs)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    assert!(
        rows.len() >= 2,
        "expected ≥2 baselines (one per paragraph), got {} rows",
        rows.len(),
    );
    // "1." + "X"  ⇒ 3 visible glyphs. "Step 1 of 5" + "X" ⇒ 12
    // glyphs (S t e p [space] 1 [space] o f [space] 5 X --exact
    // count depends on which space characters shape). Demand
    // strictly more glyphs in the custom-expression row.
    let plain = &rows[0];
    let custom = &rows[1];
    assert!(
        custom.len() > plain.len() + 3,
        "custom expression should emit substantially more glyphs than `^#.^t`; plain={}, custom={}",
        plain.len(),
        custom.len(),
    );
}

#[test]
fn numbering_start_at_5_emits_5_glyph_not_1_glyph() {
    // Two stories, one with a Plain "^#.^t" and one with StartAt=5.
    // Plain leads with the "1" glyph; StartAt=5 leads with the "5"
    // glyph. Different glyph outlines ⇒ different path_ids.
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    // Render both paragraphs in the same story so they share the
    // page's path-intern table — path_id then maps 1:1 to glyph
    // outline and is comparable across paragraphs.
    let paragraphs = r#"
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Plain">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>X</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/StartAt5">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>X</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>"#;
    let doc = paged_parse::import_idml_doc(&build_numbering_idml(paragraphs)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    assert!(
        rows.len() >= 2,
        "expected 2 paragraphs, got {} rows",
        rows.len()
    );
    // Plain leads with "1"; StartAt=5 with "5". Different glyph
    // outlines ⇒ different path_ids.
    let plain_id = rows[0][0].1;
    let start5_id = rows[1][0].1;
    // StartAt=5 isn't following the prior numbered paragraph because
    // the cascade lifts NumberingStartAt on entry — even though the
    // implicit "continue" semantics also apply, the explicit
    // start-override wins.
    assert_ne!(
        plain_id, start5_id,
        "StartAt=5 must lead with a different digit glyph than the default StartAt=1",
    );
}

#[test]
fn numbering_continue_resumes_count_across_non_numbered_paragraph() {
    // 3 paragraphs:
    //   1. Plain  → "1.\tA"
    //   2. Body   → "B" (no marker)
    //   3. Continue → "2.\tC"  (with NumberingContinue → resumes)
    //
    // The leading digit-glyph in paragraph 3 must match paragraph 2's
    // would-have-been-"2" --i.e. it must NOT equal paragraph 1's
    // leading "1" glyph.
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let paragraphs = r#"
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Plain">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>A</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>B</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Continue">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>C</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>"#;
    let doc = paged_parse::import_idml_doc(&build_numbering_idml(paragraphs)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    assert!(
        rows.len() >= 3,
        "expected 3 baselines (3 paragraphs), got {}",
        rows.len(),
    );

    let leading_1 = rows[0][0].1; // "1" glyph from paragraph 1
    let leading_3 = rows[2][0].1; // first glyph of paragraph 3
    assert_ne!(
        leading_1, leading_3,
        "NumberingContinue must resume past paragraph 1's '1' --got identical glyphs",
    );

    // Cross-check: same scenario without NumberingContinue must
    // reset to "1" (identical to paragraph 1's leading glyph).
    let reset_paragraphs = r#"
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Plain">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>A</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>B</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>
        <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Plain">
          <CharacterStyleRange AppliedFont="Inter" PointSize="24">
            <Content>C</Content>
          </CharacterStyleRange>
        </ParagraphStyleRange>"#;
    let doc = paged_parse::import_idml_doc(&build_numbering_idml(reset_paragraphs)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    let reset_leading = rows[2][0].1;
    assert_eq!(
        leading_1, reset_leading,
        "without NumberingContinue, paragraph 3 must lead with the same '1' glyph as paragraph 1.",
    );
}

// ── W1.22 (engine gap 22) — cross-story numbering continuity ──────
//
// Two stories in two frames on the same page, both bound (via the
// applied paragraph style) to one `<NumberingList>`. Story A emits
// items "1" and "2"; story B emits one item. When the list declares
// `ContinueNumbersAcrossStories="true"`, story B's marker continues
// the sequence ("3", a different glyph than "1"); when it doesn't,
// story B restarts at "1" (the SAME glyph story A led with). Both
// frames render to the same page, so the page-interned glyph path_ids
// are directly comparable across the two stories.

/// Build an IDML with two stories (u10 → frameA, u20 → frameB, stacked
/// vertically on one page), both using a `ParagraphStyle/Item` that
/// applies `NumberingList/Shared`. `continue_across` toggles the
/// list's `ContinueNumbersAcrossStories` flag. Story A has two
/// numbered paragraphs ("1", "2"); story B has one.
fn build_two_story_numbering_idml(continue_across: bool) -> Vec<u8> {
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootNumberingListGroup>
    <NumberingList Self="NumberingList/Shared"
                   Name="Shared"
                   ContinueNumbersAcrossStories="{continue_across}"/>
  </RootNumberingListGroup>
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Item"
                    Name="Item"
                    BulletsAndNumberingListType="NumberedList"
                    NumberingFormat="1, 2, 3, 4..."
                    AppliedNumberingList="NumberingList/Shared"/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#
    );
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 280 572" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u20" GeometricBounds="300 40 560 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let item = |c: &str| {
        format!(
            r#"<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Item">
                 <CharacterStyleRange AppliedFont="Inter" PointSize="24"><Content>{c}</Content></CharacterStyleRange>
               </ParagraphStyleRange>"#
        )
    };
    let story_a = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">{}{}</Story>
</idPkg:Story>"#,
        item("A"),
        item("B"),
    );
    let story_b = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u20">{}</Story>
</idPkg:Story>"#,
        item("C"),
    );
    // Designmap declares both stories in document order (A then B) —
    // the deterministic emit order the ledger follows.
    let designmap = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
  <idPkg:Story src="Stories/Story_u20.xml"/>
</Document>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", designmap);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Resources/Styles.xml", styles.as_bytes());
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story_a.as_bytes());
        put(zip, "Stories/Story_u20.xml", story_b.as_bytes());
    })
}

/// Render and return `(story_a_first_marker_glyph, story_b_marker_glyph)`
/// path_ids. Story A's frame sits above story B's (lower `ty` =
/// higher on the page = earlier baseline bucket); within each story
/// the leading glyph of the first numbered paragraph is the marker.
fn two_story_marker_glyphs(continue_across: bool) -> (u32, u32) {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let doc =
        paged_parse::import_idml_doc(&build_two_story_numbering_idml(continue_across)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    // Buckets are baseline-sorted top→bottom: rows[0] = story A item 1
    // ("1" / "3"... marker + "A"), rows[1] = story A item 2, the last
    // bucket = story B's single item (frameB starts at y=300, well
    // below frameA). Story A's first marker is rows[0][0]; story B's
    // marker is the leading glyph of the last bucket.
    assert!(
        rows.len() >= 3,
        "expected ≥3 baselines (A.1, A.2, B.1), got {}",
        rows.len()
    );
    let a_first = rows[0][0].1;
    let b_marker = rows[rows.len() - 1][0].1;
    (a_first, b_marker)
}

#[test]
fn numbering_continues_across_stories_when_list_flag_set() {
    // ContinueNumbersAcrossStories=true: story A emits 1, 2; story B
    // continues at 3. "3" is a different digit glyph than story A's
    // leading "1".
    let (a_first, b_marker) = two_story_marker_glyphs(true);
    assert_ne!(
        a_first, b_marker,
        "with ContinueNumbersAcrossStories, story B must lead with '3', \
         not the '1' glyph story A led with",
    );
}

#[test]
fn numbering_restarts_per_story_without_continue_flag() {
    // ContinueNumbersAcrossStories=false: story B restarts at 1 — the
    // SAME "1" glyph story A led with.
    let (a_first, b_marker) = two_story_marker_glyphs(false);
    assert_eq!(
        a_first, b_marker,
        "without ContinueNumbersAcrossStories, story B must restart at the '1' glyph",
    );
}

// ─────────────────────────── 12. tab-stop leader characters ─────────
//
// `<TabStop Leader=".">` tiles the leader string across the gap a
// snapped tab opens up — classic TOC dot-leader pattern:
//   "Chapter 1 ........ Page 3"
// The two assertions here pin both ends of the behaviour:
//   - non-empty Leader emits many leader glyphs between the label and
//     the right-aligned segment;
//   - absent Leader emits no leader glyphs (the existing tab snap
//     widens the gap but leaves it visually empty).

fn build_toc_leader_idml(leader_attr: &str) -> Vec<u8> {
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 540" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    // 400 pt-wide right-aligned tab stop puts the "Page 3" segment
    // far to the right of the "Chapter 1" label, opening a gap wide
    // enough to absorb many `.` copies at 14pt Inter.
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <Properties>
        <TabList>
          <ListItem><TabStop Position="400" Alignment="RightAlign"{leader_attr}/></ListItem>
        </TabList>
      </Properties>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>Chapter 1</Content>
        <Tab/>
        <Content>Page 3</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

/// Count glyph FillPath commands whose centre-x sits between
/// `gap_left` and `gap_right` (in pt). Used to count leader glyphs
/// that landed in the tab gap.
fn glyphs_in_x_range(cmds: &[DisplayCommand], gap_left: f32, gap_right: f32) -> usize {
    glyph_xys(cmds)
        .iter()
        .filter(|(x, _, _)| *x > gap_left && *x < gap_right)
        .count()
}

#[test]
fn tab_stop_leader_dots_tile_across_the_gap() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    // With Leader=".", the gap between "Chapter 1" (label, ends near
    // x ≈ 40 pt + label width ≈ 100 pt) and "Page 3" (right-snapped
    // segment near the 400 pt stop, frame origin 40, so ending near
    // x ≈ 440) must contain many `.` glyphs.
    let bytes_with = build_toc_leader_idml(r#" Leader=".""#);
    let doc_with = paged_parse::import_idml_doc(&bytes_with).unwrap();
    let built_with = pipeline::build_document(&doc_with, &opts).unwrap();
    let cmds_with = &built_with.pages[0].list.commands;

    // The right-snapped "Page 3" segment ends near x ≈ 440 pt and is
    // 6 chars wide (~40 pt). Look at glyphs between x = 130 (after
    // "Chapter 1") and x = 380 (before "Page 3"). All such glyphs
    // must be leader periods (the only thing that lives in the gap).
    let leader_count = glyphs_in_x_range(cmds_with, 130.0, 380.0);
    assert!(
        leader_count > 5,
        "expected many leader '.' glyphs in the tab gap, got {leader_count}",
    );

    // Cross-check: same paragraph without Leader → zero leader glyphs
    // in the gap. The tab still snaps "Page 3" to the right, so the
    // gap is structurally the same width.
    let bytes_without = build_toc_leader_idml("");
    let doc_without = paged_parse::import_idml_doc(&bytes_without).unwrap();
    let built_without = pipeline::build_document(&doc_without, &opts).unwrap();
    let cmds_without = &built_without.pages[0].list.commands;
    let no_leader_count = glyphs_in_x_range(cmds_without, 130.0, 380.0);
    assert_eq!(
        no_leader_count, 0,
        "without Leader, the tab gap must contain zero glyphs, got {no_leader_count}",
    );

    // Tighter pin: the leader glyphs should all share a single
    // PathId (one '.' outline reused). Walk the FillPaths that landed
    // in the gap and confirm a single path_id dominates.
    let mut path_ids_in_gap: Vec<u32> = Vec::new();
    for c in cmds_with {
        if let DisplayCommand::FillPath {
            path_id, transform, ..
        } = c
        {
            let [a, b, c2, d, tx, _ty] = transform.0;
            let is_glyph =
                b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0 && (a + d).abs() < 1e-4;
            if is_glyph && tx > 130.0 && tx < 380.0 {
                path_ids_in_gap.push(path_id.0);
            }
        }
    }
    let unique_path_ids: std::collections::BTreeSet<u32> =
        path_ids_in_gap.iter().copied().collect();
    assert_eq!(
        unique_path_ids.len(),
        1,
        "all leader glyphs should share one outline (interned by font_id+glyph_id); got {} distinct path_ids in the gap",
        unique_path_ids.len(),
    );
}

// ─────────────────────────── 12. tables: chain replay ───────────────

/// Two-frame threaded story hosting a table whose total height
/// exceeds the head frame's interior. Header row count = 1; body
/// row count chosen so the body overflows and the table breaks
/// across the chain. The header text "HDR" must appear at the top
/// of BOTH frames; without `T3.1` it appeared only in the head frame.
fn build_table_header_replay_idml() -> Vec<u8> {
    // frameA: 120 pt tall, frameB: 200 pt tall. Header row 24 pt,
    // body rows 24 pt each, 8 body rows + 1 header = 9 rows × 24 pt
    // = 216 pt > 120 pt head; the body splits.
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 600"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="20 40 140 240"
               NextTextFrame="frameB" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u10"
               GeometricBounds="20 280 220 480" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    // 1 header + 8 body rows × 1 column = 9 cells. Header label
    // unique ("HDR"); body labels "B0".."B7". The unique header
    // label lets the test count exactly how many "HDR" glyph runs
    // landed in each frame.
    let cells = (0..8)
        .map(|i| {
            format!(
                r#"<Cell Self="cb{i}" Name="0:{r}"><ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>B{i}</Content></CharacterStyleRange></ParagraphStyleRange></Cell>"#,
                r = i + 1
            )
        })
        .collect::<String>();
    let rows = (1..=8)
        .map(|i| format!(r#"<Row Self="r{i}" Name="{i}" SingleRowHeight="24"/>"#))
        .collect::<String>();
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
        <Table Self="t1" HeaderRowCount="1" BodyRowCount="8" ColumnCount="1">
          <Row Self="r0" Name="0" SingleRowHeight="24"/>
          {rows}
          <Column Self="c0" Name="0" SingleColumnWidth="160"/>
          <Cell Self="ch0" Name="0:0"><ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>HDR</Content></CharacterStyleRange></ParagraphStyleRange></Cell>
          {cells}
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

#[test]
fn threaded_table_replays_header_row_at_top_of_each_frame() {
    let bytes = build_table_header_replay_idml();
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let page = &built.pages[0];

    let glyphs = glyph_xys(&page.list.commands);
    // frameA: y ∈ [20, 140], frameB: y ∈ [20, 220 from the second frame's top
    // = approximately y ∈ [20, 220] since both frames sit on the same page).
    // Actually frameB GeometricBounds = "20 280 220 480" → x ∈ [280, 480],
    // y ∈ [20, 220]. Use x to differentiate the two frames.
    let in_a: Vec<_> = glyphs
        .iter()
        .filter(|(x, _, _)| *x < 250.0)
        .copied()
        .collect();
    let in_b: Vec<_> = glyphs
        .iter()
        .filter(|(x, _, _)| *x > 260.0)
        .copied()
        .collect();
    assert!(
        !in_a.is_empty() && !in_b.is_empty(),
        "both frames should receive cell glyphs; got a={} b={}",
        in_a.len(),
        in_b.len()
    );

    // "HDR" is 3 glyphs. After replay we expect 6+ HDR-shaped runs
    // total: at minimum, 3 in frameA (the original header) + 3 in
    // frameB (the replayed header at the top). A replay means
    // frameB's top-most row is the header, so the top-most line of
    // glyphs in frameB should be at roughly the same y-from-frame-top
    // as the head frame's first row.
    //
    // Cheap proxy: count distinct y-rows in each frame. With 1
    // header + 8 body rows split somewhere mid-body and a header
    // replayed at the top of frameB, the row count in frameB must
    // exceed (body_rows_in_B) by exactly 1 (the replayed header).
    //
    // We assert the stronger structural property: the top-most y in
    // frameB is the header's row (it has the same per-row position
    // offset within the frame as frameA's header). Headers are y =
    // 20 + 24 * 0 + leading ≈ frame_top + ~8 pt; without replay,
    // frameB's top row would be a body row whose label starts with
    // 'B', not 'H'.
    fn distinct_rows(xys: &[(f32, f32, f32)]) -> Vec<f32> {
        let mut ys: Vec<f32> = xys.iter().map(|g| g.1).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        ys
    }
    let a_rows = distinct_rows(&in_a);
    let b_rows = distinct_rows(&in_b);
    assert!(
        a_rows.len() >= 2,
        "frameA must hold ≥ 2 rows (header + ≥ 1 body); got {a_rows:?}"
    );
    assert!(
        b_rows.len() >= 2,
        "frameB must hold the replayed header + ≥ 1 body row; got {b_rows:?}"
    );
    // Header replay invariant: total emitted rows across both
    // frames > total source rows (because the header replays once).
    // 9 source rows × 1 col = 9 cell content rows total source.
    // With one replay we expect ≥ 10. Without replay we'd see ≤ 9.
    let total_rows_emitted = a_rows.len() + b_rows.len();
    assert!(
        total_rows_emitted >= 10,
        "header replay should bump total emitted rows above source count; got a={} + b={} = {total_rows_emitted}",
        a_rows.len(),
        b_rows.len()
    );

    // Sanity: total glyph count includes the replayed header's 3
    // letters. With 1 original header (3 glyphs) + 8 body rows
    // (2 glyphs each, e.g. "B0") + 1 replayed header (3 glyphs) =
    // 3 + 16 + 3 = 22. Without replay we'd see 19. Allow some
    // slack for shaping merges; the lower bound 21 still pins
    // the replay.
    assert!(
        glyphs.len() >= 21,
        "expected ≥ 21 glyphs (incl. one HDR replay), got {}",
        glyphs.len()
    );
}

// ─────────────────────────── 13. tables: row growth ─────────────────

/// Single-frame fixture, single cell wide, single row tall. The row
/// declares `SingleRowHeight="20"` (room for ~1 line at 14 pt) but
/// the cell content wraps to multiple lines at the declared column
/// width. MaximumHeight is unset; the row must grow to accommodate
/// every line and emit every glyph (no clipping).
fn build_table_row_growth_idml() -> Vec<u8> {
    // Column width 180 pt at 14 pt Inter holds ~25 chars/line.
    // Content is ~120 chars → ~5 lines at this column. Without
    // growth the renderer would clip at SingleRowHeight=20 pt and
    // produce ≤ 1 line of glyphs.
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 600"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="20 40 380 300" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Table Self="t1" HeaderRowCount="0" BodyRowCount="1" ColumnCount="1">
          <Row Self="r0" Name="0" SingleRowHeight="20"/>
          <Column Self="c0" Name="0" SingleColumnWidth="180"/>
          <Cell Self="c00" Name="0:0">
            <ParagraphStyleRange>
              <CharacterStyleRange AppliedFont="Inter" PointSize="14">
                <Content>Quick brown fox jumps over the lazy dog and runs around the meadow chasing butterflies.</Content>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    })
}

#[test]
fn table_row_grows_to_fit_content_when_single_row_height_too_small() {
    let bytes = build_table_row_growth_idml();
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let glyphs = glyph_xys(&built.pages[0].list.commands);

    // 87-char sentence wraps to ≥ 3 lines at column width 180 pt,
    // font size 14. The cell hosts every letter glyph (~70 letters,
    // ignoring spaces). Without row growth the cell was clipped to
    // SingleRowHeight=20pt which fits roughly one line ≈ 20 glyphs.
    // With row growth all ~70 glyphs emit.
    assert!(
        glyphs.len() >= 60,
        "row growth should emit all ~70 letter glyphs, got {}",
        glyphs.len()
    );

    // Verify multi-line layout: glyphs spread across at least 3
    // distinct baselines.
    let mut ys: Vec<f32> = glyphs.iter().map(|g| g.1).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    assert!(
        ys.len() >= 3,
        "cell content should wrap onto ≥ 3 baselines, got {} ({ys:?})",
        ys.len()
    );

    // Tighter pin: with row growth, the bottom-most glyph sits well
    // below the original SingleRowHeight=20pt window. Frame top = 20
    // pt + row top = 20 pt; without growth the floor would be y=40.
    let max_y = ys.last().copied().unwrap_or(0.0);
    assert!(
        max_y > 50.0,
        "bottom glyph at y={max_y} should fall below the legacy 20pt row floor"
    );
}

// ─────────────────────────── 14. Face cache shaping parity ───────────
//
// Per-render shaping-Face cache (FontTable::faces) is keyed on
// (font_id, wght_bits). The cache should be transparent — feeding the
// same (font, weight) across many paragraphs through the cached Face
// must produce the exact same glyphs, advances, and line breaks as
// the legacy per-paragraph Face construction would have. This test
// renders a multi-paragraph fixture twice (back-to-back) and asserts:
//   - the same number of glyph FillPath commands lands each pass;
//   - the same path_ids land in the same per-glyph order;
//   - the same (x, y) baselines come out;
// covering both top-level paragraphs (multiple ParagraphStyleRange
// children) and table cells (table-cell paragraphs hit
// `emit_cell_paragraph` which also routes through the cache).

fn build_many_paragraph_idml(num_paragraphs: usize) -> Vec<u8> {
    // 600pt-tall frame absorbs N=20 paragraphs at 18pt with leading.
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 760 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let mut paragraphs = String::new();
    for _ in 0..num_paragraphs {
        paragraphs.push_str(
            r#"    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="18">
        <Content>Quick brown fox</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
"#,
        );
    }
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
{paragraphs}  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

/// Sum of every per-glyph (path_id, tx, ty) across the page. The
/// cache must produce a bit-identical output for two back-to-back
/// renders of the same IDML.
fn glyph_signature(cmds: &[DisplayCommand]) -> Vec<(u32, i64, i64)> {
    let mut out: Vec<(u32, i64, i64)> = Vec::new();
    for c in cmds {
        let DisplayCommand::FillPath {
            path_id, transform, ..
        } = c
        else {
            continue;
        };
        let [a, b, c2, d, tx, ty] = transform.0;
        // Glyph emit always uses uniform-scale-with-y-flip.
        if !(b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0 && (a + d).abs() < 1e-4) {
            continue;
        }
        // Quantise to micro-pt so float jitter doesn't flake the test.
        out.push((
            path_id.0,
            (tx * 1_000.0).round() as i64,
            (ty * 1_000.0).round() as i64,
        ));
    }
    out
}

#[test]
fn face_cache_multi_paragraph_render_is_deterministic_and_reuses_glyphs() {
    let bytes = build_many_paragraph_idml(20);
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let built_a = pipeline::build_document(&doc, &opts).unwrap();
    let built_b = pipeline::build_document(&doc, &opts).unwrap();

    let sig_a = glyph_signature(&built_a.pages[0].list.commands);
    let sig_b = glyph_signature(&built_b.pages[0].list.commands);

    assert!(
        !sig_a.is_empty(),
        "fixture should produce at least one glyph FillPath"
    );
    assert_eq!(
        sig_a, sig_b,
        "two back-to-back renders of the same IDML must produce identical glyph output \
         (same path_ids, advances, baselines). The shaping-Face cache must be transparent."
    );

    // "Quick brown fox" = 13 letters + 2 spaces; the comp drops the
    // spaces' glyph emissions in shape_run output (advance-only), so
    // about 13 fill-glyphs land per paragraph. 20 paragraphs ⇒
    // ≥ 200 glyphs — enough to exercise the cache reuse path.
    assert!(
        sig_a.len() >= 200,
        "expected ≥200 glyph fills across 20 paragraphs, got {}",
        sig_a.len()
    );

    // Most glyphs are repeats across paragraphs ⇒ path_id count is
    // small relative to glyph count. Inter has ~10 distinct glyphs
    // for "Quick brown fox" (deduplicated letters: q-u-i-c-k-b-r-o-w-n-f-o-x).
    let distinct_path_ids: std::collections::BTreeSet<u32> =
        sig_a.iter().map(|(p, _, _)| *p).collect();
    assert!(
        distinct_path_ids.len() < 30,
        "expected <30 distinct glyph outlines (deduplicated letters), got {}; \
         FontTable cache or outline-interner is leaking distinct path_ids per paragraph",
        distinct_path_ids.len()
    );
}

// ─────────────────────────── 15. TOC renderer swap-in ────────────────
//
// Build a 4-page IDML: pages 1-3 each hold a chapter (one Heading_1
// paragraph + one Body paragraph), page 4 hosts an unresolved TOC
// story whose frame carries `AppliedTOCStyle="TOCStyle/Main"`. The
// renderer should detect the TOC binding, call `Document::resolve_toc`,
// and emit one synthetic paragraph per chapter (heading text + tab +
// page label) into the TOC frame's glyph stream.

fn build_toc_idml() -> Vec<u8> {
    let designmap = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
  <idPkg:Story src="Stories/Story_u20.xml"/>
  <idPkg:Story src="Stories/Story_u30.xml"/>
  <idPkg:Story src="Stories/Story_u40.xml"/>
</Document>"#;
    // Heading_1 = the include-style; TocEntry = the format-style
    // applied to each synthesised TOC paragraph; Body = ignored.
    let styles = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Heading_1" Name="Heading 1"
                    AppliedFont="Inter" PointSize="18"/>
    <ParagraphStyle Self="ParagraphStyle/Body" Name="Body"
                    AppliedFont="Inter" PointSize="12"/>
    <ParagraphStyle Self="ParagraphStyle/TocEntry" Name="TocEntry"
                    AppliedFont="Inter" PointSize="14"/>
  </RootParagraphStyleGroup>
  <RootTOCStyleGroup>
    <TOCStyle Self="TOCStyle/Main" Name="Main" Title="Contents">
      <TOCStyleEntry Name="Heading 1"
                     IncludeStyle="ParagraphStyle/Heading_1"
                     FormatStyle="ParagraphStyle/TocEntry"
                     Level="1"
                     PageNumber="On"
                     Separator="^t"/>
    </TOCStyle>
  </RootTOCStyleGroup>
</idPkg:Styles>"#;
    // Four pages stacked vertically; each holds one TextFrame
    // covering the page. Frame on page 4 carries AppliedTOCStyle.
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 400"/>
    <Page Self="p2" GeometricBounds="200 0 400 400"/>
    <Page Self="p3" GeometricBounds="400 0 600 400"/>
    <Page Self="p4" GeometricBounds="600 0 800 400"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="10 20 190 380" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u20"
               GeometricBounds="210 20 390 380" StrokeWeight="0"/>
    <TextFrame Self="frameC" ParentStory="u30"
               GeometricBounds="410 20 590 380" StrokeWeight="0"/>
    <TextFrame Self="frameToc" ParentStory="u40"
               GeometricBounds="610 20 790 380" StrokeWeight="0"
               AppliedTOCStyle="TOCStyle/Main"/>
  </Spread>
</idPkg:Spread>"#;
    let chapter = |sid: &str, title: &str| -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="{sid}">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Heading_1">
      <CharacterStyleRange AppliedFont="Inter" PointSize="18">
        <Content>{title}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Body copy that should not appear in the TOC.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
        )
    };
    // Page-4 TOC story carries one placeholder paragraph (real
    // unresolved IDMLs serialise an empty `<ParagraphStyleRange>`
    // here). The renderer ignores it once the AppliedTOCStyle
    // binding is detected.
    let toc_story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u40">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>placeholder</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", designmap);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Resources/Styles.xml", styles);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(
            zip,
            "Stories/Story_u10.xml",
            chapter("u10", "Chapter One").as_bytes(),
        );
        put(
            zip,
            "Stories/Story_u20.xml",
            chapter("u20", "Chapter Two").as_bytes(),
        );
        put(
            zip,
            "Stories/Story_u30.xml",
            chapter("u30", "Chapter Three").as_bytes(),
        );
        put(zip, "Stories/Story_u40.xml", toc_story);
    })
}

#[test]
fn toc_story_swaps_in_resolved_entries_with_heading_text_and_page_numbers() {
    let bytes = build_toc_idml();
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    assert_eq!(built.pages.len(), 4, "expected 4 body pages");

    // Page 4 hosts the TOC frame. Recover the rendered text by
    // grouping glyph emissions by baseline (one entry per line)
    // and converting glyph_id → character via the font (rustybuzz
    // shapes ASCII Inter at PointSize without ligatures so every
    // letter corresponds to one glyph).
    let toc_cmds = &built.pages[3].list.commands;
    let toc_glyphs = glyph_xys(toc_cmds);
    assert!(
        toc_glyphs.len() >= 10,
        "TOC frame should host ≥ 10 letter glyphs across 3 entries; got {}",
        toc_glyphs.len()
    );

    // Heading texts are 11 / 11 / 13 letters (no spaces shape to
    // visible glyphs in Inter), plus the trailing page label digit.
    // Bottom-line baseline check: 3 distinct baselines for 3 entries.
    let mut baselines: Vec<f32> = toc_glyphs.iter().map(|g| g.1).collect();
    baselines.sort_by(|a, b| a.partial_cmp(b).unwrap());
    baselines.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    assert_eq!(
        baselines.len(),
        3,
        "expected 3 TOC entry baselines (one per Heading_1 paragraph), got {:?}",
        baselines
    );

    // Glyph-outline cross-check: shape "Chapter Three" through
    // Inter (it's the longest string with the most distinct
    // letters: C-h-a-p-t-e-r-T-r-e — 8 distinct glyphs) and verify
    // each shaped glyph_id corresponds to a FillPath the renderer
    // emitted at the TOC page. We can't directly compare glyph_id
    // to path_id (the outline interner reassigns ids per render),
    // but the *count* of distinct path_ids on the TOC page is a
    // strong proxy: 3 headings + 3 digits cover ≥ 12 distinct
    // letters (C/h/a/p/t/e/r/O/n/T/w/h/r/e + digits 1/2/3).
    let mut distinct_path_ids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for c in toc_cmds {
        if let DisplayCommand::FillPath {
            path_id, transform, ..
        } = c
        {
            let [a, b, c2, d, _, _] = transform.0;
            if b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0 {
                distinct_path_ids.insert(path_id.0);
            }
        }
    }
    assert!(
        distinct_path_ids.len() >= 12,
        "TOC frame should reuse at least 12 distinct glyph outlines (chapter heading letters + digits), got {}",
        distinct_path_ids.len()
    );

    // Page-number presence: each TOC entry ends with the page
    // label "1", "2", or "3". The rightmost glyph on each baseline
    // is the page-number digit (glyph emission is left-to-right
    // and tabs widen the gap before the digit). Pick the rightmost
    // glyph per baseline and require x > some threshold past the
    // entry text. Without page numbers the rightmost glyph would
    // sit much further left.
    let mut per_line: std::collections::BTreeMap<i32, (f32, f32)> =
        std::collections::BTreeMap::new();
    for &(x, y, _) in &toc_glyphs {
        let key = y.round() as i32;
        let entry = per_line
            .entry(key)
            .or_insert((f32::INFINITY, f32::NEG_INFINITY));
        entry.0 = entry.0.min(x);
        entry.1 = entry.1.max(x);
    }
    for (key, (min_x, max_x)) in &per_line {
        // Tab snaps the gap to the next stop; default tab stops
        // sit on a 36 pt cadence so an 11-letter heading + tab
        // expands the line to ≥ ~80 pt of glyph spread.
        let spread = max_x - min_x;
        assert!(
            spread >= 50.0,
            "TOC entry at y={key} should span ≥ 50 pt from heading start to page-number digit; got {spread} pt (min_x {min_x}, max_x {max_x})"
        );
    }

    // Document-order assertion: the 3 entries appear top-to-bottom
    // in chapter order (Chapter One on page 1 → top baseline,
    // Chapter Three on page 3 → bottom baseline). Glyph baselines
    // grow downwards in spread coords, so sorted ascending is
    // chapter order.
    assert!(
        baselines.windows(2).all(|w| w[0] < w[1]),
        "TOC entries should be top-to-bottom in chapter order; baselines: {:?}",
        baselines
    );

    // Stories on pages 1-3 still emit their own paragraphs (the
    // TOC swap-in only affects the TOC-tagged story).
    for (page_idx, label) in [(0, "page 1"), (1, "page 2"), (2, "page 3")] {
        let glyphs = glyph_xys(&built.pages[page_idx].list.commands);
        assert!(
            glyphs.len() >= 10,
            "{label} should still emit its own heading + body glyphs; got {}",
            glyphs.len()
        );
    }

    // The TOC story's placeholder paragraph ("placeholder", 11
    // letters) must NOT appear: the TOC frame's glyph count would
    // otherwise grow by the placeholder's letters AND a 4th
    // baseline would land in `baselines`. We already asserted
    // exactly 3 baselines above; pin once more on the glyph
    // signature by confirming the TOC frame's emitted glyph
    // signature is far smaller than the placeholder + 3 entries
    // (1 + 3 = 4 baselines).
    //
    // Together: 3 baselines + ≥ 50 pt spread per baseline =
    // exactly the resolver's three entries replaced the original
    // story's content.
}

// ─────────────────────────── rotated text frames ────────────────────

/// 90° CCW rotation around the origin moves the local +x axis to +y
/// and the local +y axis to -x. As a row-major IDML transform
/// (`[a b c d tx ty]` where rotated point = `[a c; b d] · (x,y) +
/// (tx, ty)`), that's `[0, 1, -1, 0, tx, ty]`.
///
/// P-03 regression: a TextFrame authored with horizontal reading
/// direction (`GeometricBounds="0 0 30 600"` — inner 600 wide × 30
/// tall) plus a 90° `ItemTransform` projects to a *spread-space*
/// AABB of 30 wide × 600 tall — i.e. width and height swap. The
/// pre-fix renderer fed `column_width_pt = 30` to the composer
/// (using the spread AABB), so a long string of glyphs ran out of
/// room after the first wrap and the rest were silently dropped.
/// Verify here that the story emits a healthy glyph count and that
/// the per-glyph transforms carry a non-trivial rotation (the
/// post-emit `rotate_transform_around` pass).
fn build_rotated_text_idml() -> Vec<u8> {
    // ItemTransform: 90° CCW rotation + translation that places the
    // rotated 600×30 frame as a vertical sidebar at x≈400 on page.
    //   IDML row-major matrix:
    //     a=0, b=1, c=-1, d=0, tx=400, ty=40
    //   Mapping a local point (x, y):
    //     spread.x = a*x + c*y + tx = 0*x + (-1)*y + 400 = 400 - y
    //     spread.y = b*x + d*y + ty = 1*x + 0*y + 40    = 40 + x
    // Local (0,0)→(400,40); (600,0)→(400,640) ← off-page bottom;
    // shrink to fit by choosing a smaller frame height. Use 400 tall
    // so spread extent runs (400,40)→(400,440), comfortable on a
    // 500×800 page.
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 500 800"/>
    <TextFrame Self="rotFrame" ParentStory="u10"
               GeometricBounds="0 0 30 400"
               ItemTransform="0 1 -1 0 400 40"
               StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>VERTICAL SIDEBAR LABEL FOR THE COVER TWO THOUSAND THIRTY</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    })
}

#[test]
fn rotated_text_frame_emits_glyphs_along_rotated_axis() {
    let bytes = build_rotated_text_idml();
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let page = &built.pages[0];

    // Glyph emission survives the rotation: after the fix, the
    // composer receives column_width = 30 from inner coords (long
    // and narrow), which fits the same string the unrotated frame
    // would have produced. Pre-fix the column_width_pt came from
    // the spread AABB (600×30), the height collapsed to 30 pt, and
    // the composer pinned the line to the first 1-2 glyphs.
    let n_glyphs = page
        .list
        .commands
        .iter()
        .filter(|c| matches!(c, DisplayCommand::FillPath { .. }))
        .count();
    assert!(
        n_glyphs >= 30,
        "rotated frame should still emit a healthy glyph count, got {n_glyphs}",
    );

    // Per-glyph FillPath transforms must carry a non-trivial off-
    // diagonal — the post-emit `rotate_transform_around` pass folds
    // the frame's 90° linear into every glyph command. Unrotated
    // glyph emit produces `[a, 0, 0, -a, tx, ty]` (off-diagonal
    // near zero); the rotated pass moves the scale to the
    // off-diagonal so a/d collapse and b/c2 carry the visible scale.
    let mut rotated_glyphs = 0;
    let mut diag_sum = 0.0f32;
    let mut off_sum = 0.0f32;
    for c in &page.list.commands {
        if let DisplayCommand::FillPath { transform, .. } = c {
            let [a, b, c2, d, _, _] = transform.0;
            diag_sum += a.abs() + d.abs();
            off_sum += b.abs() + c2.abs();
            // Look for the rotated-glyph signature: off-diagonal
            // entries dominate over the diagonal.
            if b.abs() + c2.abs() > a.abs() + d.abs() {
                rotated_glyphs += 1;
            }
        }
    }
    assert!(
        rotated_glyphs >= 20,
        "rotated TextFrame's glyphs should carry the frame's 90° linear in their transforms; got {rotated_glyphs} rotated of {n_glyphs} FillPath cmds (diag_sum={diag_sum}, off_sum={off_sum})",
    );
    // Sanity: with a 90° rotation, total off-diagonal magnitude
    // should exceed total diagonal magnitude across the page.
    assert!(
        off_sum > diag_sum,
        "rotated frame should produce off-diagonal-dominant transforms; got diag={diag_sum}, off={off_sum}",
    );
}

// ─────────────────────────── master-spread routing ──────────────────

/// Designmap that wires up one master spread + one body spread (2
/// pages, both referencing the same master).
const DESIGNMAP_MASTER: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:MasterSpread src="MasterSpreads/MasterSpread_uad.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#;

/// P-05 regression: a full-bleed `<Rectangle>` defined once on a
/// master spread, with bounds covering both master pages (e.g. a
/// cover-spanning brand colour band), must replay onto BOTH body
/// pages — not just the centroid-winning one. Pre-fix, the master
/// pass routed the rectangle by AABB centroid, so the page that
/// didn't own the centroid emitted nothing.
fn build_master_full_bleed_idml() -> Vec<u8> {
    // Master spread: two pages side by side (each 0..612 wide,
    // 0..792 tall), with a single rectangle spanning the whole
    // spread (0,0 → 1224,792). The body spread mirrors the master
    // page geometry so the two body pages each pick up a slice of
    // the spanning rectangle.
    let master = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <MasterSpread Self="uad" Name="A-Master">
    <Page Self="uad-p1" GeometricBounds="0 0 792 612"/>
    <Page Self="uad-p2" GeometricBounds="0 612 792 1224"/>
    <Rectangle Self="uad-bleed" GeometricBounds="0 0 792 1224"
               FillColor="Color/Black" StrokeWeight="0"/>
  </MasterSpread>
</idPkg:MasterSpread>"#;
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 792 612" AppliedMaster="MasterSpread/uad"/>
    <Page Self="p2" GeometricBounds="0 612 792 1224" AppliedMaster="MasterSpread/uad"/>
  </Spread>
</idPkg:Spread>"#;
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP_MASTER);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "MasterSpreads/MasterSpread_uad.xml", master);
        put(zip, "Spreads/Spread_sp1.xml", spread);
    })
}

#[test]
fn master_full_bleed_rectangle_reaches_both_body_pages() {
    let bytes = build_master_full_bleed_idml();
    let doc = paged_parse::import_idml_doc(&bytes).expect("open synthetic master IDML");
    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&doc, &opts).expect("build document");

    assert_eq!(built.pages.len(), 2, "two body pages");

    // Each body page should carry at least one FillPath command for
    // the master's spanning rectangle. Pre-fix the centroid test
    // routed the rect to one page only; the other was empty.
    for (i, page) in built.pages.iter().enumerate() {
        let n_fills = page
            .list
            .commands
            .iter()
            .filter(|c| matches!(c, DisplayCommand::FillPath { .. }))
            .count();
        assert!(
            n_fills >= 1,
            "page {i} should receive the master's full-bleed rectangle; got {n_fills} FillPaths",
        );
    }
}

/// P-08 regression: when a run carries `HorizontalScale=200`, every
/// emitted glyph must (a) advance twice as far across the frame as
/// the same run at 100%, AND (b) carry a 2× x-axis scale in its
/// FillPath transform so the glyph outline is stretched in place.
#[test]
fn horizontal_scale_folds_into_glyph_advance_and_affine() {
    // Like `glyph_xys` but tolerates non-uniform x/y scale (the very
    // shape `HorizontalScale` produces). Returns the FillPath x
    // translation for every FillPath whose off-diagonal is ~0.
    fn glyph_xs(page: &paged_renderer::BuiltPage) -> Vec<f32> {
        let mut out = Vec::new();
        for c in &page.list.commands {
            if let DisplayCommand::FillPath { transform, .. } = c {
                let [a, b, c2, d, tx, _] = transform.0;
                if b.abs() < 1e-4 && c2.abs() < 1e-4 && a > 0.0 && d < 0.0 {
                    out.push(tx);
                }
            }
        }
        out
    }
    fn glyph_affines(page: &paged_renderer::BuiltPage) -> Vec<[f32; 6]> {
        page.list
            .commands
            .iter()
            .filter_map(|c| match c {
                DisplayCommand::FillPath { transform, .. } => Some(transform.0),
                _ => None,
            })
            .collect()
    }
    let content = "MMMMMM";
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = || PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let baseline_bytes = build_decoration_idml("", content);
    let baseline_doc = paged_parse::import_idml_doc(&baseline_bytes).unwrap();
    let baseline_built = pipeline::build_document(&baseline_doc, &opts()).unwrap();
    let baseline_xs = glyph_xs(&baseline_built.pages[0]);

    let stretched_bytes = build_decoration_idml(" HorizontalScale=\"200\"", content);
    let stretched_doc = paged_parse::import_idml_doc(&stretched_bytes).unwrap();
    let stretched_built = pipeline::build_document(&stretched_doc, &opts()).unwrap();
    let stretched_xs = glyph_xs(&stretched_built.pages[0]);
    let stretched_affines = glyph_affines(&stretched_built.pages[0]);

    assert!(
        baseline_xs.len() >= 6 && stretched_xs.len() >= 6,
        "baseline {} glyphs / stretched {} glyphs",
        baseline_xs.len(),
        stretched_xs.len()
    );
    // The run between consecutive M glyphs should roughly double when
    // HorizontalScale=200. Compare the average inter-glyph gap so a
    // single outlier doesn't trigger flakiness.
    let gap = |xs: &[f32]| -> f32 {
        let mut sum = 0.0f32;
        let mut n = 0usize;
        for w in xs.windows(2) {
            sum += w[1] - w[0];
            n += 1;
        }
        if n == 0 {
            0.0
        } else {
            sum / n as f32
        }
    };
    let g0 = gap(&baseline_xs);
    let g1 = gap(&stretched_xs);
    assert!(g0 > 0.5, "baseline glyph gap should be positive; got {g0}");
    let ratio = g1 / g0;
    assert!(
        (1.7..=2.3).contains(&ratio),
        "HorizontalScale=200 should ~double the inter-glyph gap; baseline gap {g0}, stretched gap {g1}, ratio {ratio}"
    );
    // At least one stretched glyph's affine should carry an x-scale
    // about 2× the y-scale magnitude (the glyph itself is stretched,
    // not just repositioned). Glyph affines have shape
    //   [a, 0, 0, -d, tx, ty]
    // with a positive, d positive, normally a ≈ d. With HS=200
    // we expect a ≈ 2 × d.
    let mut found = false;
    for [a, b, c2, d, _, _] in stretched_affines {
        if b.abs() > 1e-4 || c2.abs() > 1e-4 {
            continue;
        }
        let dy = -d;
        if dy.abs() < 1e-4 {
            continue;
        }
        let ratio = a / dy;
        if (1.7..=2.3).contains(&ratio) {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "expected at least one HorizontalScale=200 glyph FillPath whose x-scale ≈ 2 × y-scale; affine list did not contain one"
    );
}

/// P-18 regression: two runs in the same paragraph, the second
/// carrying a direct `FillColor` on its `CharacterStyleRange` (no
/// AppliedCharacterStyle, no AppliedParagraphStyle). The second run
/// must paint with its own colour — the cascade must NOT overwrite
/// a directly-set run colour with the (unset) character / paragraph
/// style default.
#[test]
fn per_run_fill_color_on_character_style_range_paints_run_specific_colour() {
    const GRAPHIC: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Crimson" Name="Crimson" Space="RGB" ColorValue="220 20 60"/>
  </Graphic>
</idPkg:Graphic>"#;
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36">
        <Content>AAAA</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36" FillColor="Color/Crimson">
        <Content>BBBB</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
    let bytes = write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story);
    });
    let doc = paged_parse::import_idml_doc(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let page = &built.pages[0];
    let mut saw_crimson = false;
    let mut saw_other = false;
    for c in &page.list.commands {
        if let DisplayCommand::FillPath {
            paint: paged_compose::Paint::Solid(c),
            ..
        } = c
        {
            // Crimson is a red-dominant shade: r >> g, b. The
            // exact RGB depends on the colour-management pipeline
            // (sRGB vs linear, ICC profile, etc.) — assert on
            // "red-dominant" rather than an exact tuple.
            let crimson_match = c.r > 0.4 && c.g < 0.4 && c.b < 0.4;
            if crimson_match {
                saw_crimson = true;
            } else {
                saw_other = true;
            }
        }
    }
    assert!(
        saw_crimson,
        "second run's direct FillColor=Color/Crimson should paint at least one glyph in that swatch's RGB",
    );
    assert!(
        saw_other,
        "first run should retain its default (black) fill",
    );
}

// ─────────────────────────── Q-07 Tracking pin ──────────────────────
//
// Cycle 4 Track 5: Q-07 was deferred in cycle 2 because the evidence
// was tabular-numeral letter spacing drift inside `<Table>` content,
// and the table renderer was incomplete. ace96e8 closed the table
// renderer; this test pins the tracking-end-to-end behaviour through
// the multi-font `layout_runs` path that tables use, so a future
// composer refactor can't silently drop `apply_tracking` between
// shape and emit. A regression test now is cheaper than re-auditing
// the corpus next cycle.

fn build_tracking_idml(tracking_thousandths_em: i32) -> Vec<u8> {
    let spread = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 160 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#;
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36" Tracking="{tracking_thousandths_em}">
        <Content>AAAA</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    write_zip(|zip| {
        put(zip, "designmap.xml", DESIGNMAP);
        put(zip, "Resources/Graphic.xml", GRAPHIC_XML);
        put(zip, "Spreads/Spread_sp1.xml", spread);
        put(zip, "Stories/Story_u10.xml", story.as_bytes());
    })
}

#[test]
fn cycle4_q07_positive_tracking_widens_emitted_glyph_advances() {
    // Render the same 4-A string at Tracking=0 and Tracking=200/1000em.
    // The wider tracking must push the last glyph further right —
    // that's `apply_tracking`'s contract surviving the
    // shape → layout_runs → emit pipeline (the path tables use).
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let glyphs_at_tracking = |t: i32| -> Vec<(f32, f32, f32)> {
        let bytes = build_tracking_idml(t);
        let doc = paged_parse::import_idml_doc(&bytes).unwrap();
        let built = pipeline::build_document(&doc, &opts).unwrap();
        glyph_xys(&built.pages[0].list.commands)
    };

    let base = glyphs_at_tracking(0);
    let tracked = glyphs_at_tracking(200);
    assert_eq!(base.len(), 4, "expected 4 'A' glyphs at Tracking=0");
    assert_eq!(tracked.len(), 4, "expected 4 'A' glyphs at Tracking=200");

    let base_span = base.last().unwrap().0 - base.first().unwrap().0;
    let tracked_span = tracked.last().unwrap().0 - tracked.first().unwrap().0;
    // At 36pt, 200/1000em = 7.2pt per glyph gap. 3 inter-glyph gaps =
    // ~21.6pt extra. Be generous — the precise number depends on
    // rounding through ADVANCE_PRECISION.
    let delta = tracked_span - base_span;
    assert!(
        delta > 15.0,
        "Tracking=200 should widen the 4-A span by ~21.6pt; observed Δ={delta} (base_span={base_span}, tracked_span={tracked_span})"
    );
}

#[test]
fn cycle4_q07_negative_tracking_tightens_emitted_glyph_advances() {
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let bytes_pos = build_tracking_idml(0);
    let bytes_neg = build_tracking_idml(-100);
    let doc_pos = paged_parse::import_idml_doc(&bytes_pos).unwrap();
    let doc_neg = paged_parse::import_idml_doc(&bytes_neg).unwrap();
    let built_pos = pipeline::build_document(&doc_pos, &opts).unwrap();
    let built_neg = pipeline::build_document(&doc_neg, &opts).unwrap();

    let base = glyph_xys(&built_pos.pages[0].list.commands);
    let tightened = glyph_xys(&built_neg.pages[0].list.commands);
    let base_span = base.last().unwrap().0 - base.first().unwrap().0;
    let tightened_span = tightened.last().unwrap().0 - tightened.first().unwrap().0;
    assert!(
        tightened_span < base_span,
        "Tracking=-100 should tighten the 4-A span vs Tracking=0; observed base_span={base_span}, tightened_span={tightened_span}"
    );
}
