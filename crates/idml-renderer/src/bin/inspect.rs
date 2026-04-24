//! `idml-inspect`: open an IDML, parse its manifest, spreads, and
//! stories, then print a human-readable summary. Exercises parse +
//! spread + story + shape + compose end-to-end against real IDML
//! bytes.
//!
//! With `--font <path>`, every run is shaped via rustybuzz. When a
//! paragraph belongs to a TextFrame, the frame's inner width is used
//! as the composer's column width automatically, so line counts match
//! the document's layout intent.
//!
//! With `--display-list`, also builds the page's DisplayList by
//! emitting one `FillPath` command per frame background and (when a
//! font is available) one `FillPath` per glyph. Command and path
//! counts are reported at the end.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use idml_compose::{emit_paragraph, emit_rect, Color, DisplayList, Paint, Rect, TtfOutliner};
use idml_parse::{graphic, Container, Graphic, Spread, Story};

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
    /// Mainly useful when the IDML has no frames (rare) or when you
    /// want to experiment with a different column width.
    #[arg(long)]
    column_width_pt: Option<f32>,
    /// Build the DisplayList (frame backgrounds + positioned glyphs)
    /// and report command / path counts.
    #[arg(long)]
    display_list: bool,
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
    let shaping_face = font_bytes
        .as_deref()
        .and_then(|b| rustybuzz::Face::from_slice(b, 0));
    let outline_face = font_bytes
        .as_deref()
        .and_then(|b| ttf_parser::Face::parse(b, 0).ok());

    // Parse the document's swatch palette if present. Missing is OK —
    // not every IDML ships Resources/Graphic.xml.
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

    // Parse every Spread the manifest references, and index TextFrames
    // by their ParentStory so the story-walk below can fetch each
    // paragraph's column width without a second pass.
    let mut frame_for_story: HashMap<String, idml_parse::TextFrame> = HashMap::new();
    // Accumulates the scene's display list as we walk spreads + stories.
    let mut list = DisplayList::new();
    // Fallback fill when a frame has no FillColor (or it doesn't resolve).
    let placeholder_fill = Paint::Solid(Color::rgba(0.92, 0.92, 0.92, 1.0));

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
        for frame in spread.text_frames {
            let fill_paint = resolve_fill(&frame, &palette).unwrap_or(placeholder_fill);
            println!(
                "  frame       {} → story {}   {:>6.2} × {:<6.2} pt  fill={}",
                frame.self_id.as_deref().unwrap_or("?"),
                frame.parent_story.as_deref().unwrap_or("(none)"),
                frame.bounds.width(),
                frame.bounds.height(),
                describe_fill(&frame, &palette),
            );
            if args.display_list {
                emit_rect(
                    Rect {
                        x: frame.bounds.left,
                        y: frame.bounds.top,
                        w: frame.bounds.width(),
                        h: frame.bounds.height(),
                    },
                    fill_paint,
                    &mut list,
                );
            }
            if let Some(story_id) = frame.parent_story.clone() {
                frame_for_story.insert(story_id, frame);
            }
        }
    }

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
        let story_id = derive_story_id(&story_ref.src);
        let frame = story_id.as_ref().and_then(|id| frame_for_story.get(id));
        let column_width_pt = args
            .column_width_pt
            .or_else(|| frame.map(|f| f.bounds.width()));

        println!(
            "\nstory         {}  ({} paragraph(s){})",
            story_ref.src,
            story.paragraphs.len(),
            column_width_pt
                .map(|w| format!(", column {w:.2} pt"))
                .unwrap_or_default(),
        );
        for (pi, p) in story.paragraphs.iter().enumerate() {
            total_paragraphs += 1;
            let paragraph_size = p
                .runs
                .first()
                .and_then(|r| r.point_size)
                .unwrap_or(args.default_size);
            let paragraph_text: String = p.runs.iter().map(|r| r.text.as_str()).collect();

            for (ri, r) in p.runs.iter().enumerate() {
                total_runs += 1;
                total_chars += r.text.chars().count();
                let size = r.point_size.unwrap_or(args.default_size);
                let (preview, glyph_count) = if let Some(face) = shaping_face.as_ref() {
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

            if let (Some(face), Some(col_pt)) = (shaping_face.as_ref(), column_width_pt) {
                let measurer = idml_text::RustybuzzMeasurer::new(face, paragraph_size);
                let lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
                let laid_out = idml_text::layout_paragraph(&paragraph_text, &measurer, &lopts);
                total_lines += laid_out.lines.len();
                println!(
                    "  p{pi:02}        composed lines={:<4} (column {:.2} pt)",
                    laid_out.lines.len(),
                    col_pt
                );
                if args.display_list {
                    if let (Some(outline), Some(frame)) = (outline_face.as_ref(), frame) {
                        let outliner = TtfOutliner::new(outline);
                        let font_id = font_bytes.as_deref().map(fnv_1a_u32).unwrap_or(0);
                        // Build a cluster → Paint picker from the run
                        // table: walk the paragraph text, track the
                        // byte-offset where each run starts, and pick
                        // based on which run's range contains the
                        // cluster. Falls back to black when a run's
                        // FillColor is absent or unresolved.
                        let picker = run_paint_picker(p, &palette);
                        emit_paragraph(
                            &laid_out,
                            font_id,
                            paragraph_size,
                            |cluster| picker.pick(cluster),
                            (frame.bounds.left, frame.bounds.top),
                            &outliner,
                            &mut list,
                        );
                    }
                }
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
    if args.display_list {
        println!(
            "  display-list: {} command(s), {} unique path(s)",
            list.commands.len(),
            list.paths.len(),
        );
    }
    if shaping_face.is_none() {
        println!("  (pass --font <path> to shape + compose runs)");
    }
    Ok(())
}

/// Pick the paint for a frame: look up FillColor in the palette,
/// convert to linear RGB, drop alpha at 1.0. None means "fall back".
fn resolve_fill(frame: &idml_parse::TextFrame, palette: &Graphic) -> Option<Paint> {
    let id = frame.fill_color.as_deref()?;
    let entry = palette.resolve(id)?;
    let [r, g, b] = graphic::to_linear_rgb(entry)?;
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// Cluster → Paint picker built from a paragraph's run table. The
/// paragraph text is the concatenation of each run's `text`; this
/// struct remembers the (start_byte, paint) pair for each run.
struct RunPaintPicker {
    // Sorted ascending by `start`. `start` is the byte offset of the
    // first character of the run in the concatenated paragraph text.
    bands: Vec<(u32, Paint)>,
    default: Paint,
}

impl RunPaintPicker {
    fn pick(&self, cluster: u32) -> Paint {
        // Linear search is fine: paragraphs rarely have more than a
        // dozen runs. Find the last band whose start <= cluster.
        let mut chosen = self.default;
        for (start, paint) in &self.bands {
            if *start <= cluster {
                chosen = *paint;
            } else {
                break;
            }
        }
        chosen
    }
}

fn run_paint_picker(paragraph: &idml_parse::Paragraph, palette: &Graphic) -> RunPaintPicker {
    let default = Paint::Solid(Color::BLACK);
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len());
    let mut cursor: u32 = 0;
    for run in &paragraph.runs {
        let paint = run
            .fill_color
            .as_deref()
            .and_then(|id| palette.resolve(id))
            .and_then(graphic::to_linear_rgb)
            .map(|[r, g, b]| Paint::Solid(Color::rgba(r, g, b, 1.0)))
            .unwrap_or(default);
        bands.push((cursor, paint));
        cursor += run.text.len() as u32;
    }
    RunPaintPicker { bands, default }
}

/// One-line fill description for the human-readable report.
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

fn derive_story_id(src: &str) -> Option<String> {
    let stem = src.rsplit_once('/').map(|(_, t)| t).unwrap_or(src);
    let without_ext = stem.strip_suffix(".xml").unwrap_or(stem);
    without_ext
        .strip_prefix("Story_")
        .map(|s| s.to_string())
        .or_else(|| Some(without_ext.to_string()))
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

fn fnv_1a_u32(bytes: &[u8]) -> u32 {
    // 32-bit FNV-1a — cheap, non-cryptographic; fine for a per-render
    // font-cache key.
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}
