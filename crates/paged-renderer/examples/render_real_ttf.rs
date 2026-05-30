//! Tiny demo: builds a synthetic IDML in memory, registers Inter
//! (the same fixture the real-TTF tests use), renders the page,
//! and writes the PNG to disk so you can eyeball the output.
//!
//!     cargo run -p paged-renderer --example render_real_ttf -- /tmp/inter.png
//!
//! Default output path is `target/render_real_ttf.png` if you don't
//! pass one.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::Color;
use paged_renderer::{pipeline, BytesResolver, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/render_real_ttf.png"));

    let font = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )?;
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, font);

    let document = Document::open(&build_idml())?;
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let (built, mut images) = pipeline::render_document(&document, &opts, 144.0, Color::WHITE)?;
    let img = images.remove(0);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    img.save(&out)?;
    println!(
        "wrote {} ({}×{} px) — {} glyphs, {} lines",
        out.display(),
        img.width(),
        img.height(),
        built.stats.glyphs,
        built.stats.lines
    );
    Ok(())
}

fn build_idml() -> Vec<u8> {
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
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 360 572" StrokeWeight="0"/>
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
      <CharacterStyleRange AppliedFont="Inter" PointSize="48">
        <Content>Hello, IDML.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="14">
        <Content>Real TTF shaping, real glyph outlines, real CPU rasterizer.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}
