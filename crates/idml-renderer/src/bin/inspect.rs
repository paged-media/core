//! `idml-inspect`: open an IDML, run the renderer pipeline, and print
//! a human-readable summary of what happened.
//!
//! Pure CLI wrapper over `idml_renderer::pipeline` — everything
//! structural lives in the library so other hosts (WASM, tests) can
//! drive the same flow without re-parsing the argv.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use idml_parse::{graphic, Graphic};
use idml_renderer::{pipeline, Document, PipelineOptions};

#[derive(Parser, Debug)]
#[command(name = "idml-inspect", version, about)]
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
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bytes =
        std::fs::read(&args.file).with_context(|| format!("read {}", args.file.display()))?;
    let document = Document::open(&bytes).context("open IDML")?;
    let palette = &document.palette;

    if !args.json {
        println!("file          {}", args.file.display());
        println!("mimetype      {}", document.container.mimetype);
        println!(
            "manifest      {} spread(s), {} story ref(s), {} master(s)",
            document.container.designmap.spreads.len(),
            document.container.designmap.stories.len(),
            document.container.designmap.master_spreads.len(),
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
    let mut metric_overrides: Vec<(String, idml_renderer::FontMetricsOverride)> = Vec::new();
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
            Some(s) if !s.is_empty() => Some(s.parse::<f32>().with_context(|| format!("parse cap_height in {spec}"))?),
            _ => None,
        };
        let x_height = match parts.next() {
            Some(s) if !s.is_empty() => Some(s.parse::<f32>().with_context(|| format!("parse x_height in {spec}"))?),
            _ => None,
        };
        metric_overrides.push((
            family.to_string(),
            idml_renderer::FontMetricsOverride {
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
    let embedded_images: Vec<(String, bytes::Bytes)> = document
        .container
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
        let mut r = idml_renderer::BytesResolver::new();
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
    } else if let Some(name) = document
        .container
        .designmap
        .color_settings
        .cmyk_profile
        .as_deref()
    {
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

    let mut opts = PipelineOptions {
        font: font_bytes.as_deref(),
        assets: resolver
            .as_ref()
            .map(|r| r as &dyn idml_renderer::AssetResolver),
        cmyk_icc_profile: cmyk_profile_bytes.as_deref(),
        default_point_size: args.default_size,
        fallback_column_width_pt: args.column_width_pt,
        font_metrics_overrides: &metric_overrides,
        missing_image_placeholder: !args.no_missing_image_placeholder,
        ..PipelineOptions::default()
    };
    // Explicit for clarity; default already matches.
    opts.fallback_frame_fill =
        idml_compose::Paint::Solid(idml_compose::Color::rgba(0.92, 0.92, 0.92, 1.0));

    let built = pipeline::build_document(&document, &opts)?;
    let total_cmds: usize = built.pages.iter().map(|p| p.list.commands.len()).sum();
    let total_paths: usize = built.pages.iter().map(|p| p.list.paths.len()).sum();

    let mut rendered_paths: Vec<(usize, std::path::PathBuf, u32, u32)> = Vec::new();
    if let Some(out) = args.render.as_deref() {
        let multi = built.pages.len() > 1;
        for (i, page) in built.pages.iter().enumerate() {
            let mut raster_opts = idml_gpu::RasterOptions::new(page.width_pt, page.height_pt);
            raster_opts.dpi = args.dpi;
            let img = idml_gpu::rasterize(&page.list, &raster_opts);
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

fn build_json_report(
    args: &Args,
    document: &Document,
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
        "mimetype": document.container.mimetype,
        "manifest": {
            "spreads": document.container.designmap.spreads.len(),
            "stories": document.container.designmap.stories.len(),
            "masters": document.container.designmap.master_spreads.len(),
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
            "commands": total_cmds,
            "unique_paths": total_paths,
        },
        "renders": renders,
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
            match graphic::to_linear_rgb(entry) {
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
