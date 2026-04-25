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
