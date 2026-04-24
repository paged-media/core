//! `idml-inspect`: open an IDML, parse its manifest and stories, and
//! print a human-readable summary. Exercises the parse + story + shape
//! pipeline end-to-end against real IDML bytes.
//!
//! With `--font <path>`, also shapes every run via rustybuzz and reports
//! glyph counts — proving the text engine's first primitive works against
//! real IDML content.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use idml_parse::{Container, Story};

#[derive(Parser, Debug)]
#[command(name = "idml-inspect", version, about)]
struct Args {
    /// IDML file to inspect.
    file: PathBuf,
    /// Optional TTF/OTF font to use for shaping every run. When absent,
    /// shaping is skipped and only text extraction is reported.
    #[arg(long)]
    font: Option<PathBuf>,
    /// Default point size used for shaping when a run has none.
    #[arg(long, default_value_t = 12.0)]
    default_size: f32,
    /// Column width in pt. When set together with --font, each
    /// paragraph is composed and the line count is reported.
    #[arg(long)]
    column_width_pt: Option<f32>,
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

    let font_bytes = args
        .font
        .as_deref()
        .map(|p| std::fs::read(p).with_context(|| format!("read font {}", p.display())))
        .transpose()?;
    let face = font_bytes
        .as_deref()
        .and_then(|b| rustybuzz::Face::from_slice(b, 0));

    let mut total_paragraphs = 0usize;
    let mut total_runs = 0usize;
    let mut total_chars = 0usize;
    let mut total_glyphs = 0usize;
    let mut total_lines = 0usize;

    for story_ref in &container.designmap.stories {
        let Some(raw) = container.entry(&story_ref.src) else {
            eprintln!(
                "warning: manifest lists {} but archive has no such entry",
                story_ref.src
            );
            continue;
        };
        let story = Story::parse(raw)?;
        println!(
            "\nstory         {}  ({} paragraph(s))",
            story_ref.src,
            story.paragraphs.len()
        );
        for (pi, p) in story.paragraphs.iter().enumerate() {
            total_paragraphs += 1;
            // Representative point size for the paragraph: first run's.
            let paragraph_size = p
                .runs
                .first()
                .and_then(|r| r.point_size)
                .unwrap_or(args.default_size);
            // Concatenate all runs so compose sees the whole paragraph.
            let paragraph_text: String = p.runs.iter().map(|r| r.text.as_str()).collect();

            // Per-run shaping report.
            for (ri, r) in p.runs.iter().enumerate() {
                total_runs += 1;
                total_chars += r.text.chars().count();
                let size = r.point_size.unwrap_or(args.default_size);
                let (preview, glyph_count) = if let Some(face) = face.as_ref() {
                    let shaped = idml_text::shape_run(face, &r.text, size);
                    total_glyphs += shaped.glyphs.len();
                    (first_line(&r.text), shaped.glyphs.len())
                } else {
                    (first_line(&r.text), 0)
                };
                println!(
                    "  p{pi:02} r{ri:02}   {:>6.2}pt  glyphs={:>4}  {}",
                    size, glyph_count, preview
                );
            }

            // Per-paragraph composition report (if font + column given).
            if let (Some(face), Some(col_pt)) = (face.as_ref(), args.column_width_pt) {
                let measurer = idml_text::RustybuzzMeasurer::new(face, paragraph_size);
                let opts = idml_text::ComposeOptions::new(col_pt);
                let lines = idml_text::compose_paragraph(&paragraph_text, &measurer, &opts);
                total_lines += lines.len();
                println!(
                    "  p{pi:02}        composed lines={:<4} (column {:.2} pt)",
                    lines.len(),
                    col_pt
                );
            }
        }
    }

    println!("\ntotals");
    println!(
        "  paragraphs={paragraph}  runs={run}  chars={ch}  glyphs={gl}  lines={ln}",
        paragraph = total_paragraphs,
        run = total_runs,
        ch = total_chars,
        gl = total_glyphs,
        ln = total_lines,
    );
    if face.is_none() {
        println!("  (pass --font <path> to shape runs)");
    } else if args.column_width_pt.is_none() {
        println!("  (pass --column-width-pt <n> to compose paragraphs into lines)");
    }
    Ok(())
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
