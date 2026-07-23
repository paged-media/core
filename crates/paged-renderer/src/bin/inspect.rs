/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! `paged-inspect`: open an IDML, run the renderer pipeline, and print
//! a human-readable summary of what happened.
//!
//! Pure CLI wrapper over `paged_renderer::pipeline` — everything
//! structural lives in the library so other hosts (WASM, tests) can
//! drive the same flow without re-parsing the argv.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use paged_model::{to_linear_rgb, Graphic};
use paged_renderer::{pipeline, Document, PipelineOptions};

#[derive(Parser, Debug)]
#[command(name = "paged-inspect", version, about)]
struct Args {
    /// IDML file to inspect.
    file: PathBuf,
    /// Optional TTF/OTF font to use for shaping every run.
    #[arg(long)]
    font: Option<PathBuf>,
    /// Optional fallback TTF/OTF font registered as the resolver's
    /// `default_font` — used for any IDML-referenced family that
    /// isn't explicitly registered. Lets you render documents whose
    /// fonts you can't ship (Adobe-licensed Minion, Caslon, etc.) by
    /// substituting a permissive face.
    #[arg(long)]
    default_font: Option<PathBuf>,
    /// Register a TTF/OTF under a specific IDML family name. Pass as
    /// `--font-family "Open Sans=corpus/fonts/OpenSans.ttf"`. Repeatable.
    /// Takes precedence over `--default-font` for the named family.
    /// Optional `/STYLE` suffix targets a specific FontStyle:
    /// `--font-family "Open Sans/Italic=corpus/fonts/OpenSans-Italic.ttf"`.
    #[arg(long, value_name = "NAME[/STYLE]=PATH")]
    font_family: Vec<String>,
    /// Override the metrics the renderer uses for first-baseline math
    /// for a specific IDML family. Useful when you've substituted a
    /// font (e.g. Arial → Roboto) and the substitute's ascender shifts
    /// every first baseline against the reference. Format:
    /// `"Arial=ASCENDER[,CAP_HEIGHT[,X_HEIGHT]]"` — em-fractions.
    /// Glyph rendering still uses the substitute font's outlines;
    /// only baseline placement reads these values. Repeatable.
    #[arg(long, value_name = "FAMILY=ASCENDER[,CAP_HEIGHT[,X_HEIGHT]]")]
    font_metrics: Vec<String>,
    /// Directory to search for linked images. The IDML stores URIs
    /// like `file:///.../Links/Photo.jpg`; the resolver looks up the
    /// basename in each registered dir. Repeatable.
    #[arg(long, value_name = "DIR")]
    links_dir: Vec<PathBuf>,
    /// CMYK ICC profile to use for color conversion. Overrides the
    /// document's declared `CMYKProfile`. When omitted, the renderer
    /// auto-probes the Adobe ColorSync directory for a profile matching
    /// the document's declared name (e.g. "Coated FOGRA39 (ISO
    /// 12647-2:2004)" → CoatedFOGRA39.icc); falls back to a naive
    /// CMYK→sRGB approximation when nothing matches.
    #[arg(long, value_name = "PATH")]
    cmyk_profile: Option<PathBuf>,
    /// Default point size used when a run has no explicit PointSize.
    #[arg(long, default_value_t = 12.0)]
    default_size: f32,
    /// Explicit column width in pt. Overrides any frame geometry.
    #[arg(long)]
    column_width_pt: Option<f32>,
    /// W3.B2 — IDML save-back round-trip check. Parse the input,
    /// re-serialise it through `idml_export::write_idml`, re-parse the
    /// result, and compare: (a) per-entry byte-identity, (b) parsed
    /// model stats (spreads / stories / frames / run text), (c) the
    /// rendered page hashes (CPU backend). Prints a compact JSON report
    /// and exits 0 iff the re-parse succeeds, the stats match, and
    /// every page hashes identically. The conformance harness's
    /// round-trips level (W3.B3) calls this. All other rendering /
    /// reporting flags are ignored in this mode.
    #[arg(long)]
    roundtrip: bool,
    /// W4.14 — mutation save-back round-trip check. Open the input, apply
    /// ONE typed `paged_mutate` Operation against a target picked from the
    /// document (first TextFrame else first Rectangle / first non-empty
    /// story / a non-`[None]` palette swatch), re-serialise through
    /// `idml_export::write_idml`, re-open, and verify (a) the mutated
    /// value SURVIVED the round-trip and (b) the rest of the structure
    /// (spread / story / frame counts, run text) matches the mutated
    /// model. Prints a single JSON line `{"applied","survived",
    /// "untouched_ok","ok","note"}` and exits 0 iff `ok` OR the document
    /// has no target for the mutation (n/a); exits 1 on a genuine apply /
    /// write / round-trip failure. (`insertPage` fully round-trips since
    /// C-8 emitted new spread entries — the old KNOWN_LOSS W3.B2 lane is
    /// retired.) The conformance harness's mutate-round-trips lane calls
    /// this. Variants:
    /// `setFrameStrokeWeight|setFrameFill|setFrameTransform|setCharFontSize|insertPage`.
    /// All other rendering / reporting flags are ignored in this mode.
    #[arg(long, value_name = "MUTATION")]
    mutate_roundtrip: Option<String>,
    /// Build the DisplayList and report command / path counts.
    #[arg(long)]
    display_list: bool,
    /// Rasterise the DisplayList via the CPU backend and write a PNG.
    /// Implies --display-list.
    #[arg(long)]
    render: Option<PathBuf>,
    /// DPI for --render output (72 = 1 px per pt; 300 = print).
    #[arg(long, default_value_t = 144.0)]
    dpi: f32,
    /// Emit the report as machine-readable JSON instead of the
    /// human-readable plain text. Suppresses the per-spread/story
    /// log lines.
    #[arg(long)]
    json: bool,
    /// Suppress InDesign's missing-image placeholder (30% grey +
    /// diagonal X) on image-bearing frames whose `LinkResourceURI`
    /// doesn't resolve. The default is to stamp the placeholder so
    /// renders match real-world InDesign PDFs that bake the
    /// placeholder visual into broken-link templates. Synthetic
    /// fixtures whose references were exported without the
    /// placeholder visible can opt out via this flag.
    #[arg(long)]
    no_missing_image_placeholder: bool,
    /// Install a tracing subscriber that prints debug-level events
    /// from the `paged_renderer::icc` target to stderr. Used to confirm
    /// the JPEG-embedded-ICC branch fires on a corpus pack — the
    /// CMYK-decode path emits one event per decoded image (either
    /// "decoded via embedded ICC" or "no ICC; naive multiplicative").
    #[arg(long)]
    trace_icc: bool,
    /// Cycle-8 Track 1: install a tracing subscriber for the
    /// `paged_renderer::routing` target. Emits one debug event per
    /// Rectangle / Oval / GraphicLine / Polygon whose page-routing
    /// is decided, with the rect's inner bounds, item transform,
    /// computed spread-coord bounds, each page's spread-coord
    /// bounds, and the chosen page indices. Diagnoses image-rect
    /// off-page routing bugs (cycle-7 Track 3 finding).
    #[arg(long)]
    trace_routing: bool,
    /// Track 2 A/B harness candidate side: write one JSON record per
    /// laid-out line to this file (JSONL). Each record carries
    /// `story_id`, `paragraph_idx`, `line_idx`, `page_idx`,
    /// `frame_idx`, `first_byte`, `last_byte`, `baseline_y_pt`,
    /// `width_pt`. The reference side (`corpus/envato/breaks-extract.py`)
    /// reconstructs the same shape from PDF word geometry.
    #[arg(long, value_name = "PATH")]
    emit_breaks: Option<PathBuf>,
    /// Cycle-6 Track 1: restrict `--emit-breaks` collection to a
    /// single story by its `Self` id (e.g. `u10`). Without this flag
    /// every story's lines are emitted (cycle-5 behaviour). Combines
    /// with `--break-page-range`.
    #[arg(long, value_name = "STORY_ID")]
    break_story_id: Option<String>,
    /// Cycle-6 Track 1: restrict `--emit-breaks` collection to a
    /// half-open page-index range, written as `START:END` (e.g.
    /// `0:4` covers pages 0..3). Combines with `--break-story-id`.
    #[arg(long, value_name = "START:END")]
    break_page_range: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.trace_icc || args.trace_routing {
        let mut targets: Vec<&str> = Vec::new();
        if args.trace_icc {
            targets.push("paged_renderer::icc=debug");
        }
        if args.trace_routing {
            targets.push("paged_renderer::routing=debug");
        }
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(targets.join(",")));
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(true)
            .without_time()
            .init();
    }
    let bytes =
        std::fs::read(&args.file).with_context(|| format!("read {}", args.file.display()))?;

    // W3.B2 — round-trip mode is a self-contained check that
    // short-circuits the normal inspect flow. Exits non-zero on any
    // divergence (re-parse failure / stats mismatch / pixel mismatch).
    if args.roundtrip {
        let report = run_roundtrip(&bytes, args.dpi)?;
        println!("{}", serde_json::to_string(&report.json)?);
        std::process::exit(if report.ok { 0 } else { 1 });
    }

    // W4.14 — mutation round-trip mode is a second self-contained check
    // that short-circuits the normal inspect flow. Exits non-zero only on
    // a genuine apply/write failure; an n/a (no target) or a documented
    // known-loss still exits 0 (it's a conformance-pass, not a crash).
    if let Some(mutation) = args.mutate_roundtrip.as_deref() {
        let report = run_mutate_roundtrip(&bytes, mutation)?;
        println!("{}", serde_json::to_string(&report.json)?);
        std::process::exit(if report.exit_ok { 0 } else { 1 });
    }

    // The importer now lives in `paged-parse` (the IDML adapter) and returns the
    // raw source archive alongside the model — the model no longer carries it (N9).
    let (document, source_archive) = idml_import::import_idml(&bytes).context("open IDML")?;
    let palette = &document.palette;

    if !args.json {
        println!("file          {}", args.file.display());
        println!("mimetype      {}", source_archive.mimetype);
        if let Some(v) = document.designmap.dom_version.as_deref() {
            println!("DOMVersion    {v}");
        }
        println!(
            "manifest      {} spread(s), {} story ref(s), {} master(s)",
            document.designmap.spreads.len(),
            document.designmap.stories.len(),
            document.designmap.master_spreads.len(),
        );
        if !palette.colors.is_empty() || !palette.swatches.is_empty() {
            println!(
                "palette       {} colour(s), {} swatch(es)",
                palette.colors.len(),
                palette.swatches.len(),
            );
        }

        for parsed in &document.spreads {
            let spread = &parsed.spread;
            println!(
                "\nspread        {}  ({} page(s), {} frame(s){})",
                parsed.src,
                spread.pages.len(),
                spread.text_frames.len() + spread.rectangles.len(),
                if spread.skipped_nested_frames > 0 {
                    format!(", {} nested frame(s) skipped", spread.skipped_nested_frames)
                } else {
                    String::new()
                },
            );
            for (i, p) in spread.pages.iter().enumerate() {
                println!(
                    "  page {i:02}    {:>6.2} × {:<6.2} pt",
                    p.bounds.width(),
                    p.bounds.height(),
                );
            }
            for frame in &spread.text_frames {
                println!(
                    "  frame       {} → story {}   {:>6.2} × {:<6.2} pt  fill={}",
                    frame.self_id.as_deref().unwrap_or("?"),
                    frame.parent_story.as_deref().unwrap_or("(none)"),
                    frame.bounds.width(),
                    frame.bounds.height(),
                    describe_fill(frame.fill_color.as_deref(), palette),
                );
            }
            for rect in &spread.rectangles {
                println!(
                    "  rect        {}   {:>6.2} × {:<6.2} pt  fill={}",
                    rect.self_id.as_deref().unwrap_or("?"),
                    rect.bounds.width(),
                    rect.bounds.height(),
                    describe_fill(rect.fill_color.as_deref(), palette),
                );
            }
        }

        for parsed in &document.stories {
            println!(
                "\nstory         {}  ({} paragraph(s))",
                parsed.src,
                parsed.story.paragraphs.len(),
            );
            for (pi, p) in parsed.story.paragraphs.iter().enumerate() {
                for (ri, r) in p.runs.iter().enumerate() {
                    let size = r.point_size.unwrap_or(args.default_size);
                    println!(
                        "  p{pi:02} r{ri:02}   {:>6.2}pt  {}",
                        size,
                        first_line(&r.text)
                    );
                }
            }
        }
    }

    // Everything below is driven by the library.
    let want_display_list = args.display_list || args.render.is_some();
    let font_bytes = args
        .font
        .as_deref()
        .map(|p| std::fs::read(p).with_context(|| format!("read font {}", p.display())))
        .transpose()?;
    let default_font_bytes = args
        .default_font
        .as_deref()
        .map(|p| std::fs::read(p).with_context(|| format!("read default font {}", p.display())))
        .transpose()?;
    // --font-family accepts two shapes:
    //   "Family=PATH"        — registered for any style of that family
    //   "Family/Style=PATH"   — registered only for that exact style
    //                           (matches IDML's "FontStyle" attribute)
    // Style example: "Italic", "Bold", "Bold Italic", "Light".
    let mut family_registrations: Vec<(String, Option<String>, Vec<u8>)> = Vec::new();
    for spec in &args.font_family {
        let (lhs, path) = spec
            .split_once('=')
            .with_context(|| format!("--font-family expects NAME[/STYLE]=PATH, got {spec}"))?;
        let (family, style) = match lhs.split_once('/') {
            Some((f, s)) => (f.to_string(), Some(s.to_string())),
            None => (lhs.to_string(), None),
        };
        let bytes =
            std::fs::read(path).with_context(|| format!("read font for {family}: {path}"))?;
        family_registrations.push((family, style, bytes));
    }
    // --font-metrics "Family=ASCENDER[,CAP_HEIGHT[,X_HEIGHT]]" — em
    // fractions. Used to pin baseline math when the substitute font's
    // metrics differ from the IDML-named font's. Repeatable.
    let mut metric_overrides: Vec<(String, paged_renderer::FontMetricsOverride)> = Vec::new();
    for spec in &args.font_metrics {
        let (family, rhs) = spec
            .split_once('=')
            .with_context(|| format!("--font-metrics expects FAMILY=ASCENDER[,...], got {spec}"))?;
        let mut parts = rhs.split(',');
        let ascender: f32 = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing ascender in {spec}"))?
            .parse()
            .with_context(|| format!("parse ascender in {spec}"))?;
        let cap_height = match parts.next() {
            Some(s) if !s.is_empty() => Some(
                s.parse::<f32>()
                    .with_context(|| format!("parse cap_height in {spec}"))?,
            ),
            _ => None,
        };
        let x_height = match parts.next() {
            Some(s) if !s.is_empty() => Some(
                s.parse::<f32>()
                    .with_context(|| format!("parse x_height in {spec}"))?,
            ),
            _ => None,
        };
        metric_overrides.push((
            family.to_string(),
            paged_renderer::FontMetricsOverride {
                ascender,
                cap_height,
                x_height,
            },
        ));
    }
    // Pre-load every image-shaped entry from the IDML container so the
    // resolver can serve URIs that point inside the package (Resources/
    // *.png, embedded JPEGs, etc.). Indexes by full archive path AND
    // by basename — IDML LinkResourceURIs are commonly absolute paths
    // baked at packaging time and we just want to match the basename.
    let embedded_images: Vec<(String, bytes::Bytes)> = source_archive
        .entries
        .iter()
        .filter(|(name, _)| is_image_path(name))
        .map(|(name, bytes)| (name.clone(), bytes.clone()))
        .collect();

    let resolver = if default_font_bytes.is_some()
        || !family_registrations.is_empty()
        || !args.links_dir.is_empty()
        || !embedded_images.is_empty()
    {
        let mut r = paged_renderer::BytesResolver::new();
        for (family, style, bytes) in &family_registrations {
            r.add_font(family, style.as_deref(), bytes.clone());
        }
        if let Some(bytes) = default_font_bytes.as_ref() {
            r.default_font = Some(bytes.clone().into());
        }
        for (path, bytes) in &embedded_images {
            r.add_image(path.clone(), bytes.clone());
            if let Some(name) = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
            {
                r.add_image(name.to_string(), bytes.clone());
            }
        }
        r.link_dirs = args.links_dir.clone();
        Some(r)
    } else {
        None
    };

    // Resolve the CMYK ICC profile bytes — explicit CLI override wins;
    // otherwise probe the document's declared name against the
    // host's Adobe ColorSync install. Naive fallback is fine.
    let cmyk_profile_bytes: Option<Vec<u8>> = if let Some(path) = args.cmyk_profile.as_deref() {
        Some(std::fs::read(path).with_context(|| format!("read {}", path.display()))?)
    } else if let Some(name) = document.designmap.color_settings.cmyk_profile.as_deref() {
        match resolve_cmyk_profile_by_name(name) {
            Some(bytes) => {
                eprintln!("color: using CMYK profile match for {name:?}");
                Some(bytes)
            }
            None => {
                eprintln!("color: no CMYK profile match for {name:?}; falling back to naive math");
                None
            }
        }
    } else {
        None
    };

    let break_page_range = match args.break_page_range.as_deref() {
        Some(s) => {
            let (lo, hi) = s
                .split_once(':')
                .with_context(|| format!("--break-page-range must be START:END (got {s:?})"))?;
            let lo: u32 = lo
                .parse()
                .with_context(|| format!("--break-page-range start: {lo:?}"))?;
            let hi: u32 = hi
                .parse()
                .with_context(|| format!("--break-page-range end: {hi:?}"))?;
            Some(lo..hi)
        }
        None => None,
    };
    let mut opts = PipelineOptions {
        font: font_bytes.as_deref(),
        assets: resolver
            .as_ref()
            .map(|r| r as &dyn paged_renderer::AssetResolver),
        cmyk_icc_profile: cmyk_profile_bytes.as_deref(),
        default_point_size: args.default_size,
        fallback_column_width_pt: args.column_width_pt,
        font_metrics_overrides: &metric_overrides,
        missing_image_placeholder: !args.no_missing_image_placeholder,
        collect_breaks: args.emit_breaks.is_some(),
        break_story_filter: args.break_story_id.clone(),
        break_page_range,
        ..PipelineOptions::default()
    };
    // Explicit for clarity; default already matches.
    opts.fallback_frame_fill =
        paged_compose::Paint::Solid(paged_compose::Color::rgba(0.92, 0.92, 0.92, 1.0));

    let built = pipeline::build_document(&document, &opts)?;
    let total_cmds: usize = built.pages.iter().map(|p| p.list.commands.len()).sum();
    let total_paths: usize = built.pages.iter().map(|p| p.list.paths.len()).sum();

    if let Some(path) = args.emit_breaks.as_deref() {
        use std::io::Write;
        let file =
            std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        let mut w = std::io::BufWriter::new(file);
        for rec in &built.breaks {
            serde_json::to_writer(&mut w, rec)?;
            w.write_all(b"\n")?;
        }
        w.flush()?;
        if !args.json {
            eprintln!(
                "breaks        {} record(s) → {}",
                built.breaks.len(),
                path.display()
            );
        }
    }

    let mut rendered_paths: Vec<(usize, std::path::PathBuf, u32, u32)> = Vec::new();
    if let Some(out) = args.render.as_deref() {
        let multi = built.pages.len() > 1;
        for (i, page) in built.pages.iter().enumerate() {
            let mut raster_opts = paged_gpu::RasterOptions::new(page.width_pt, page.height_pt);
            raster_opts.dpi = args.dpi;
            let img = paged_gpu::rasterize(&page.list, &raster_opts);
            let path = if multi {
                page_output_path(out, i + 1)
            } else {
                out.to_path_buf()
            };
            img.save(&path)
                .with_context(|| format!("write {}", path.display()))?;
            rendered_paths.push((i + 1, path, img.width(), img.height()));
        }
    }

    if args.json {
        let payload = build_json_report(
            &args,
            &document,
            source_archive.mimetype.as_str(),
            palette,
            &built,
            total_cmds,
            total_paths,
            &rendered_paths,
        );
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("\ntotals");
        println!(
            "  pages={pg}  paragraphs={p}  runs={r}  glyphs={g}  lines={l}",
            pg = built.pages.len(),
            p = built.stats.paragraphs,
            r = built.stats.runs,
            g = built.stats.glyphs,
            l = built.stats.lines,
        );
        // Overset signal: lines that fell past the last frame in a chain
        // are dropped (matching InDesign's clipped PDF), but silently
        // dropping them hides genuine overflow. Surface the count so the
        // caller can tell "fit exactly" from "text was clipped".
        if built.stats.dropped_overflow_lines > 0 {
            println!(
                "  overset: {} line(s) dropped past the last frame (text clipped, not lost)",
                built.stats.dropped_overflow_lines,
            );
        }
        // Structured render diagnostics: lossy / degraded outcomes
        // (missing image links, decode failures, section-numbering
        // fallback, footnote overflow) that were previously log-only.
        if !built.diagnostics.is_empty() {
            let (errors, warnings, infos) = built.diagnostics.counts();
            println!(
                "  diagnostics: {} ({} error(s), {} warning(s), {} info)",
                built.diagnostics.len(),
                errors,
                warnings,
                infos,
            );
            for (code, n) in built.diagnostics.by_code() {
                println!("    {code:?}: {n}");
            }
        }
        if want_display_list {
            println!(
                "  display-list: {} command(s) total across {} page(s), {} path(s) total",
                total_cmds,
                built.pages.len(),
                total_paths,
            );
        }
        for (idx, path, w, h) in &rendered_paths {
            println!("  page {idx} rendered {w} × {h} px to {}", path.display());
        }
        if rendered_paths.is_empty() && !want_display_list && font_bytes.is_none() {
            println!("  (pass --font <path>, --display-list, or --render for more detail)");
        }
    }
    Ok(())
}

