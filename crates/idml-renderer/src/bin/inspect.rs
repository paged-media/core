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
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bytes =
        std::fs::read(&args.file).with_context(|| format!("read {}", args.file.display()))?;
    let document = Document::open(&bytes).context("open IDML")?;
    let palette = &document.palette;

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

    // Everything below is driven by the library.
    let want_display_list = args.display_list || args.render.is_some();
    let font_bytes = args
        .font
        .as_deref()
        .map(|p| std::fs::read(p).with_context(|| format!("read font {}", p.display())))
        .transpose()?;

    let mut opts = PipelineOptions {
        font: font_bytes.as_deref(),
        default_point_size: args.default_size,
        fallback_column_width_pt: args.column_width_pt,
        ..PipelineOptions::default()
    };
    // Explicit for clarity; default already matches.
    opts.fallback_frame_fill =
        idml_compose::Paint::Solid(idml_compose::Color::rgba(0.92, 0.92, 0.92, 1.0));

    let built = pipeline::build_document(&document, &opts)?;
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
        let total_cmds: usize = built.pages.iter().map(|p| p.list.commands.len()).sum();
        let total_paths: usize = built.pages.iter().map(|p| p.list.paths.len()).sum();
        println!(
            "  display-list: {} command(s) total across {} page(s), {} path(s) total",
            total_cmds,
            built.pages.len(),
            total_paths,
        );
    }
    if let Some(out) = args.render.as_deref() {
        // Multi-page output writes <stem>-001.png, <stem>-002.png, …
        // when the document has more than one page. Single-page docs
        // get the unmodified path so existing scripts still work.
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
            println!(
                "  page {} rendered {} × {} px to {}",
                i + 1,
                img.width(),
                img.height(),
                path.display()
            );
        }
    } else if !want_display_list && font_bytes.is_none() {
        println!("  (pass --font <path>, --display-list, or --render for more detail)");
    }
    Ok(())
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
