//! End-to-end test: build a synthetic IDML containing a multi-paragraph
//! story, run the `idml-inspect` binary against it, and verify the whole
//! pipeline (ZIP → designmap → Story → summary) produces the expected
//! counts.

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
  <idPkg:Story src="Stories/Story_u10.xml"/>
  <idPkg:Story src="Stories/Story_u20.xml"/>
</Document>"#,
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
fn inspects_synthetic_idml_with_two_stories() {
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

    // Key assertions: both stories are discovered, the body paragraph's
    // three runs are extracted, and totals line up.
    assert!(stdout.contains("2 story ref(s)"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("Stories/Story_u10.xml"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("Stories/Story_u20.xml"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("Hello,"), "stdout:\n{stdout}");
    assert!(stdout.contains("world"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("A second paragraph of prose."),
        "stdout:\n{stdout}"
    );
    // Totals line: 3 paragraphs (2 + 1), 5 runs (3 + 1 + 1).
    assert!(stdout.contains("paragraphs=3"), "stdout:\n{stdout}");
    assert!(stdout.contains("runs=5"), "stdout:\n{stdout}");
}