/// Outcome of a `--roundtrip` run: the compact JSON report plus the
/// overall pass/fail used for the process exit code.
struct RoundtripReport {
    json: serde_json::Value,
    ok: bool,
}

/// Comparable structural fingerprint of a parsed document. Equality of
/// two fingerprints is the "parsed-model stats equality" gate.
#[derive(PartialEq, Eq)]
struct ModelStats {
    spreads: usize,
    stories: usize,
    frames: usize,
    /// Concatenated run text across every story (in document order),
    /// so a content change (or a dropped/added run) trips the gate even
    /// when the run *count* is unchanged.
    run_text: String,
}

impl ModelStats {
    fn of(doc: &Document) -> Self {
        let frames = doc.spreads.iter().map(|s| s.spread.text_frames.len()).sum();
        let run_text = doc
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .flat_map(|p| p.runs.iter())
            .map(|r| r.text.as_str())
            .collect();
        ModelStats {
            spreads: doc.spreads.len(),
            stories: doc.stories.len(),
            frames,
            run_text,
        }
    }
}

/// Decompress every non-directory entry of an IDML package into a
/// path→bytes map. Used for the per-entry byte-identity tally.
fn package_entries(idml: &[u8]) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    use std::io::Read as _;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(idml)).context("open re-zip")?;
    let mut out = std::collections::BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).context("zip entry")?;
        if e.is_dir() {
            continue;
        }
        let name = e.name().to_string();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).context("read entry")?;
        out.insert(name, buf);
    }
    Ok(out)
}

