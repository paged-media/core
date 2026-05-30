//! End-to-end test against the self-authored `corpus/seeds/hello`
//! seed IDML.
//!
//! The seed is stored as plain XML (one file per IDML resource) for
//! readability + git-friendly diffs. This test packs them into a
//! valid IDML container at run time, opens it through the library,
//! runs the multi-page pipeline, and asserts the structural
//! invariants that matter for the demoable batch:
//!
//!   * 2 pages, 612 × 792 pt each
//!   * master-spread items present on every page (band + footer rule
//!     emitted before page-level items)
//!   * page-level frames + body text frames placed correctly
//!   * stats line up across pages

use std::io::Write;
use std::path::Path;

use paged_compose::Color;
use paged_renderer::{pipeline, Document, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn pack_seed(seed_dir: &Path) -> Vec<u8> {
    // Walk the source tree and stuff every file into a ZIP. `mimetype`
    // is required to be the first entry, stored uncompressed (per the
    // OASIS / IDML container spec).
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mimetype = std::fs::read(seed_dir.join("mimetype")).expect("mimetype");
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(&mimetype).unwrap();

    fn walk(
        zip: &mut ZipWriter<std::io::Cursor<Vec<u8>>>,
        opts: SimpleFileOptions,
        root: &Path,
        prefix: &str,
    ) {
        for entry in std::fs::read_dir(root).expect("read seed dir") {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "mimetype" {
                continue;
            }
            let path = entry.path();
            let archive_path = if prefix.is_empty() {
                name_str.to_string()
            } else {
                format!("{prefix}/{name_str}")
            };
            if path.is_dir() {
                walk(zip, opts, &path, &archive_path);
            } else {
                let bytes = std::fs::read(&path).expect("read seed file");
                zip.start_file(&archive_path, opts).unwrap();
                zip.write_all(&bytes).unwrap();
            }
        }
    }
    walk(&mut zip, deflated, seed_dir, "");

    zip.finish().unwrap().into_inner()
}

fn seed_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/seeds/hello/source")
}

#[test]
fn seed_hello_parses_and_carries_two_pages() {
    let bytes = pack_seed(&seed_dir());
    let document = Document::open(&bytes).expect("open seed IDML");

    assert_eq!(document.spreads.len(), 2, "two <Spread>s in the manifest");
    assert_eq!(document.stories.len(), 2, "two <Story>s in the manifest");
    assert_eq!(
        document.master_spreads.len(),
        1,
        "one master spread <MasterSpread/uad>"
    );

    // Both pages reference the master.
    let masters: Vec<_> = document
        .spreads
        .iter()
        .flat_map(|s| s.spread.pages.iter())
        .filter_map(|p| p.applied_master.as_deref())
        .collect();
    assert_eq!(masters, vec!["MasterSpread/uad", "MasterSpread/uad"]);

    // Palette has all five swatches.
    assert_eq!(document.palette.colors.len(), 5);
}

#[test]
fn seed_hello_builds_multi_page_display_list() {
    let bytes = pack_seed(&seed_dir());
    let document = Document::open(&bytes).unwrap();

    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&document, &opts).unwrap();

    assert_eq!(built.pages.len(), 2);
    for page in &built.pages {
        assert_eq!(page.width_pt, 612.0);
        assert_eq!(page.height_pt, 792.0);
        // Master items (band + footer rule) + page-level rect = 3
        // FillPath commands per page. The body TextFrame carries
        // `FillColor="Swatch/None"`, which the renderer treats as
        // transparent and emits no fill rect for. No strokes anywhere
        // (StrokeWeight=0 throughout).
        assert_eq!(
            page.list.commands.len(),
            3,
            "expected 2 master items + 1 page rect per page \
             (transparent text frame contributes no fill)",
        );
    }
    assert_eq!(built.stats.spreads, 2);
    // 4 master frames (2 per page × 2 pages) + 4 page frames (2 per page).
    assert_eq!(built.stats.frames, 4 + 4);
}

