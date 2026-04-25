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

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Red" Name="Red" Space="CMYK" ColorValue="0 100 100 0"/>
    <Color Self="Color/Paper" Name="Paper" Space="RGB" ColorValue="255 255 255"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Page Self="p2" GeometricBounds="0 612 792 1224"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="72 72 720 540"
               FillColor="Color/Red" StrokeColor="Color/Paper" StrokeWeight="2"/>
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

    // Palette surfaced and the red-filled frame shows up with its name.
    assert!(stdout.contains("palette"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("fill=Red"),
        "expected frame A to display Red fill\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("fill=(none)"),
        "expected frame B to display no fill\nstdout:\n{stdout}"
    );

    // Totals line.
    assert!(stdout.contains("paragraphs=3"), "stdout:\n{stdout}");
    assert!(stdout.contains("runs=5"), "stdout:\n{stdout}");
}

#[test]
fn render_flag_produces_png_that_passes_fidelity_self_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let idml = tmp.path().join("hello.idml");
    std::fs::write(&idml, build_idml()).unwrap();
    // Multi-page output: --render writes <stem>-001.png, <stem>-002.png.
    // We compare the first page's render across two runs.
    let base_a = tmp.path().join("a.png");
    let base_b = tmp.path().join("b.png");
    let page_a = tmp.path().join("a-001.png");
    let page_b = tmp.path().join("b-001.png");

    for (base, page) in [(&base_a, &page_a), (&base_b, &page_b)] {
        let status = Command::new(inspect_binary())
            .arg(&idml)
            .arg("--render")
            .arg(base)
            .arg("--dpi")
            .arg("72")
            .status()
            .expect("spawn idml-inspect");
        assert!(status.success(), "render failed");
        assert!(page.exists(), "PNG not produced at {:?}", page);
    }

    let (report, _deltas) = idml_fidelity::diff::compare_pngs(&page_a, &page_b).unwrap();
    assert!(
        report.passes(),
        "self-diff failed: mean ΔE={} p99 ΔE={} SSIM={}",
        report.mean_delta_e,
        report.p99_delta_e,
        report.ssim
    );
    assert!(report.mean_delta_e < 1e-6, "mean ΔE should be zero");
    assert!((report.ssim - 1.0).abs() < 1e-6, "SSIM should be 1");
}

#[test]
fn json_flag_emits_machine_readable_report() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hello.idml");
    std::fs::write(&path, build_idml()).unwrap();

    let output = Command::new(inspect_binary())
        .arg(&path)
        .arg("--json")
        .arg("--display-list")
        .output()
        .expect("spawn idml-inspect");
    assert!(
        output.status.success(),
        "idml-inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not valid JSON: {e}\n{stdout}"));
    assert_eq!(
        json["mimetype"],
        "application/vnd.adobe.indesign-idml-package"
    );
    assert_eq!(json["manifest"]["spreads"], 1);
    assert_eq!(json["manifest"]["stories"], 2);
    assert_eq!(json["totals"]["pages"], 2);
    assert_eq!(json["totals"]["paragraphs"], 3);
    assert_eq!(json["totals"]["runs"], 5);
    assert!(json["pages"].as_array().unwrap().len() == 2);
    assert!(json["spreads"].as_array().unwrap().len() == 1);
    assert!(json["stories"].as_array().unwrap().len() == 2);
}

#[test]
fn display_list_flag_emits_one_command_per_frame_without_font() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hello.idml");
    std::fs::write(&path, build_idml()).unwrap();

    let output = Command::new(inspect_binary())
        .arg(&path)
        .arg("--display-list")
        .output()
        .expect("spawn idml-inspect");
    assert!(
        output.status.success(),
        "idml-inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Two frames split across the spread's two pages → 3 commands
    // total (frame A fill + stroke on page 1; frame B fill on page 2).
    // Each page interns its own unit-rect path → 2 paths total.
    assert!(stdout.contains("3 command(s) total"), "stdout:\n{stdout}");
    assert!(stdout.contains("2 path(s) total"), "stdout:\n{stdout}");
}