/// Render every page of `doc` to RGBA and return one 32-byte content
/// hash per page (in page order). Reuses the `--render` plumbing:
/// `build_document` → `paged_gpu::rasterize` (CPU backend). Both sides
/// of the round-trip are rendered with identical options + each
/// document's own embedded images, so the comparison is apples-to-apples.
fn render_page_hashes(
    doc: &Document,
    source: &idml_import::SourceArchive,
    dpi: f32,
) -> Result<Vec<[u8; 32]>> {
    // Harvest the document's own embedded images so placed-content URIs
    // pointing inside the package resolve (same logic as the main flow).
    let embedded: Vec<(String, bytes::Bytes)> = source
        .entries
        .iter()
        .filter(|(name, _)| is_image_path(name))
        .map(|(name, bytes)| (name.clone(), bytes.clone()))
        .collect();
    let resolver = if embedded.is_empty() {
        None
    } else {
        let mut r = paged_renderer::BytesResolver::new();
        for (path, b) in &embedded {
            r.add_image(path.clone(), b.clone());
            if let Some(name) = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
            {
                r.add_image(name.to_string(), b.clone());
            }
        }
        Some(r)
    };
    let opts = PipelineOptions {
        assets: resolver
            .as_ref()
            .map(|r| r as &dyn paged_renderer::AssetResolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(doc, &opts)?;
    let mut hashes = Vec::with_capacity(built.pages.len());
    for page in &built.pages {
        let mut raster_opts = paged_gpu::RasterOptions::new(page.width_pt, page.height_pt);
        raster_opts.dpi = dpi;
        let img = paged_gpu::rasterize(&page.list, &raster_opts);
        // FNV-1a-ish? No — keep it a real cryptographic-strength digest
        // so a single-pixel divergence is caught. blake3 isn't a dep
        // here, but the renderer already pulls in nothing hash-shaped;
        // use a stable 32-byte digest over the raw RGBA buffer.
        hashes.push(hash_rgba(img.as_raw()));
    }
    Ok(hashes)
}

/// Stable 32-byte digest of an RGBA buffer. Uses the `image` crate's
/// raw byte slice; the digest only needs to be deterministic + change
/// on any pixel difference, not collision-resistant against an
/// adversary — a doubled FNV-1a over interleaved halves keeps it
/// dependency-free while spreading bytes across all 32 output bytes.
fn hash_rgba(raw: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // Four independent FNV-1a lanes seeded differently, each consuming
    // the whole buffer — cheap, deterministic, and sensitive to any
    // single-byte change (FNV-1a's avalanche is adequate for a
    // pixel-equality gate; this is not a security boundary).
    const SEEDS: [u64; 4] = [
        0xcbf29ce484222325,
        0x100000001b3,
        0x9e3779b97f4a7c15,
        0xff51afd7ed558ccd,
    ];
    for (lane, &seed) in SEEDS.iter().enumerate() {
        let mut h = seed;
        for (i, &b) in raw.iter().enumerate() {
            // Fold the index in so a transposition (same bytes, moved)
            // also changes the digest.
            h ^= b as u64 ^ (i as u64).rotate_left(lane as u32 * 7);
            h = h.wrapping_mul(0x100000001b3);
        }
        out[lane * 8..lane * 8 + 8].copy_from_slice(&h.to_le_bytes());
    }
    out
}

/// Run the full save-back round-trip and assemble the report.
///
/// Exit-0 criterion: re-parse succeeds AND model stats match AND every
/// page hashes identically. The per-entry identical/patched tally is
/// reported for visibility but is NOT a gate (a faithfully-patched
/// entry is a correct round-trip, not a failure).
fn run_roundtrip(original: &[u8], dpi: f32) -> Result<RoundtripReport> {
    use serde_json::json;

    let (doc, doc_source) = idml_import::import_idml(original).context("open input IDML")?;
    let written = idml_export::write_idml(&doc, original).context("write_idml")?;

    // (a) Per-entry byte-identity tally.
    let src = package_entries(original)?;
    let dst = package_entries(&written)?;
    let mut entries_identical = 0usize;
    let mut entries_patched = 0usize;
    for (name, src_bytes) in &src {
        match dst.get(name) {
            Some(d) if d == src_bytes => entries_identical += 1,
            _ => entries_patched += 1,
        }
    }

    // (b) Re-parse + parsed-model stats equality.
    let reparse = idml_import::import_idml(&written);
    let (reparsed_ok, stats_match) = match &reparse {
        Ok((re, _)) => (true, ModelStats::of(&doc) == ModelStats::of(re)),
        Err(_) => (false, false),
    };

    // (c) Render both → per-page hash equality. Only attempted when the
    // re-parse succeeded (no doc to render otherwise).
    let (pages_identical, page_count) = match &reparse {
        Ok((re, re_source)) => {
            let a = render_page_hashes(&doc, &doc_source, dpi)?;
            let b = render_page_hashes(re, re_source, dpi)?;
            (a.len() == b.len() && a == b, a.len())
        }
        Err(_) => (false, 0),
    };

    let ok = reparsed_ok && stats_match && pages_identical;
    let report = json!({
        "entries_identical": entries_identical,
        "entries_patched": entries_patched,
        "stats_match": stats_match,
        "pages_identical": pages_identical,
        "page_count": page_count,
    });
    Ok(RoundtripReport { json: report, ok })
}

/// Outcome of a `--mutate-roundtrip` run.
struct MutateRoundtripReport {
    json: serde_json::Value,
    /// Process exit code gate: `true` ⇒ exit 0. True when the mutation
    /// fully round-tripped (`ok`), when the document had no target for the
    /// mutation (n/a), or when the only loss is a documented defer
    /// (`insertPage`). Only a genuine apply/write failure flips this off.
    exit_ok: bool,
}

/// The page-item the frame-targeting mutations address: the first
/// TextFrame (preferred — it also carries a story) else the first
/// Rectangle, identified across every spread.
enum FrameTarget {
    TextFrame(String),
    Rectangle(String),
}

impl FrameTarget {
    fn self_id(&self) -> &str {
        match self {
            FrameTarget::TextFrame(s) | FrameTarget::Rectangle(s) => s,
        }
    }
    fn node(&self) -> paged_mutate::NodeId {
        match self {
            FrameTarget::TextFrame(s) => paged_mutate::NodeId::TextFrame(s.clone()),
            FrameTarget::Rectangle(s) => paged_mutate::NodeId::Rectangle(s.clone()),
        }
    }
}

/// Pick the first TextFrame (with a `Self` id) else the first Rectangle,
/// scanning spreads in document order. `None` ⇒ the document carries no
/// addressable frame (the n/a path — e.g. the storyless `corners.idml`).
fn first_frame_target(doc: &Document) -> Option<FrameTarget> {
    for s in &doc.spreads {
        if let Some(id) = s.spread.text_frames.iter().find_map(|f| f.self_id.clone()) {
            return Some(FrameTarget::TextFrame(id));
        }
    }
    for s in &doc.spreads {
        if let Some(id) = s.spread.rectangles.iter().find_map(|r| r.self_id.clone()) {
            return Some(FrameTarget::Rectangle(id));
        }
    }
    None
}

/// Read a frame's `(fill_color, stroke_weight, item_transform)` out of a
/// (re-parsed) document by `Self` id, checking both the TextFrame and
/// Rectangle pools. `None` ⇒ the frame is gone (a structural loss).
#[allow(clippy::type_complexity)]
fn frame_props(
    doc: &Document,
    self_id: &str,
) -> Option<(Option<String>, Option<f32>, Option<[f32; 6]>)> {
    for s in &doc.spreads {
        for f in &s.spread.text_frames {
            if f.self_id.as_deref() == Some(self_id) {
                return Some((f.fill_color.clone(), f.stroke_weight, f.item_transform));
            }
        }
        for r in &s.spread.rectangles {
            if r.self_id.as_deref() == Some(self_id) {
                return Some((r.fill_color.clone(), r.stroke_weight, r.item_transform));
            }
        }
    }
    None
}

/// The first story (in document order) carrying at least one
/// non-empty run, as `(story_id, first_run_text)`. `None` ⇒ no text to
/// resize (the n/a path).
fn first_nonempty_story(doc: &Document) -> Option<(String, String)> {
    for s in &doc.stories {
        if let Some(run) = s
            .story
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter())
            .find(|r| !r.text.is_empty())
        {
            return Some((s.self_id.clone(), run.text.clone()));
        }
    }
    None
}

/// The point size of a story's first non-empty run (post-reparse).
fn first_run_point_size(doc: &Document, story_id: &str) -> Option<f32> {
    doc.stories
        .iter()
        .find(|s| s.self_id == story_id)
        .and_then(|s| {
            s.story
                .paragraphs
                .iter()
                .flat_map(|p| p.runs.iter())
                .find(|r| !r.text.is_empty())
                .and_then(|r| r.point_size)
        })
}

/// Run the W4.14 mutate-and-reverify flow for one mutation kind.
///
/// `apply` ⇒ the Operation applied cleanly to the model. `survived` ⇒ the
/// mutated value re-parsed equal after `write_idml`. `untouched_ok` ⇒ the
/// document's structural fingerprint (spread / story / frame counts + run
/// text) matches the mutated model. `ok = apply && survived &&
/// untouched_ok`.
fn run_mutate_roundtrip(original: &[u8], mutation: &str) -> Result<MutateRoundtripReport> {
    use paged_mutate::{NodeId, Operation, Project, PropertyPath, Value};
    use serde_json::json;

    let (doc, _source) = idml_import::import_idml(original).context("open input IDML")?;

    // Each arm yields (op, target_self_id, verify-closure, note). `target`
    // being `None` is the n/a path: no target for this mutation → exit 0
    // with applied=false. The verify closure reads the re-parsed doc and
    // returns whether the mutated value survived.
    type Verify = Box<dyn Fn(&Document) -> bool>;
    struct Plan {
        op: Operation,
        verify: Verify,
        /// A note that overrides the default; e.g. `insertPage`'s known
        /// loss. Empty ⇒ the default note is synthesised from outcomes.
        note: Option<&'static str>,
    }

    let plan: Option<Plan> = match mutation {
        "setFrameStrokeWeight" => first_frame_target(&doc).map(|t| {
            let id = t.self_id().to_string();
            Plan {
                op: Operation::SetProperty {
                    node: t.node(),
                    path: PropertyPath::FrameStrokeWeight,
                    value: Value::Length(Some(3.5)),
                },
                verify: Box::new(move |re: &Document| {
                    frame_props(re, &id)
                        .and_then(|(_, sw, _)| sw)
                        .map(|sw| (sw - 3.5).abs() < 1e-3)
                        .unwrap_or(false)
                }),
                note: None,
            }
        }),
        "setFrameFill" => {
            let target = first_frame_target(&doc);
            // A real swatch that isn't the no-fill sentinel — prefer a
            // colour, fall back to any named swatch.
            let swatch = doc
                .palette
                .colors
                .keys()
                .find(|id| !id.contains("[None]") && id.as_str() != "Swatch/None")
                .or_else(|| {
                    doc.palette
                        .swatches
                        .keys()
                        .find(|id| !id.contains("[None]") && id.as_str() != "Swatch/None")
                })
                .cloned();
            match (target, swatch) {
                (Some(t), Some(swatch)) => {
                    let id = t.self_id().to_string();
                    let want = swatch.clone();
                    Some(Plan {
                        op: Operation::SetProperty {
                            node: t.node(),
                            path: PropertyPath::FrameFillColor,
                            value: Value::ColorRef(Some(swatch)),
                        },
                        verify: Box::new(move |re: &Document| {
                            frame_props(re, &id)
                                .map(|(fill, _, _)| fill.as_deref() == Some(want.as_str()))
                                .unwrap_or(false)
                        }),
                        note: None,
                    })
                }
                // No frame, or no usable swatch → n/a.
                _ => None,
            }
        }
        "setFrameTransform" => first_frame_target(&doc).map(|t| {
            let id = t.self_id().to_string();
            // Compose a +11,+7 translate ONTO the frame's current
            // transform (identity when absent), so the result is the
            // original shifted — the renderer applies the matrix verbatim.
            let base = frame_props(&doc, &id)
                .and_then(|(_, _, m)| m)
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            let mut want = base;
            want[4] = base[4] + 11.0;
            want[5] = base[5] + 7.0;
            Plan {
                op: Operation::SetProperty {
                    node: t.node(),
                    path: PropertyPath::FrameTransform,
                    value: Value::Transform(Some(want)),
                },
                verify: Box::new(move |re: &Document| {
                    frame_props(re, &id)
                        .and_then(|(_, _, m)| m)
                        .map(|m| (m[4] - want[4]).abs() < 1e-3 && (m[5] - want[5]).abs() < 1e-3)
                        .unwrap_or(false)
                }),
                note: None,
            }
        }),
        "setCharFontSize" => first_nonempty_story(&doc).map(|(story_id, first_text)| {
            let id = story_id.clone();
            // Address the WHOLE first run, not just `[0, 1)`. A sub-run
            // range would SPLIT the run (model gains a run the source XML
            // has no `<CharacterStyleRange>` for); `rewrite_story` patches
            // ranges positionally and can't yet emit an inserted run, so
            // the split-out tail would be dropped (the W3 run-insert
            // defer). Covering the full run mutates its single range in
            // place — a clean round-trip with no structural change.
            let end = first_text.chars().count() as u32;
            Plan {
                op: Operation::SetProperty {
                    node: NodeId::StoryRange {
                        story_id,
                        start: 0,
                        end,
                    },
                    path: PropertyPath::CharacterFontSize,
                    value: Value::Length(Some(18.0)),
                },
                verify: Box::new(move |re: &Document| {
                    first_run_point_size(re, &id)
                        .map(|pt| (pt - 18.0).abs() < 1e-3)
                        .unwrap_or(false)
                }),
                note: None,
            }
        }),
        "insertPage" => {
            // Insert a page after the first page. C-8: the writer emits
            // the minted spread as a full new entry + designmap ref, so
            // the page must SURVIVE the round-trip (page count grows).
            let after_page_id = doc
                .spreads
                .iter()
                .find_map(|s| s.spread.pages.iter().find_map(|p| p.self_id.clone()));
            let pages =
                |d: &Document| -> usize { d.spreads.iter().map(|s| s.spread.pages.len()).sum() };
            let pages_before = pages(&doc);
            Some(Plan {
                op: Operation::InsertPage {
                    after_page_id,
                    master_id: None,
                    spread_self_id: None,
                    page_self_id: None,
                    restore_spread_json: None,
                },
                verify: Box::new(move |re: &Document| pages(re) == pages_before + 1),
                note: None,
            })
        }
        other => {
            anyhow::bail!(
                "unknown --mutate-roundtrip mutation {other:?}; expected one of \
                 setFrameStrokeWeight|setFrameFill|setFrameTransform|setCharFontSize|insertPage"
            );
        }
    };

    // n/a path — the document carries no target for this mutation. Not a
    // failure: exit 0 with applied=false.
    let Some(plan) = plan else {
        let report = json!({
            "mutation": mutation,
            "applied": false,
            "survived": false,
            "untouched_ok": true,
            "ok": false,
            "note": "n/a: no target in document",
        });
        return Ok(MutateRoundtripReport {
            json: report,
            exit_ok: true,
        });
    };

    // Apply the Operation. A genuine apply failure is the one case that
    // exits 1 (the model rejected a valid Op — a real bug).
    let mut project = Project::new(doc);
    if let Err(e) = project.apply(plan.op) {
        let report = json!({
            "mutation": mutation,
            "applied": false,
            "survived": false,
            "untouched_ok": false,
            "ok": false,
            "note": format!("apply failed: {e}"),
        });
        return Ok(MutateRoundtripReport {
            json: report,
            exit_ok: false,
        });
    }

    // Save the mutated model back through the writer. A write failure is
    // also a genuine bug → exit 1.
    let written = match idml_export::write_idml(project.document(), original) {
        Ok(w) => w,
        Err(e) => {
            let report = json!({
                "mutation": mutation,
                "applied": true,
                "survived": false,
                "untouched_ok": false,
                "ok": false,
                "note": format!("write_idml failed: {e}"),
            });
            return Ok(MutateRoundtripReport {
                json: report,
                exit_ok: false,
            });
        }
    };

    // Re-open the written package. A reparse failure is a genuine bug.
    let reparsed = match idml_import::import_idml(&written) {
        Ok((re, _)) => re,
        Err(e) => {
            let report = json!({
                "mutation": mutation,
                "applied": true,
                "survived": false,
                "untouched_ok": false,
                "ok": false,
                "note": format!("reparse failed: {e}"),
            });
            return Ok(MutateRoundtripReport {
                json: report,
                exit_ok: false,
            });
        }
    };

    let survived = (plan.verify)(&reparsed);
    // Structure: the reparse must reproduce the MUTATED model's
    // fingerprint. For the property mutations that equals the pre-apply
    // stats; for insertPage it includes the minted spread (C-8), so the
    // comparison is against the post-apply model, not `before`.
    let untouched_ok = ModelStats::of(project.document()) == ModelStats::of(&reparsed);
    let ok = survived && untouched_ok;

    let note: String = match plan.note {
        Some(n) => n.to_string(),
        None if ok => "ok".to_string(),
        None if !survived => "mutated value did not survive round-trip".to_string(),
        None => "untouched structure diverged".to_string(),
    };

    let exit_ok = ok;

    let report = json!({
        "mutation": mutation,
        "applied": true,
        "survived": survived,
        "untouched_ok": untouched_ok,
        "ok": ok,
        "note": note,
    });
    Ok(MutateRoundtripReport {
        json: report,
        exit_ok,
    })
}

fn build_json_report(
    args: &Args,
    document: &Document,
    mimetype: &str,
    palette: &Graphic,
    built: &pipeline::BuiltDocument,
    total_cmds: usize,
    total_paths: usize,
    rendered_paths: &[(usize, std::path::PathBuf, u32, u32)],
) -> serde_json::Value {
    use serde_json::json;

    let spreads: Vec<serde_json::Value> = document
        .spreads
        .iter()
        .map(|s| {
            json!({
                "src": s.src,
                "pages": s.spread.pages.iter().map(|p| json!({
                    "self_id": p.self_id,
                    "width_pt": p.bounds.width(),
                    "height_pt": p.bounds.height(),
                    "applied_master": p.applied_master,
                })).collect::<Vec<_>>(),
                "text_frames": s.spread.text_frames.iter().map(|f| json!({
                    "self_id": f.self_id,
                    "parent_story": f.parent_story,
                    "width_pt": f.bounds.width(),
                    "height_pt": f.bounds.height(),
                    "fill_color": f.fill_color,
                    "stroke_color": f.stroke_color,
                    "stroke_weight": f.stroke_weight,
                })).collect::<Vec<_>>(),
                "rectangles": s.spread.rectangles.len(),
                "ovals": s.spread.ovals.len(),
                "graphic_lines": s.spread.graphic_lines.len(),
                "skipped_nested_frames": s.spread.skipped_nested_frames,
            })
        })
        .collect();

    let stories: Vec<serde_json::Value> = document
        .stories
        .iter()
        .map(|s| {
            json!({
                "src": s.src,
                "self_id": s.self_id,
                "paragraphs": s.story.paragraphs.len(),
                "runs": s.story.paragraphs.iter().map(|p| p.runs.len()).sum::<usize>(),
            })
        })
        .collect();

    let pages: Vec<serde_json::Value> = built
        .pages
        .iter()
        .map(|p| {
            json!({
                "width_pt": p.width_pt,
                "height_pt": p.height_pt,
                "commands": p.list.commands.len(),
                "paths": p.list.paths.len(),
                "stats": {
                    "frames": p.stats.frames,
                    "paragraphs": p.stats.paragraphs,
                    "runs": p.stats.runs,
                    "glyphs": p.stats.glyphs,
                    "lines": p.stats.lines,
                },
            })
        })
        .collect();

    let renders: Vec<serde_json::Value> = rendered_paths
        .iter()
        .map(
            |(idx, path, w, h)| json!({ "page": idx, "path": path, "width_px": w, "height_px": h }),
        )
        .collect();

    json!({
        "file": args.file,
        "mimetype": mimetype,
        "dom_version": document.designmap.dom_version,
        "manifest": {
            "spreads": document.designmap.spreads.len(),
            "stories": document.designmap.stories.len(),
            "masters": document.designmap.master_spreads.len(),
        },
        "palette": {
            "colors": palette.colors.len(),
            "swatches": palette.swatches.len(),
        },
        "spreads": spreads,
        "stories": stories,
        "pages": pages,
        "totals": {
            "spreads": built.stats.spreads,
            "pages": built.stats.pages,
            "frames": built.stats.frames,
            "stories": built.stats.stories,
            "paragraphs": built.stats.paragraphs,
            "runs": built.stats.runs,
            "glyphs": built.stats.glyphs,
            "lines": built.stats.lines,
            "decoded_images": built.stats.decoded_images,
            "dropped_overflow_lines": built.stats.dropped_overflow_lines,
            "commands": total_cmds,
            "unique_paths": total_paths,
        },
        "renders": renders,
        "diagnostics": built.diagnostics.items,
    })
}

/// Resolve an IDML-declared `CMYKProfile` name (e.g. `"Coated FOGRA39
/// (ISO 12647-2:2004)"`) to ICC bytes by mapping common Adobe profile
/// names to Adobe's standard Recommended/ filenames, then probing the
/// host's per-platform install location. We deliberately avoid bundling
/// these — they're large and individually licensed by their issuers.
fn resolve_cmyk_profile_by_name(name: &str) -> Option<Vec<u8>> {
    let trimmed = name.trim();
    // "$ID/" is InDesign's sentinel for "use the application default"
    // — no profile was declared in the document. The corpus diff
    // harness forces pdftoppm to FOGRA39 for the reference PDF, so
    // matching that here keeps the candidate render and the reference
    // rasterisation in the same colour space.
    if trimmed == "$ID/" || trimmed.is_empty() {
        return load_profile_bytes("CoatedFOGRA39.icc");
    }
    // Try the full declared name first (handles mid-name parentheticals
    // like `"U.S. Web Coated (SWOP) v2"`), then retry with a trailing
    // parenthetical stripped (handles version-note suffixes like
    // `"Coated FOGRA39 (ISO 12647-2:2004)"`).
    if let Some(bytes) = lookup_cmyk_profile_filename(trimmed).and_then(load_profile_bytes) {
        return Some(bytes);
    }
    if let Some(head) = trimmed
        .split_once('(')
        .map(|(h, _)| h.trim())
        .filter(|h| !h.is_empty())
    {
        if let Some(bytes) = lookup_cmyk_profile_filename(head).and_then(load_profile_bytes) {
            return Some(bytes);
        }
    }
    None
}

fn lookup_cmyk_profile_filename(name: &str) -> Option<&'static str> {
    Some(match name {
        "Coated FOGRA39" | "Coated Fogra39" => "CoatedFOGRA39.icc",
        "Coated FOGRA27" => "CoatedFOGRA27.icc",
        "Uncoated FOGRA29" => "UncoatedFOGRA29.icc",
        "Web Coated FOGRA28" => "WebCoatedFOGRA28.icc",
        "Coated GRACoL 2006" | "Coated GRACoL2006" => "CoatedGRACoL2006.icc",
        "U.S. Web Coated (SWOP) v2" | "U.S. Web Coated SWOP v2" => "USWebCoatedSWOP.icc",
        "U.S. Sheetfed Coated v2" => "USSheetfedCoated.icc",
        "U.S. Sheetfed Uncoated v2" => "USSheetfedUncoated.icc",
        "U.S. Web Uncoated v2" => "USWebUncoated.icc",
        "Web Coated SWOP 2006 Grade 3 Paper" => "WebCoatedSWOP2006Grade3.icc",
        "Web Coated SWOP 2006 Grade 5 Paper" => "WebCoatedSWOP2006Grade5.icc",
        "Japan Color 2001 Coated" => "JapanColor2001Coated.icc",
        "Japan Color 2001 Uncoated" => "JapanColor2001Uncoated.icc",
        "Japan Color 2002 Newspaper" => "JapanColor2002Newspaper.icc",
        "Japan Color 2003 Web Coated" => "JapanColor2003WebCoated.icc",
        "Japan Web Coated (Ad)" | "Japan Web Coated" => "JapanWebCoated.icc",
        "US Newsprint (SNAP 2007)" => "USNewsprintSNAP2007.icc",
        _ => return None,
    })
}

