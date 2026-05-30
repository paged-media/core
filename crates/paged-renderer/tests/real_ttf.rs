//! End-to-end test exercising the full text path with real OpenType
//! fonts. Every other test in this crate either uses placeholder
//! font bytes (rustybuzz never shapes) or skips text by registering
//! no resolver. This one drives:
//!
//!     parse → scene → shape (rustybuzz) → compose (Knuth-Plass)
//!         → outline (ttf-parser) → CPU raster → pixel sample
//!
//! Fonts are vendored under `corpus/fonts/` (Google Fonts:
//! Inter / Lora / RobotoSlab, all OFL/Apache).

use std::io::Write;
use std::path::PathBuf;

use paged_compose::{Color, DisplayCommand};
use paged_renderer::{pipeline, BytesResolver, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

fn build_text_idml(applied_font: &str, font_style: Option<&str>, point_size: f32) -> Vec<u8> {
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

    // Page 612 × 400 pt; a single text frame from (40, 40) to (572, 200).
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 200 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    let style_attr = font_style
        .map(|s| format!(" FontStyle=\"{s}\""))
        .unwrap_or_default();
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="{applied_font}"{style_attr} PointSize="{point_size}">
        <Content>Hello, IDML world.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

/// Count pixels darker than `threshold` (per-channel) across the
/// whole image. Used as a paint-anywhere sanity check — the test
/// doesn't depend on any specific glyph landing on any specific
/// coordinate, just that the rasterizer drew _something_ dark on a
/// white background.
fn count_dark_pixels(img: &image::RgbaImage, threshold: u8) -> usize {
    img.pixels()
        .filter(|p| p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold)
        .count()
}

#[test]
fn real_ttf_shapes_outlines_and_rasterises_glyphs() {
    let bytes = build_text_idml("Inter", None, 36.0);
    let document = Document::open(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));

    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let (built, images) = pipeline::render_document(&document, &opts, 144.0, Color::WHITE).unwrap();

    // Story is "Hello, IDML world." → 18 chars; some will cluster
    // (e.g. ',' attaching) but ≥ 14 glyphs is a safe lower bound.
    assert!(
        built.stats.glyphs >= 14,
        "expected real shaping to produce many glyphs, got {}",
        built.stats.glyphs
    );
    assert!(
        built.stats.lines >= 1,
        "expected ≥1 composed line, got {}",
        built.stats.lines
    );

    // Most shaped glyphs emit a FillPath; whitespace glyphs (and
    // a handful of zero-contour marks) produce none. The story has
    // 3 spaces, so allow up to ~5 unfilled glyphs as headroom for
    // future composer changes (ligatures etc.).
    let fill_paths = built.pages[0]
        .list
        .commands
        .iter()
        .filter(|c| matches!(c, DisplayCommand::FillPath { .. }))
        .count();
    assert!(
        fill_paths + 5 >= built.stats.glyphs,
        "fill-path count diverged from glyph count: {fill_paths} fills for {} glyphs",
        built.stats.glyphs
    );

    // Render at 144 dpi → image is page_pt × 2.
    let img = &images[0];
    assert_eq!(img.width(), 1224);
    assert_eq!(img.height(), 800);

    // Real glyphs were rasterised: count near-black pixels. With
    // 36pt text on a 144 dpi canvas at least a few hundred pixels
    // will be inked. The threshold is loose so faint AA edges still
    // count without false positives from the white background.
    let inked = count_dark_pixels(img, 80);
    assert!(
        inked > 500,
        "expected the glyph ink load to be substantial, got {inked} dark pixels"
    );
}

#[test]
fn real_ttf_render_is_byte_deterministic() {
    let bytes = build_text_idml("Lora", None, 24.0);
    let document = Document::open(&bytes).unwrap();

    let mut resolver = BytesResolver::new();
    resolver.add_font("Lora", None, read_font("Lora.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };

    let (_a, images_a) = pipeline::render_document(&document, &opts, 144.0, Color::WHITE).unwrap();
    let (_b, images_b) = pipeline::render_document(&document, &opts, 144.0, Color::WHITE).unwrap();

    assert_eq!(images_a.len(), images_b.len());
    for (i, (a, b)) in images_a.iter().zip(images_b.iter()).enumerate() {
        assert_eq!(
            a.as_raw(),
            b.as_raw(),
            "page {i} re-rendered with different bytes — non-determinism in the text path"
        );
    }
}

#[test]
fn different_fonts_produce_different_pixel_output() {
    // Same text, same point size, same frame — only the font bytes
    // differ. If the renderer were silently falling back to a default
    // (or skipping shaping) the two outputs would coincide. They must
    // not. A real shape → outline → raster path is the only way the
    // pages diverge.
    let bytes = build_text_idml("FixtureFont", None, 36.0);
    let document = Document::open(&bytes).unwrap();

    let mut sans = BytesResolver::new();
    sans.add_font("FixtureFont", None, read_font("Inter.ttf"));
    let mut serif = BytesResolver::new();
    serif.add_font("FixtureFont", None, read_font("RobotoSlab.ttf"));

    let opts_sans = PipelineOptions {
        assets: Some(&sans),
        ..PipelineOptions::default()
    };
    let opts_serif = PipelineOptions {
        assets: Some(&serif),
        ..PipelineOptions::default()
    };

    let (_, sans_imgs) =
        pipeline::render_document(&document, &opts_sans, 144.0, Color::WHITE).unwrap();
    let (_, serif_imgs) =
        pipeline::render_document(&document, &opts_serif, 144.0, Color::WHITE).unwrap();

    assert_ne!(
        sans_imgs[0].as_raw(),
        serif_imgs[0].as_raw(),
        "Inter vs RobotoSlab produced byte-identical pages — text path is not actually using the font"
    );
}
