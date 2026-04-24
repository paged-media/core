//! End-to-end test: build a synthetic IDML with a Spread, two pages,
//! and text frames bound to stories, run the `idml-inspect` binary
//! against it, and verify the whole pipeline (ZIP → designmap →
//! spread → stories → summary) produces the expected counts and
//! frame-to-story bindings.

use std::io::Write;
use std::process::Command;

use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

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
  <idPkg:Story src="Stories/Story_u20.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Page Self="p2" GeometricBounds="0 612 792 1224"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="72 72 720 540"/>
    <TextFrame Self="frameB" ParentStory="u20" GeometricBounds="100 700 300 1100"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Body Font" PointSize="11">
        <Content>Hello, </Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Body Font" FontStyle="Bold" PointSize="11">
        <Content>world</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Body Font" PointSize="11">
        <Content>.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Body Font" PointSize="11">
        <Content>A second paragraph of prose.</Content>
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
      <CharacterStyleRange>
        <Content>Short story.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

fn inspect_binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_idml-inspect"))
}

#[test]
fn inspects_synthetic_idml_with_spread_and_frames() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hello.idml");
    std::fs::write(&path, build_idml()).unwrap();

    let output = Command::new(inspect_binary())
        .arg(&path)
        .output()
        .expect("spawn idml-inspect");
    assert!(
        output.status.success(),
        "idml-inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Manifest counts.
    assert!(stdout.contains("1 spread(s)"), "stdout:\n{stdout}");
    assert!(stdout.contains("2 story ref(s)"), "stdout:\n{stdout}");

    // Spread output.
    assert!(
        stdout.contains("Spreads/Spread_sp1.xml"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("2 page(s)"), "stdout:\n{stdout}");
    assert!(stdout.contains("2 frame(s)"), "stdout:\n{stdout}");
    // Page 1 dimensions: width = 612, height = 792.
    assert!(stdout.contains("612.00 × 792.00"), "stdout:\n{stdout}");
    // Frame A: width = 540 - 72 = 468, height = 720 - 72 = 648.
    assert!(stdout.contains("frameA → story u10"), "stdout:\n{stdout}");
    assert!(stdout.contains("468.00 × 648.00"), "stdout:\n{stdout}");
    // Frame B: width = 1100 - 700 = 400, height = 300 - 100 = 200.
    assert!(stdout.contains("frameB → story u20"), "stdout:\n{stdout}");
    assert!(stdout.contains("400.00 × 200.00"), "stdout:\n{stdout}");

    // Story text.
    assert!(stdout.contains("Hello,"), "stdout:\n{stdout}");
    assert!(stdout.contains("world"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("A second paragraph of prose."),
        "stdout:\n{stdout}"
    );

    // Totals line.
    assert!(stdout.contains("paragraphs=3"), "stdout:\n{stdout}");
    assert!(stdout.contains("runs=5"), "stdout:\n{stdout}");
}