fn load_profile_bytes(filename: &str) -> Option<Vec<u8>> {
    // Per-platform Adobe Recommended dirs (Adobe Creative Cloud and
    // legacy Adobe Color package both install here). The user can
    // always override via --cmyk-profile when these miss.
    let dirs: &[&str] = if cfg!(target_os = "macos") {
        &["/Library/Application Support/Adobe/Color/Profiles/Recommended"]
    } else if cfg!(target_os = "windows") {
        &["C:/Program Files (x86)/Common Files/Adobe/Color/Profiles/Recommended"]
    } else {
        &["/usr/share/color/icc", "/usr/share/color/icc/colord"]
    };
    for dir in dirs {
        let candidate = std::path::Path::new(dir).join(filename);
        if let Ok(bytes) = std::fs::read(&candidate) {
            return Some(bytes);
        }
    }
    None
}

/// True for archive paths whose extension marks them as a bitmap or
/// PDF placed-content asset. Used to harvest the IDML container's
/// embedded images into the asset resolver so LinkResourceURIs that
/// point inside the package resolve without an external Links/ dir.
fn is_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        std::path::Path::new(&lower)
            .extension()
            .and_then(|s| s.to_str()),
        Some(
            "png" | "jpg" | "jpeg" | "gif" | "tif" | "tiff" | "bmp" | "webp" | "pdf" | "psd" | "ai"
        )
    )
}

