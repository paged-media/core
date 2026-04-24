//! End-to-end pipeline: `Container` → `DisplayList` → `RgbaImage`.
//!
//! Everything the inspect binary does minus the pretty-printing. This
//! is the thin, reusable top-level Rust API that hosts (WASM binding,
//! native tools, the fidelity harness) call into.

use std::collections::HashMap;

use idml_compose::{
    emit_paragraph, emit_rect, emit_stroke_rect, Color, DisplayList, Paint, Rect, Stroke,
    TtfOutliner,
};
use idml_parse::{graphic, Container, Graphic, Spread, Story, TextFrame};

/// Knobs the caller tunes when driving the full pipeline.
#[derive(Debug, Clone)]
pub struct PipelineOptions<'a> {
    /// Font bytes used for both shaping (`rustybuzz`) and glyph
    /// outlining (`ttf-parser`). `None` → text is skipped.
    pub font: Option<&'a [u8]>,
    /// Fallback point size for runs with no `PointSize` attribute.
    pub default_point_size: f32,
    /// Fallback column width in pt when a paragraph has no frame
    /// (extremely rare).
    pub fallback_column_width_pt: Option<f32>,
    /// Fill paint for frames that have no resolvable FillColor.
    pub fallback_frame_fill: Paint,
    /// Fill paint for runs that have no resolvable FillColor.
    pub fallback_text_paint: Paint,
}

impl Default for PipelineOptions<'_> {
    fn default() -> Self {
        Self {
            font: None,
            default_point_size: 12.0,
            fallback_column_width_pt: None,
            fallback_frame_fill: Paint::Solid(Color::rgba(0.92, 0.92, 0.92, 1.0)),
            fallback_text_paint: Paint::Solid(Color::BLACK),
        }
    }
}

/// Page bounding box and display-list built from a `Container`.
#[derive(Debug)]
pub struct BuiltPage {
    pub width_pt: f32,
    pub height_pt: f32,
    pub list: DisplayList,
    /// Aggregated counts, useful for logging / CI reporting.
    pub stats: PipelineStats,
}

#[derive(Debug, Default, Clone)]
pub struct PipelineStats {
    pub spreads: usize,
    pub pages: usize,
    pub frames: usize,
    pub stories: usize,
    pub paragraphs: usize,
    pub runs: usize,
    pub glyphs: usize,
    pub lines: usize,
}

