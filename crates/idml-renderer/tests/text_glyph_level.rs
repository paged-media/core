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

use idml_compose::{DisplayCommand, PathSegment, Transform};
use idml_renderer::{pipeline, BytesResolver, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name))
        .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
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
) -> impl Iterator<Item = (&'a idml_compose::PathId, &'a Transform)> + 'a {
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
) -> impl Iterator<Item = (&'a idml_compose::PathId, &'a idml_compose::Stroke, &'a Transform)> + 'a
{
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
/// (see `idml_compose::text::emit_glyph_slice`). Pull `(x, y, scale)`
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
            if b.abs() < 1e-5
                && c2.abs() < 1e-5
                && a > 0.0
                && d < 0.0
                && (a + d).abs() < 1e-4
            {
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
    let doc = Document::open(&bytes).unwrap();

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
    let doc = Document::open(&bytes).unwrap();

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
        assert!(w[1] > w[0], "frame A baselines not monotonic: {a_baselines:?}");
    }
    for w in b_baselines.windows(2) {
        assert!(w[1] > w[0], "frame B baselines not monotonic: {b_baselines:?}");
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
fn horizontal_stroke_lines(page: &idml_renderer::BuiltPage) -> Vec<(f32, f32, f32)> {
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
        &Document::open(&build_decoration_idml("", "Underline")).unwrap(),
        &opts,
    )
    .unwrap();
    let under = pipeline::build_document(
        &Document::open(&build_decoration_idml(r#" Underline="true""#, "Underline")).unwrap(),
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
        &Document::open(&build_decoration_idml(r#" StrikeThru="true""#, "Strike")).unwrap(),
        &opts,
    )
    .unwrap();
    let both = pipeline::build_document(
        &Document::open(
            &build_decoration_idml(r#" StrikeThru="true" Underline="true""#, "Both"),
        )
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
    let above = both_lines.iter().filter(|(y, _, _)| *y < both_baseline).count();
    let below = both_lines.iter().filter(|(y, _, _)| *y > both_baseline).count();
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
        let doc = Document::open(&bytes).unwrap();
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
    let bullet_doc = Document::open(&build_bullet_idml(0x2022)).unwrap();
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
    let plain_doc = Document::open(&plain_bytes).unwrap();
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

    let doc = Document::open(&build_bullet_with_character_style_idml()).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();

    // Collect every glyph FillPath in emit order. The filter mirrors
    // `glyph_xys` — uniform-scale-with-y-flip matrices are glyphs;
    // frame fills carry shearing/scaling, so they get rejected.
    let glyph_paints: Vec<(f32, idml_compose::Paint)> = built.pages[0]
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath {
                paint, transform, ..
            } => {
                let [a, b, c2, d, tx, _] = transform.0;
                if b.abs() < 1e-5
                    && c2.abs() < 1e-5
                    && a > 0.0
                    && d < 0.0
                    && (a + d).abs() < 1e-4
                {
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
        idml_compose::Paint::Solid(c) => {
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

    let doc = Document::open(&build_numbered_list_idml()).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let xys = glyph_xys(&built.pages[0].list.commands);

    // Group glyphs by baseline (rounded to int pt). Three numbered
    // paragraphs → three distinct baselines.
    let mut by_baseline: std::collections::BTreeMap<i32, Vec<(f32, u32)>> =
        std::collections::BTreeMap::new();
    for ((_, _, _), (path_id, transform)) in xys
        .iter()
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
        row.iter()
            .map(|(x, _)| *x)
            .fold(f32::INFINITY, f32::min)
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
    let doc = Document::open(&build_numbering_idml(paragraphs)).unwrap();
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
    let doc = Document::open(&build_numbering_idml(paragraphs)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    assert!(rows.len() >= 2, "expected 2 paragraphs, got {} rows", rows.len());
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
    let doc = Document::open(&build_numbering_idml(paragraphs)).unwrap();
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
    let doc = Document::open(&build_numbering_idml(reset_paragraphs)).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    let rows = glyphs_by_baseline(&built.pages[0].list.commands);
    let reset_leading = rows[2][0].1;
    assert_eq!(
        leading_1, reset_leading,
        "without NumberingContinue, paragraph 3 must lead with the same '1' glyph as paragraph 1",
    );
}