fn page_output_path(base: &std::path::Path, page_index: usize) -> std::path::PathBuf {
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("png");
    let parent = base.parent().unwrap_or(std::path::Path::new(""));
    parent.join(format!("{stem}-{page_index:03}.{ext}"))
}

fn describe_fill(fill_color: Option<&str>, palette: &Graphic) -> String {
    let Some(id) = fill_color else {
        return "(none)".to_string();
    };
    match palette.resolve(id) {
        Some(entry) => {
            let name = entry.name.as_deref().unwrap_or(&entry.self_id);
            match to_linear_rgb(entry) {
                Some(rgb) => format!(
                    "{name} [{:?} rgb≈{:.2},{:.2},{:.2}]",
                    entry.space, rgb[0], rgb[1], rgb[2]
                ),
                None => format!("{name} [{:?} unconverted]", entry.space),
            }
        }
        None => format!("{id} (unresolved)"),
    }
}

fn first_line(s: &str) -> String {
    const MAX: usize = 60;
    let line = s.split('\n').next().unwrap_or("");
    if line.chars().count() > MAX {
        format!("{}…", line.chars().take(MAX).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::lookup_cmyk_profile_filename;

    #[test]
    fn resolves_mid_name_parenthetical() {
        // InDesign emits "U.S. Web Coated (SWOP) v2" with the
        // parenthetical mid-name. The resolver must look up the full
        // string verbatim, not a parenthetical-stripped head.
        assert_eq!(
            lookup_cmyk_profile_filename("U.S. Web Coated (SWOP) v2"),
            Some("USWebCoatedSWOP.icc")
        );
    }

    #[test]
    fn resolves_trailing_parenthetical_via_head() {
        // For `"Coated FOGRA39 (ISO 12647-2:2004)"` the resolver in
        // resolve_cmyk_profile_by_name strips the trailing version
        // note and falls back to the bare family name.
        assert_eq!(
            lookup_cmyk_profile_filename("Coated FOGRA39"),
            Some("CoatedFOGRA39.icc")
        );
    }

    #[test]
    fn unknown_name_returns_none() {
        assert!(lookup_cmyk_profile_filename("Some Made Up Profile").is_none());
    }
}