/// Flatten a parsed `Container` into one `BuiltPage`. Today this unions
/// every page's bounding box into a single canvas — multi-page output
/// lands once the scene-graph crate exposes per-page iteration.
pub fn build(
    container: &Container,
    palette: &Graphic,
    options: &PipelineOptions,
) -> anyhow::Result<BuiltPage> {
    let mut stats = PipelineStats::default();
    let mut list = DisplayList::new();

    // Page bounding box — union across every page the manifest lists.
    let mut page_w: f32 = 612.0;
    let mut page_h: f32 = 792.0;
    let mut saw_page = false;

    // Index TextFrames by ParentStory so the story pass below can pick
    // a frame without re-parsing.
    let mut frame_for_story: HashMap<String, TextFrame> = HashMap::new();

    for spread_ref in &container.designmap.spreads {
        let Some(raw) = container.entry(&spread_ref.src) else {
            continue;
        };
        let spread = Spread::parse(raw)?;
        stats.spreads += 1;
        stats.pages += spread.pages.len();
        stats.frames += spread.text_frames.len();

        for p in &spread.pages {
            if saw_page {
                page_w = page_w.max(p.bounds.width());
                page_h = page_h.max(p.bounds.height());
            } else {
                page_w = p.bounds.width();
                page_h = p.bounds.height();
                saw_page = true;
            }
        }

        for frame in spread.text_frames {
            let rect = Rect {
                x: frame.bounds.left,
                y: frame.bounds.top,
                w: frame.bounds.width(),
                h: frame.bounds.height(),
            };
            let fill_paint = resolve_fill(&frame, palette).unwrap_or(options.fallback_frame_fill);
            emit_rect(rect, fill_paint, &mut list);

            if let Some(stroke_paint) = resolve_stroke(&frame, palette) {
                let width = frame.stroke_weight.unwrap_or(1.0);
                if width > 0.0 {
                    emit_stroke_rect(rect, Stroke::new(width), stroke_paint, &mut list);
                }
            }

            if let Some(story_id) = frame.parent_story.clone() {
                frame_for_story.insert(story_id, frame);
            }
        }
    }

    let shaping_face = options
        .font
        .and_then(|bytes| rustybuzz::Face::from_slice(bytes, 0));
    let outline_face = options
        .font
        .and_then(|bytes| ttf_parser::Face::parse(bytes, 0).ok());
    let font_id = options.font.map(fnv_1a_u32).unwrap_or(0);

    for story_ref in &container.designmap.stories {
        let Some(raw) = container.entry(&story_ref.src) else {
            continue;
        };
        let story = Story::parse(raw)?;
        stats.stories += 1;

        let story_id = derive_story_id(&story_ref.src);
        let frame = story_id.as_ref().and_then(|id| frame_for_story.get(id));
        let column_width_pt = options
            .fallback_column_width_pt
            .or_else(|| frame.map(|f| f.bounds.width()));

        for paragraph in &story.paragraphs {
            stats.paragraphs += 1;
            stats.runs += paragraph.runs.len();

            let paragraph_size = paragraph
                .runs
                .first()
                .and_then(|r| r.point_size)
                .unwrap_or(options.default_point_size);
            let paragraph_text: String = paragraph.runs.iter().map(|r| r.text.as_str()).collect();

            // Per-run glyph count — useful for logging even when we
            // don't compose (no font, no frame).
            if let Some(face) = shaping_face.as_ref() {
                for run in &paragraph.runs {
                    let size = run.point_size.unwrap_or(options.default_point_size);
                    let shaped = idml_text::shape_run(face, &run.text, size);
                    stats.glyphs += shaped.glyphs.len();
                }
            }

            // Layout + emit requires both a shaping face and a column
            // width. Outlining is a third requirement for actually
            // producing FillPath commands.
            let (Some(face), Some(col_pt)) = (shaping_face.as_ref(), column_width_pt) else {
                continue;
            };
            let measurer = idml_text::RustybuzzMeasurer::new(face, paragraph_size);
            let lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
            let laid_out = idml_text::layout_paragraph(&paragraph_text, &measurer, &lopts);
            stats.lines += laid_out.lines.len();

            let (Some(outline), Some(frame)) = (outline_face.as_ref(), frame) else {
                continue;
            };
            let outliner = TtfOutliner::new(outline);
            let picker = build_run_paint_picker(paragraph, palette, options.fallback_text_paint);
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

    Ok(BuiltPage {
        width_pt: page_w,
        height_pt: page_h,
        list,
        stats,
    })
}

/// Build + rasterise in one call. `dpi` and `background` control the
/// raster pass; everything else comes from `options`.
#[cfg(feature = "cpu")]
pub fn render(
    container: &Container,
    palette: &Graphic,
    options: &PipelineOptions,
    dpi: f32,
    background: Color,
) -> anyhow::Result<(BuiltPage, image::RgbaImage)> {
    let built = build(container, palette, options)?;
    let mut raster_opts = idml_gpu::RasterOptions::new(built.width_pt, built.height_pt);
    raster_opts.dpi = dpi;
    raster_opts.background = background;
    let image = idml_gpu::rasterize(&built.list, &raster_opts);
    Ok((built, image))
}

/// Pick the paint for a frame from its FillColor attribute.
pub fn resolve_fill(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    let id = frame.fill_color.as_deref()?;
    let entry = palette.resolve(id)?;
    let [r, g, b] = graphic::to_linear_rgb(entry)?;
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// Same, for StrokeColor.
pub fn resolve_stroke(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    let id = frame.stroke_color.as_deref()?;
    let entry = palette.resolve(id)?;
    let [r, g, b] = graphic::to_linear_rgb(entry)?;
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// Cluster → Paint picker built from a paragraph's run table.
pub struct RunPaintPicker {
    bands: Vec<(u32, Paint)>,
    default: Paint,
}

impl RunPaintPicker {
    pub fn pick(&self, cluster: u32) -> Paint {
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

pub fn build_run_paint_picker(
    paragraph: &idml_parse::Paragraph,
    palette: &Graphic,
    default: Paint,
) -> RunPaintPicker {
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

/// Stories are manifested by src filename ("Stories/Story_uXX.xml");
/// the frame references the story's Self id ("uXX"). This pulls the id
/// back out so frames can be matched to stories without a second parse.
pub fn derive_story_id(src: &str) -> Option<String> {
    let stem = src.rsplit_once('/').map(|(_, t)| t).unwrap_or(src);
    let without_ext = stem.strip_suffix(".xml").unwrap_or(stem);
    without_ext
        .strip_prefix("Story_")
        .map(|s| s.to_string())
        .or_else(|| Some(without_ext.to_string()))
}

fn fnv_1a_u32(bytes: &[u8]) -> u32 {
    // Stable per-render font-cache key; the u32 range collides in
    // ~2B fonts — enough for any realistic document.
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}
