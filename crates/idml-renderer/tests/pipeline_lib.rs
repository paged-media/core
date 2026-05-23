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
    build_gradient_idml_with_angle(None)
}

/// Like `build_gradient_idml` but writes the supplied
/// `GradientFillAngle` (in degrees) onto the rectangle. `None`
/// omits the attribute, exercising IDML's spec default of 0°.
fn build_gradient_idml_with_angle(angle_deg: Option<f32>) -> Vec<u8> {
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

    let angle_attr = match angle_deg {
        Some(a) => format!(" GradientFillAngle=\"{a}\""),
        None => String::new(),
    };
    let spread_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="0 0 200 200"
               FillColor="Gradient/SkyDown" StrokeWeight="0"{angle_attr}/>
  </Spread>
</idPkg:Spread>"#,
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread_xml.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn linear_gradient_fills_left_to_right_by_default() {
    // IDML's GradientFillAngle defaults to 0° = horizontal-right
    // (left = first stop, right = last). This test pins the default
    // direction; gradient_fill_angle != 0 rotates the line through
    // the rect centre per `color_id_to_paint_with_list_dir`.
    let bytes = build_gradient_idml();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();

    assert_eq!(images.len(), 1);
    let img = &images[0];

    // The gradient defaults to (0,0.5) → (1,0.5) in unit coords, so
    // the left of the 200 × 200 page should look like Color/Sun (warm
    // yellow) and the right should look like Color/Sky (cool blue).
    let left = *img.get_pixel(5, 100);
    let right = *img.get_pixel(195, 100);
    assert!(
        left.0[0] > left.0[2] + 50,
        "expected warm left pixel, got {:?}",
        left
    );
    assert!(
        right.0[2] > right.0[0] + 50,
        "expected cool right pixel, got {:?}",
        right
    );
    // The display list carries one gradient definition.
    assert_eq!(built.pages[0].list.gradients.len(), 1);
}

#[test]
fn linear_gradient_rotated_90_degrees_runs_vertically() {
    // Regression for Tier 1 #2: a rotated `GradientFillAngle`
    // (= IDML's `<Gradient>` Angle) must rotate the gradient line
    // through the rect centre. Before the fix the renderer hardcoded
    // unit endpoints `(0, 0) → (0, 1)` which painted every gradient
    // top-to-bottom regardless of the angle.
    //
    // At 90° (vertical-down per IDML convention) the warm `Color/Sun`
    // (first stop) lives at the TOP and the cool `Color/Sky` (last
    // stop) at the BOTTOM. The horizontal default test asserts the
    // 0° = left→right axis; this one pins the 90° = top→bottom axis,
    // so the two together reject a hardcoded direction in either
    // orientation.
    let bytes = build_gradient_idml_with_angle(Some(90.0));
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (_, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
    let img = &images[0];

    let top = *img.get_pixel(100, 5);
    let bottom = *img.get_pixel(100, 195);
    assert!(
        top.0[0] > top.0[2] + 50,
        "expected warm top pixel at 90°, got {:?}",
        top
    );
    assert!(
        bottom.0[2] > bottom.0[0] + 50,
        "expected cool bottom pixel at 90°, got {:?}",
        bottom
    );

    // Sibling cross-check: the horizontal strip at the rect centre
    // should be ~uniform when the gradient runs vertically — proving
    // the axis really rotated, not just shifted.
    let mid_left = *img.get_pixel(5, 100);
    let mid_right = *img.get_pixel(195, 100);
    let dr = (mid_left.0[0] as i32 - mid_right.0[0] as i32).abs();
    let db = (mid_left.0[2] as i32 - mid_right.0[2] as i32).abs();
    assert!(
        dr < 15 && db < 15,
        "expected near-uniform horizontal strip at 90°, got left={mid_left:?} right={mid_right:?}",
    );
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
        Paint::LinearGradient(_) | Paint::RadialGradient(_) => {
            panic!("default should be a solid grey, not a gradient")
        }
        Paint::Cmyk { .. } => {
            panic!("default should be a solid grey, not a CMYK paint")
        }
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

fn build_image_idml() -> Vec<u8> {
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
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="rectImage" GeometricBounds="40 40 160 160" StrokeWeight="0">
      <Image Self="imageA" LinkResourceURI="logo.png"/>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

/// Build a 1×1 fully-green PNG so the test asserts on a known
/// pixel after rendering. PNG is the simplest format the `image`
/// crate decodes by default.
fn green_pixel_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 220, 0, 255]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[test]
fn rectangle_image_link_decodes_and_blits() {
    let bytes = build_image_idml();
    let document = Document::open(&bytes).unwrap();

    let mut br = idml_renderer::BytesResolver::new();
    br.add_image("logo.png", green_pixel_png());

    let opts = PipelineOptions {
        assets: Some(&br),
        ..PipelineOptions::default()
    };
    let (built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();

    // The rectangle covers x=40..160, y=40..160. The image is a
    // 1×1 green pixel scaled across the whole rect; sampling deep
    // inside the rect should read green.
    let img = &images[0];
    let inside = *img.get_pixel(100, 100);
    assert!(
        inside.0[0] < 60 && inside.0[1] > 180 && inside.0[2] < 60,
        "expected green inside placed image, got {:?}",
        inside
    );
    // The DisplayList must own one DecodedImage, plus the
    // Image command alongside the rectangle's FillPath.
    let page = &built.pages[0];
    assert_eq!(page.list.images.len(), 1);
}

fn build_threaded_idml() -> Vec<u8> {
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
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 400"/>
    <TextFrame Self="frameA" ParentStory="u10"
               GeometricBounds="20 20 100 180"
               NextTextFrame="frameB"/>
    <TextFrame Self="frameB" ParentStory="u10"
               GeometricBounds="20 200 100 380"
               NextTextFrame="frameC"/>
    <TextFrame Self="frameC" ParentStory="u10"
               GeometricBounds="120 20 200 180"/>
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
        <Content>Hello</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn frame_chain_walks_next_text_frame_links() {
    let bytes = build_threaded_idml();
    let document = Document::open(&bytes).unwrap();
    let chain = document.frame_chain("u10");
    assert_eq!(chain.len(), 3, "frameA → frameB → frameC");
    assert_eq!(chain[0].self_id.as_deref(), Some("frameA"));
    assert_eq!(chain[1].self_id.as_deref(), Some("frameB"));
    assert_eq!(chain[2].self_id.as_deref(), Some("frameC"));
}

#[test]
fn threaded_story_renders_without_panic() {
    let bytes = build_threaded_idml();
    let document = Document::open(&bytes).unwrap();
    // No font registered → text is skipped per the resolver fallback,
    // but the chain construction + per-frame emission plumbing must
    // still run cleanly. Smoke test for the threading refactor.
    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&document, &opts).unwrap();
    // Exactly one page in this synthetic IDML.
    assert_eq!(built.pages.len(), 1);
    // The three TextFrames in the chain carry no FillColor, so the
    // renderer skips frame-background fills (transparent frames are
    // pure layout containers in InDesign). With no font registered
    // there are also no glyph fills — the pipeline ran cleanly with
    // an empty display list, which is what this smoke test cares
    // about.
    let frame_fills = built.pages[0]
        .list
        .commands
        .iter()
        .filter(|c| matches!(c, idml_compose::DisplayCommand::FillPath { .. }))
        .count();
    assert_eq!(
        frame_fills, 0,
        "transparent text frames + no font ⇒ zero fills, got {frame_fills}"
    );
}

/// Build a 1-page IDML with two Rectangles on different layers.
/// Cycle-8 correction: `layerBack` (the bottom-of-z-stack layer) is
/// declared *first* in designmap.xml. Real-world IDMLs we've inspected
/// (e.g. company-profile-template, where Bg/Image/Text layers list
/// Bg first and Bg renders at the bottom of the canvas) use
/// designmap ordering where layers[0] = bottom of z-stack. The XML
/// order of the Rectangles is reversed from the desired paint order:
/// the front-layer rect (Blue) comes first in the spread, the
/// back-layer rect (Red) comes second. A correct renderer emits Red
/// FIRST (behind) and Blue SECOND (on top), regardless of XML order.
fn build_layered_rects_idml() -> Vec<u8> {
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
  <Layer Self="layerBack" Name="Back" Visible="true" Printable="true"/>
  <Layer Self="layerFront" Name="Front" Visible="true" Printable="true"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Red" Name="Red" Space="RGB" ColorValue="255 0 0"/>
    <Color Self="Color/Blue" Name="Blue" Space="RGB" ColorValue="0 0 255"/>
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
    <Rectangle Self="rFront" ItemLayer="layerFront" GeometricBounds="0 0 200 200"
               FillColor="Color/Blue" StrokeWeight="0"/>
    <Rectangle Self="rBack" ItemLayer="layerBack" GeometricBounds="0 0 200 200"
               FillColor="Color/Red" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

/// Same shape as `build_layered_rects_idml` but BOTH rectangles share
/// the SAME ItemLayer. The Q-10 sort must be a no-op when all items
/// resolve to a single layer-z; emission preserves XML order
/// (Blue first, Red second).
fn build_same_layer_rects_idml() -> Vec<u8> {
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
  <Layer Self="layerOnly" Name="Only" Visible="true" Printable="true"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Red" Name="Red" Space="RGB" ColorValue="255 0 0"/>
    <Color Self="Color/Blue" Name="Blue" Space="RGB" ColorValue="0 0 255"/>
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
    <Rectangle Self="rBlue" ItemLayer="layerOnly" GeometricBounds="0 0 200 200"
               FillColor="Color/Blue" StrokeWeight="0"/>
    <Rectangle Self="rRed" ItemLayer="layerOnly" GeometricBounds="0 0 200 200"
               FillColor="Color/Red" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn cross_shape_same_layer_preserves_xml_order() {
    // Q-10 safeguard: with both rects on the same layer the sort
    // must be a no-op. XML order (Blue first, Red second) wins.
    let bytes = build_same_layer_rects_idml();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&document, &opts).unwrap();
    let fills: Vec<&Paint> = built.pages[0]
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            idml_compose::DisplayCommand::FillPath { paint, .. } => Some(paint),
            _ => None,
        })
        .collect();
    assert_eq!(fills.len(), 2);
    fn is_blue(p: &Paint) -> bool {
        matches!(p, Paint::Solid(c) if c.b > c.r && c.b > c.g)
    }
    fn is_red(p: &Paint) -> bool {
        matches!(p, Paint::Solid(c) if c.r > c.b && c.r > c.g)
    }
    assert!(is_blue(fills[0]), "XML-first Blue rect emits first");
    assert!(is_red(fills[1]), "XML-second Red rect emits second");
}

#[test]
fn cross_shape_item_layer_z_order_back_emits_before_front() {
    // Q-10: items on a back layer paint first (behind) regardless of
    // XML order. The IDML places Blue (front layer) before Red (back
    // layer) — the renderer must invert that so Red's FillPath comes
    // first in the page command list and Blue's comes second.
    let bytes = build_layered_rects_idml();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&document, &opts).unwrap();
    assert_eq!(built.pages.len(), 1);

    let fills: Vec<&Paint> = built.pages[0]
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            idml_compose::DisplayCommand::FillPath { paint, .. } => Some(paint),
            _ => None,
        })
        .collect();
    assert_eq!(fills.len(), 2, "two rectangles → two FillPaths");

    // Red ≈ (1,0,0); Blue ≈ (0,0,1). Use the dominant channel to
    // identify each paint without depending on exact sRGB encoding.
    fn is_red(p: &Paint) -> bool {
        matches!(p, Paint::Solid(c) if c.r > c.b && c.r > c.g)
    }
    fn is_blue(p: &Paint) -> bool {
        matches!(p, Paint::Solid(c) if c.b > c.r && c.b > c.g)
    }
    assert!(
        is_red(fills[0]),
        "back-layer (Red) rect should emit first, got {:?}",
        fills[0]
    );
    assert!(
        is_blue(fills[1]),
        "front-layer (Blue) rect should emit second, got {:?}",
        fills[1]
    );
}
