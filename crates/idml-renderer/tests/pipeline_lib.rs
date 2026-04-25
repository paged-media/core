//! Library-level pipeline tests — exercise `pipeline::build` and
//! `pipeline::render` without spawning the inspect binary.

use std::io::Write;

use idml_compose::{Color, Paint};
use idml_renderer::{pipeline, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn build_minimal_idml() -> Vec<u8> {
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
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Red" Name="Red" Space="CMYK" ColorValue="0 100 100 0"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 300"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="20 20 100 200"
               FillColor="Color/Red" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Content>Body text.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn build_produces_display_list_and_page_dimensions() {
    let bytes = build_minimal_idml();
    let document = Document::open(&bytes).unwrap();

    let opts = PipelineOptions::default();
    let built = pipeline::build(&document, &opts).unwrap();

    assert_eq!(built.width_pt, 300.0);
    assert_eq!(built.height_pt, 400.0);
    // 1 spread, 1 frame → 1 FillPath (no stroke, weight=0).
    assert_eq!(built.list.commands.len(), 1);
    assert_eq!(built.list.paths.len(), 1);
    assert_eq!(built.stats.spreads, 1);
    assert_eq!(built.stats.frames, 1);
    assert_eq!(built.stats.paragraphs, 1);
    assert_eq!(built.stats.runs, 1);
}

#[test]
fn build_document_emits_one_page_with_correct_geometry() {
    let bytes = build_minimal_idml();
    let document = Document::open(&bytes).unwrap();

    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&document, &opts).unwrap();

    assert_eq!(built.pages.len(), 1, "one <Page> in the manifest");
    let page = &built.pages[0];
    assert_eq!(page.width_pt, 300.0);
    assert_eq!(page.height_pt, 400.0);
    assert_eq!(page.list.commands.len(), 1);
    assert_eq!(page.stats.frames, 1);
}

#[test]
fn render_fills_frame_with_resolved_paint() {
    let bytes = build_minimal_idml();
    let document = Document::open(&bytes).unwrap();

    let opts = PipelineOptions::default();
    let (built, img) = pipeline::render(&document, &opts, 72.0, Color::WHITE).unwrap();

    // Page is 300×400 pt at 72 dpi → 300×400 px.
    assert_eq!(img.width(), 300);
    assert_eq!(img.height(), 400);

    // Frame covers x=20..200, y=20..100 — sample inside for red.
    let px = img.get_pixel(50, 50);
    assert!(
        px.0[0] > 200 && px.0[1] < 50 && px.0[2] < 50,
        "expected red inside frame, got {:?}",
        px
    );

    // Outside the frame should be background white.
    let bg = img.get_pixel(5, 5);
    assert!(
        bg.0[0] > 240 && bg.0[1] > 240 && bg.0[2] > 240,
        "expected white bg, got {:?}",
        bg
    );

    // Stats sanity: text isn't shaped (no font), so no glyphs/lines.
    assert_eq!(built.stats.glyphs, 0);
    assert_eq!(built.stats.lines, 0);
}

fn build_gradient_idml() -> Vec<u8> {
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
  <Graphic>
    <Color Self="Color/Sun" Name="Sun" Space="RGB" ColorValue="255 200 80"/>
    <Color Self="Color/Sky" Name="Sky" Space="RGB" ColorValue="60 120 220"/>
    <Gradient Self="Gradient/SkyDown" Name="SkyDown" Type="Linear">
      <GradientStop StopColor="Color/Sun" Location="0"/>
      <GradientStop StopColor="Color/Sky" Location="100"/>
    </Gradient>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="0 0 200 200"
               FillColor="Gradient/SkyDown" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn linear_gradient_fills_top_to_bottom() {
    let bytes = build_gradient_idml();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();

    assert_eq!(images.len(), 1);
    let img = &images[0];

    // The gradient runs (0,0) → (0,1) in unit coords, so the top of
    // the 200 × 200 page should look like Color/Sun (warm yellow) and
    // the bottom should look like Color/Sky (cool blue).
    let top = *img.get_pixel(100, 5);
    let bottom = *img.get_pixel(100, 195);
    assert!(
        top.0[0] > top.0[2] + 50,
        "expected warm top pixel, got {:?}",
        top
    );
    assert!(
        bottom.0[2] > bottom.0[0] + 50,
        "expected cool bottom pixel, got {:?}",
        bottom
    );
    // The display list carries one gradient definition.
    assert_eq!(built.pages[0].list.gradients.len(), 1);
}

#[test]
fn frame_drop_shadow_paints_offset_pixels() {
    let bytes = build_minimal_idml();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions {
        frame_drop_shadow: Some(idml_compose::DropShadow {
            offset_x: 6.0,
            offset_y: 6.0,
            blur_radius: 0.0,
            color: Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 1.0,
        }),
        ..PipelineOptions::default()
    };
    let (_built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
    let img = &images[0];

    // Frame is at x=20..200, y=20..100. Shadow offset (6, 6) places
    // the shadow rect at x=26..206, y=26..106. The strip just below
    // the frame (y=101..106, x=200..206 area) should be dark from
    // the shadow.
    let shadow_strip = *img.get_pixel(202, 105);
    assert!(
        shadow_strip.0[0] < 60 && shadow_strip.0[1] < 60 && shadow_strip.0[2] < 60,
        "expected dark shadow pixel, got {:?}",
        shadow_strip
    );
    // Inside the frame, the red fill draws over the shadow.
    let inside = *img.get_pixel(50, 50);
    assert!(
        inside.0[0] > 200 && inside.0[1] < 50,
        "frame interior should still read red, got {:?}",
        inside
    );
}

#[test]
fn pipeline_options_default_uses_gray_fallback() {
    let opts = PipelineOptions::default();
    match opts.fallback_frame_fill {
        Paint::Solid(c) => {
            assert!(c.r > 0.8 && c.r < 1.0);
            assert_eq!(c.r, c.g);
            assert_eq!(c.g, c.b);
        }
        Paint::LinearGradient(_) => panic!("default should be a solid grey, not a gradient"),
    }
}

fn build_multi_font_idml() -> Vec<u8> {
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
  <idPkg:Story src="Stories/Story_u10.xml"/>
  <idPkg:Story src="Stories/Story_u20.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 300"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="20 20 80 280"/>
    <TextFrame Self="frameB" ParentStory="u20" GeometricBounds="100 20 160 280"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Body Font" PointSize="11">
        <Content>Body text.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.start_file("Stories/Story_u20.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u20">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Heading Font" FontStyle="Bold" PointSize="22">
        <Content>HEADING</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

/// AssetResolver wrapper that records every resolve_font call so the
/// test can verify the pipeline pre-resolves every distinct (family,
/// style) pair exactly once.
struct CountingResolver {
    inner: idml_renderer::BytesResolver,
    calls: std::sync::Mutex<Vec<(String, Option<String>)>>,
}

impl idml_renderer::AssetResolver for CountingResolver {
    fn resolve_font(&self, family: &str, style: Option<&str>) -> Option<bytes::Bytes> {
        self.calls
            .lock()
            .unwrap()
            .push((family.to_string(), style.map(str::to_string)));
        self.inner.resolve_font(family, style)
    }
    fn resolve_image(&self, uri: &str) -> Option<bytes::Bytes> {
        self.inner.resolve_image(uri)
    }
    fn resolve_icc(&self, name: &str) -> Option<bytes::Bytes> {
        self.inner.resolve_icc(name)
    }
}

#[test]
fn asset_resolver_is_consulted_for_every_distinct_font() {
    let bytes = build_multi_font_idml();
    let document = Document::open(&bytes).unwrap();

    let mut br = idml_renderer::BytesResolver::new();
    // Register fake bytes — the test only checks that the resolver
    // is asked, not that shaping succeeds.
    br.add_font("Body Font", None, b"BODY".to_vec());
    br.add_font("Heading Font", Some("Bold"), b"HEAD".to_vec());

    let resolver = CountingResolver {
        inner: br,
        calls: std::sync::Mutex::new(Vec::new()),
    };

    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let _built = pipeline::build_document(&document, &opts).unwrap();

    let mut calls = resolver.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            ("Body Font".to_string(), None),
            ("Heading Font".to_string(), Some("Bold".to_string())),
        ],
        "resolver should be asked once per distinct (family, style)"
    );
}

fn build_translated_idml() -> Vec<u8> {
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
  <Graphic>
    <Color Self="Color/Red" Name="Red" Space="CMYK" ColorValue="0 100 100 0"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    // The frame has local bounds (0, 0, 40, 40) and ItemTransform
    // translates by (100, 50). The rendered frame should land at
    // (100, 50) → (140, 90) in spread coords.
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="rectA" GeometricBounds="0 0 40 40"
               ItemTransform="1 0 0 1 100 50"
               FillColor="Color/Red" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn item_transform_translation_moves_rendered_frame() {
    let bytes = build_translated_idml();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (_built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
    let img = &images[0];

    // Frame should land at (100, 50)..(140, 90). Inside the rect:
    let inside = *img.get_pixel(120, 70);
    assert!(
        inside.0[0] > 200 && inside.0[1] < 50,
        "expected red inside translated frame, got {:?}",
        inside
    );
    // The original (untransformed) location (0, 0)..(40, 40) should
    // be background — proves the translation actually applied.
    let untransformed_origin = *img.get_pixel(20, 20);
    assert!(
        untransformed_origin.0[0] > 240
            && untransformed_origin.0[1] > 240
            && untransformed_origin.0[2] > 240,
        "untransformed origin should be background, got {:?}",
        untransformed_origin
    );
}