#[test]
fn seed_hello_renders_and_passes_self_diff() {
    let bytes = pack_seed(&seed_dir());
    let document = Document::open(&bytes).unwrap();

    let opts = PipelineOptions::default();
    let (built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();

    assert_eq!(built.pages.len(), 2);
    assert_eq!(images.len(), 2);
    for img in &images {
        assert_eq!(img.width(), 612);
        assert_eq!(img.height(), 792);
        // Top of the page is the master brand band; expect it not to
        // be the white background. A pixel at (300, 18) sits inside
        // the band's vertical extent (0..36 pt → 0..36 px at 72dpi).
        let band = img.get_pixel(300, 18);
        assert!(
            band.0[0] < 240 || band.0[1] < 240 || band.0[2] < 240,
            "expected coloured master band at top, got {:?}",
            band
        );
        // A pixel at the very bottom-right corner stays white (no
        // master items extend that far).
        let bg = img.get_pixel(610, 790);
        assert!(
            bg.0[0] > 240 && bg.0[1] > 240 && bg.0[2] > 240,
            "expected white bg, got {:?}",
            bg
        );
    }

    // Render once more, diff page-by-page — should be byte-identical.
    let (_again, images2) =
        pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
    for (a, b) in images.iter().zip(images2.iter()) {
        let (report, _) = paged_fidelity::diff::compare_images(
            &image::DynamicImage::ImageRgba8(a.clone()).to_rgb8(),
            &image::DynamicImage::ImageRgba8(b.clone()).to_rgb8(),
        )
        .unwrap();
        assert!(
            report.passes(),
            "deterministic render expected, got {:?}",
            report
        );
        assert!(report.mean_delta_e < 1e-6);
    }
}

/// Snapshot test against pinned golden PNGs. Renders the seed at
/// 144 dpi and ΔE-diffs every page against the corresponding
/// `corpus/seeds/hello/golden/page-NN.png`. This is the project's
/// regression gate: any change to the renderer that visibly
/// shifts the seed_hello output blocks the merge.
///
/// To re-bake the goldens after an intentional renderer change:
///
///     IDML_BAKE_GOLDENS=1 cargo test -p paged-renderer \
///         --test seed_hello -- snapshot --exact
///
/// Inspect the diff carefully before re-baking (the golden is
/// authoritative — that's the whole point).
#[test]
fn seed_hello_matches_golden_snapshot() {
    let bytes = pack_seed(&seed_dir());
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (_built, images) =
        pipeline::render_document(&document, &opts, 144.0, Color::WHITE).unwrap();

    let golden_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/seeds/hello/golden");
    let bake = std::env::var_os("IDML_BAKE_GOLDENS").is_some();
    if bake {
        std::fs::create_dir_all(&golden_dir).expect("create golden dir");
    }

    for (i, img) in images.iter().enumerate() {
        let path = golden_dir.join(format!("page-{:02}.png", i + 1));
        if bake || !path.exists() {
            img.save(&path).expect("write golden png");
            // Skip diff on baked-or-just-created entries; the
            // next test run will diff against this fresh golden.
            continue;
        }
        let expected =
            image::open(&path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()));
        let candidate_rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let expected_rgb = expected.to_rgb8();
        assert_eq!(
            (expected_rgb.width(), expected_rgb.height()),
            (candidate_rgb.width(), candidate_rgb.height()),
            "golden page {} has different dimensions — re-bake?",
            i + 1
        );
        let (report, _) = paged_fidelity::diff::compare_images(&expected_rgb, &candidate_rgb)
            .expect("compare images");
        // Strict regression gate: the seed is deterministic and
        // self-authored, so we expect byte-identity (mean ΔE ≈ 0).
        // Allow a tiny epsilon for f32 / sRGB-roundtrip jitter.
        assert!(
            report.mean_delta_e < 0.01,
            "page {} drifted from golden: meanΔE={:.4}, p99ΔE={:.4}, ssim={:.4}",
            i + 1,
            report.mean_delta_e,
            report.p99_delta_e,
            report.ssim
        );
    }
}
