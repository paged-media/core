//! `idml-inspect`: open an IDML, run the renderer pipeline, and print
//! a human-readable summary of what happened.
//!
//! Pure CLI wrapper over `idml_renderer::pipeline` — everything
//! structural lives in the library so other hosts (WASM, tests) can
//! drive the same flow without re-parsing the argv.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use idml_parse::{graphic, Container, Graphic, Spread, Story};
use idml_renderer::{pipeline, PipelineOptions};

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
    let container = Container::open(&bytes).context("open IDML")?;

    println!("file          {}", args.file.display());
    println!("mimetype      {}", container.mimetype);
    println!(
        "manifest      {} spread(s), {} story ref(s), {} master(s)",
        container.designmap.spreads.len(),
        container.designmap.stories.len(),
        container.designmap.master_spreads.len(),
    );

    let palette = container
        .entry("Resources/Graphic.xml")
        .map(|raw| Graphic::parse(raw))
        .transpose()?
        .unwrap_or_default();
    if !palette.colors.is_empty() || !palette.swatches.is_empty() {
        println!(
            "palette       {} colour(s), {} swatch(es)",
            palette.colors.len(),
            palette.swatches.len(),
        );
    }

    // Per-spread / per-story pretty output — independent of the
    // library pipeline (which is a single-pass flatten without
    // intermediate logging).
    for spread_ref in &container.designmap.spreads {
        let Some(raw) = container.entry(&spread_ref.src) else {
            eprintln!(
                "warning: manifest lists {} but archive has no such entry",
                spread_ref.src
            );
            continue;
        };
        let spread = Spread::parse(raw)?;
        println!(
            "\nspread        {}  ({} page(s), {} frame(s){})",
            spread_ref.src,
            spread.pages.len(),
            spread.text_frames.len(),
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
                describe_fill(frame, &palette),
            );
        }
    }

    for story_ref in &container.designmap.stories {
        let Some(raw) = container.entry(&story_ref.src) else {
            continue;
        };
        let story = Story::parse(raw)?;
        println!(
            "\nstory         {}  ({} paragraph(s))",
            story_ref.src,
            story.paragraphs.len(),
        );
        for (pi, p) in story.paragraphs.iter().enumerate() {
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

    let built = pipeline::build(&container, &palette, &opts)?;
    println!("\ntotals");
    println!(
        "  paragraphs={p}  runs={r}  glyphs={g}  lines={l}",
        p = built.stats.paragraphs,
        r = built.stats.runs,
        g = built.stats.glyphs,
        l = built.stats.lines,
    );
    if want_display_list {
        println!(
            "  display-list: {} command(s), {} unique path(s)",
            built.list.commands.len(),
            built.list.paths.len(),
        );
    }
    if let Some(out) = args.render.as_deref() {
        let mut raster_opts = idml_gpu::RasterOptions::new(built.width_pt, built.height_pt);
        raster_opts.dpi = args.dpi;
        let img = idml_gpu::rasterize(&built.list, &raster_opts);
        img.save(out)
            .with_context(|| format!("write {}", out.display()))?;
        println!(
            "  rendered {} × {} px to {}",
            img.width(),
            img.height(),
            out.display()
        );
    } else if !want_display_list && font_bytes.is_none() {
        println!("  (pass --font <path>, --display-list, or --render for more detail)");
    }
    Ok(())
}

fn describe_fill(frame: &idml_parse::TextFrame, palette: &Graphic) -> String {
    let Some(id) = frame.fill_color.as_deref() else {
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
