//! End-to-end pipeline: `Document` → `DisplayList` → `RgbaImage`.
//!
//! Everything the inspect binary does minus the pretty-printing. This
//! is the thin, reusable top-level Rust API that hosts (WASM binding,
//! native tools, the fidelity harness) call into.
//!
//! The pipeline consumes `&idml_scene::Document` — parsing and resource
//! walking live in that crate so we stay focused on layout + emission.

use idml_compose::{
    emit_paragraph, emit_rect, emit_stroke_rect, Color, DisplayList, Paint, Rect, Stroke,
    TtfOutliner,
};
use idml_parse::{graphic, Graphic, Rectangle, TextFrame};
use idml_scene::Document;

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
    /// CMYK ICC profile bytes. When present (and on a target with
    /// lcms2 available — i.e. not wasm32), CMYK swatches are routed
    /// through ICC instead of the naive math in `idml-parse::graphic`.
    /// None → naive conversion (existing behaviour).
    pub cmyk_icc_profile: Option<&'a [u8]>,
}

impl Default for PipelineOptions<'_> {
    fn default() -> Self {
        Self {
            font: None,
            default_point_size: 12.0,
            fallback_column_width_pt: None,
            fallback_frame_fill: Paint::Solid(Color::rgba(0.92, 0.92, 0.92, 1.0)),
            fallback_text_paint: Paint::Solid(Color::BLACK),
            cmyk_icc_profile: None,
        }
    }
}

/// Page bounding box and display-list built from a `Document`.
#[derive(Debug)]
pub struct BuiltPage {
    pub width_pt: f32,
    pub height_pt: f32,
    /// Page origin in spread coordinates (top-left). The display list's
    /// commands are page-relative — the rasterizer treats (0, 0) as
    /// the page's top-left corner regardless of where the page sits in
    /// its parent spread.
    pub spread_origin: (f32, f32),
    pub list: DisplayList,
    /// Aggregated counts, useful for logging / CI reporting.
    pub stats: PipelineStats,
}

/// Multi-page render output. Each entry is a fully populated
/// `BuiltPage` with its own DisplayList and dimensions.
#[derive(Debug)]
pub struct BuiltDocument {
    pub pages: Vec<BuiltPage>,
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

/// Build one `BuiltPage` per `<Page>` in the document. Each page's
/// display list contains only frames whose centres fall inside the
/// page's `GeometricBounds`. Frames placed entirely on the pasteboard
/// (rare) land on the first page so they don't disappear silently.
///
/// Returns a `BuiltDocument` with aggregated stats. Use `build` for
/// the historical single-page (union of all bounds) shape.
pub fn build_document(
    document: &Document,
    options: &PipelineOptions,
) -> anyhow::Result<BuiltDocument> {
    let palette = &document.palette;
    // Build the CMYK ICC transform once per render. Failures are
    // logged + swallowed: if the profile is malformed we silently
    // fall back to naive math so the render still produces output.
    let cmyk_xform = options.cmyk_icc_profile.and_then(|bytes| {
        match idml_color::IccTransform::cmyk_to_linear_rgb(bytes) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %e, "failed to build CMYK ICC transform; using naive conversion");
                None
            }
        }
    });
    let mut pages: Vec<BuiltPage> = Vec::new();
    let mut total_stats = PipelineStats::default();

    // Walk every page in every spread. We capture each page's bounds,
    // origin, and applied-master reference so the next passes can
    // route frames by containment and apply master backgrounds.
    let mut page_geometries: Vec<PageGeom> = Vec::new();
    for parsed in &document.spreads {
        total_stats.spreads += 1;
        for p in &parsed.spread.pages {
            let geom = PageGeom {
                bounds_in_spread: p.bounds,
                applied_master: p.applied_master.clone(),
            };
            page_geometries.push(geom);
            pages.push(BuiltPage {
                width_pt: p.bounds.width(),
                height_pt: p.bounds.height(),
                spread_origin: (p.bounds.left, p.bounds.top),
                list: DisplayList::new(),
                stats: PipelineStats::default(),
            });
        }
    }
    total_stats.pages = pages.len();
    if pages.is_empty() {
        // Documents without a page (rare but valid) get a single
        // letter-sized canvas so callers always see a renderable output.
        pages.push(BuiltPage {
            width_pt: 612.0,
            height_pt: 792.0,
            spread_origin: (0.0, 0.0),
            list: DisplayList::new(),
            stats: PipelineStats::default(),
        });
        page_geometries.push(PageGeom {
            bounds_in_spread: idml_parse::Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 792.0,
                right: 612.0,
            },
            applied_master: None,
        });
    }

    // Master-spread pass — runs first so master items end up at the
    // bottom of each page's display list (page-level frames overlay on
    // top). Master frames are stamped into every page that references
    // the master.
    for (i, geom) in page_geometries.iter().enumerate() {
        let Some(master_ref) = geom.applied_master.as_deref() else {
            continue;
        };
        let Some(master) = document.master_spread(master_ref) else {
            continue;
        };
        // Master items are positioned in the master-spread coordinate
        // system; map them onto the live page by translating from the
        // master's first page origin to the live page origin. For the
        // common single-page master this is a straight passthrough.
        let master_origin = master
            .spread
            .pages
            .first()
            .map(|p| (p.bounds.left, p.bounds.top))
            .unwrap_or((0.0, 0.0));
        let target_origin = pages[i].spread_origin;
        let dx = target_origin.0 - master_origin.0;
        let dy = target_origin.1 - master_origin.1;
        for frame in &master.spread.text_frames {
            let translated = idml_parse::Bounds {
                top: frame.bounds.top + dy,
                left: frame.bounds.left + dx,
                bottom: frame.bounds.bottom + dy,
                right: frame.bounds.right + dx,
            };
            let mut copy = frame.clone();
            copy.bounds = translated;
            emit_text_frame_into(
                &mut pages[i],
                &copy,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
        for rect in &master.spread.rectangles {
            let translated = idml_parse::Bounds {
                top: rect.bounds.top + dy,
                left: rect.bounds.left + dx,
                bottom: rect.bounds.bottom + dy,
                right: rect.bounds.right + dx,
            };
            let mut copy = rect.clone();
            copy.bounds = translated;
            emit_rectangle_into(
                &mut pages[i],
                &copy,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
    }

    // Frame pass: route every frame to the page whose bounds contain
    // its centre. Falls back to page 0 for outliers.
    for parsed in &document.spreads {
        let spread = &parsed.spread;
        for frame in &spread.text_frames {
            total_stats.frames += 1;
            let page_idx = page_for_frame(&frame.bounds, &page_geometries).unwrap_or(0);
            emit_text_frame_into(
                &mut pages[page_idx],
                frame,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
        for rect in &spread.rectangles {
            total_stats.frames += 1;
            let bounds = rect.bounds;
            let page_idx = page_for_frame(&bounds, &page_geometries).unwrap_or(0);
            emit_rectangle_into(
                &mut pages[page_idx],
                rect,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
    }

    // Story pass: layout text into its hosting frame's page.
    let shaping_face = options
        .font
        .and_then(|bytes| rustybuzz::Face::from_slice(bytes, 0));
    let outline_face = options
        .font
        .and_then(|bytes| ttf_parser::Face::parse(bytes, 0).ok());
    let font_id = options.font.map(fnv_1a_u32).unwrap_or(0);

    for parsed in &document.stories {
        total_stats.stories += 1;
        let story = &parsed.story;
        let frame = document.frame_for(&parsed.self_id);
        let page_idx = frame
            .map(|f| page_for_frame(&f.bounds, &page_geometries).unwrap_or(0))
            .unwrap_or(0);
        let column_width_pt = options
            .fallback_column_width_pt
            .or_else(|| frame.map(|f| f.bounds.width()));

        for paragraph in &story.paragraphs {
            total_stats.paragraphs += 1;
            total_stats.runs += paragraph.runs.len();
            pages[page_idx].stats.paragraphs += 1;
            pages[page_idx].stats.runs += paragraph.runs.len();

            let paragraph_size = paragraph
                .runs
                .first()
                .and_then(|r| r.point_size)
                .unwrap_or(options.default_point_size);
            let paragraph_text: String = paragraph.runs.iter().map(|r| r.text.as_str()).collect();

            if let Some(face) = shaping_face.as_ref() {
                for run in &paragraph.runs {
                    let size = run.point_size.unwrap_or(options.default_point_size);
                    let shaped = idml_text::shape_run(face, &run.text, size);
                    total_stats.glyphs += shaped.glyphs.len();
                    pages[page_idx].stats.glyphs += shaped.glyphs.len();
                }
            }

            let (Some(face), Some(col_pt)) = (shaping_face.as_ref(), column_width_pt) else {
                continue;
            };
            let measurer = idml_text::RustybuzzMeasurer::new(face, paragraph_size);
            let mut lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
            lopts.alignment = map_justification(paragraph.justification.as_deref());
            let laid_out = idml_text::layout_paragraph(&paragraph_text, &measurer, &lopts);
            total_stats.lines += laid_out.lines.len();
            pages[page_idx].stats.lines += laid_out.lines.len();

            let (Some(outline), Some(frame)) = (outline_face.as_ref(), frame) else {
                continue;
            };
            let outliner = TtfOutliner::new(outline);
            let picker = build_run_paint_picker(paragraph, palette, options.fallback_text_paint);
            // Translate frame origin into the page's local coordinate
            // system: page-relative = spread-relative - page_origin.
            let (ox, oy) = pages[page_idx].spread_origin;
            emit_paragraph(
                &laid_out,
                font_id,
                paragraph_size,
                |cluster| picker.pick(cluster),
                (frame.bounds.left - ox, frame.bounds.top - oy),
                &outliner,
                &mut pages[page_idx].list,
            );
        }
    }

    Ok(BuiltDocument {
        pages,
        stats: total_stats,
    })
}

/// Wraps a page's bounds for centre-point routing + its master
/// reference for master-spread application.
struct PageGeom {
    bounds_in_spread: idml_parse::Bounds,
    applied_master: Option<String>,
}

fn page_for_frame(frame: &idml_parse::Bounds, pages: &[PageGeom]) -> Option<usize> {
    let cx = (frame.left + frame.right) * 0.5;
    let cy = (frame.top + frame.bottom) * 0.5;
    pages.iter().position(|p| {
        let b = p.bounds_in_spread;
        cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom
    })
}

fn emit_text_frame_into(
    page: &mut BuiltPage,
    frame: &TextFrame,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let (ox, oy) = page.spread_origin;
    let r = Rect {
        x: frame.bounds.left - ox,
        y: frame.bounds.top - oy,
        w: frame.bounds.width(),
        h: frame.bounds.height(),
    };
    let fill = frame
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
        .unwrap_or(fallback);
    emit_rect(r, fill, &mut page.list);
    if let Some(stroke) = frame
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
    {
        let width = frame.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_rect(r, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

fn emit_rectangle_into(
    page: &mut BuiltPage,
    rect: &Rectangle,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let (ox, oy) = page.spread_origin;
    let r = Rect {
        x: rect.bounds.left - ox,
        y: rect.bounds.top - oy,
        w: rect.bounds.width(),
        h: rect.bounds.height(),
    };
    let fill = rect
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
        .unwrap_or(fallback);
    emit_rect(r, fill, &mut page.list);
    if let Some(stroke) = rect
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
    {
        let width = rect.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_rect(r, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

/// Single-page convenience: union every page's bounds and emit all
/// frames in spread coordinates. Kept for back-compat and for hosts
/// that genuinely want one canvas — but multi-page callers should use
/// `build_document` instead.
pub fn build(document: &Document, options: &PipelineOptions) -> anyhow::Result<BuiltPage> {
    let palette = &document.palette;
    let mut stats = PipelineStats::default();
    let mut list = DisplayList::new();

    // Page bounding box — union across every page the document has.
    let mut page_w: f32 = 612.0;
    let mut page_h: f32 = 792.0;
    let mut saw_page = false;

    for parsed in &document.spreads {
        let spread = &parsed.spread;
        stats.spreads += 1;
        stats.pages += spread.pages.len();
        stats.frames += spread.text_frames.len() + spread.rectangles.len();

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

        for frame in &spread.text_frames {
            let rect = Rect {
                x: frame.bounds.left,
                y: frame.bounds.top,
                w: frame.bounds.width(),
                h: frame.bounds.height(),
            };
            let fill_paint = resolve_fill(frame, palette).unwrap_or(options.fallback_frame_fill);
            emit_rect(rect, fill_paint, &mut list);
            if let Some(stroke_paint) = resolve_stroke(frame, palette) {
                let width = frame.stroke_weight.unwrap_or(1.0);
                if width > 0.0 {
                    emit_stroke_rect(rect, Stroke::new(width), stroke_paint, &mut list);
                }
            }
        }

        for rect in &spread.rectangles {
            let r = Rect {
                x: rect.bounds.left,
                y: rect.bounds.top,
                w: rect.bounds.width(),
                h: rect.bounds.height(),
            };
            let fill = resolve_rect_fill(rect, palette).unwrap_or(options.fallback_frame_fill);
            emit_rect(r, fill, &mut list);
            if let Some(paint) = resolve_rect_stroke(rect, palette) {
                let width = rect.stroke_weight.unwrap_or(1.0);
                if width > 0.0 {
                    emit_stroke_rect(r, Stroke::new(width), paint, &mut list);
                }
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

    for parsed in &document.stories {
        stats.stories += 1;
        let story = &parsed.story;
        let frame = document.frame_for(&parsed.self_id);
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

            if let Some(face) = shaping_face.as_ref() {
                for run in &paragraph.runs {
                    let size = run.point_size.unwrap_or(options.default_point_size);
                    let shaped = idml_text::shape_run(face, &run.text, size);
                    stats.glyphs += shaped.glyphs.len();
                }
            }

            let (Some(face), Some(col_pt)) = (shaping_face.as_ref(), column_width_pt) else {
                continue;
            };
            let measurer = idml_text::RustybuzzMeasurer::new(face, paragraph_size);
            let mut lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
            lopts.alignment = map_justification(paragraph.justification.as_deref());
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
        spread_origin: (0.0, 0.0),
        list,
        stats,
    })
}

/// Build + rasterise every page. Returns one `RgbaImage` per page in
/// document order. `dpi` and `background` apply uniformly.
#[cfg(feature = "cpu")]
pub fn render_document(
    document: &Document,
    options: &PipelineOptions,
    dpi: f32,
    background: Color,
) -> anyhow::Result<(BuiltDocument, Vec<image::RgbaImage>)> {
    let built = build_document(document, options)?;
    let mut images = Vec::with_capacity(built.pages.len());
    for page in &built.pages {
        let mut raster_opts = idml_gpu::RasterOptions::new(page.width_pt, page.height_pt);
        raster_opts.dpi = dpi;
        raster_opts.background = background;
        images.push(idml_gpu::rasterize(&page.list, &raster_opts));
    }
    Ok((built, images))
}

/// Build + rasterise in one call. `dpi` and `background` control the
/// raster pass; everything else comes from `options`.
#[cfg(feature = "cpu")]
pub fn render(
    document: &Document,
    options: &PipelineOptions,
    dpi: f32,
    background: Color,
) -> anyhow::Result<(BuiltPage, image::RgbaImage)> {
    let built = build(document, options)?;
    let mut raster_opts = idml_gpu::RasterOptions::new(built.width_pt, built.height_pt);
    raster_opts.dpi = dpi;
    raster_opts.background = background;
    let image = idml_gpu::rasterize(&built.list, &raster_opts);
    Ok((built, image))
}

/// Pick the paint for a frame from its FillColor attribute.
pub fn resolve_fill(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.fill_color.as_deref()?, palette, None)
}

/// Same, for StrokeColor.
pub fn resolve_stroke(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.stroke_color.as_deref()?, palette, None)
}

/// Rectangle flavour of `resolve_fill` (no ParentStory to consider).
pub fn resolve_rect_fill(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.fill_color.as_deref()?, palette, None)
}

/// Rectangle flavour of `resolve_stroke`.
pub fn resolve_rect_stroke(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.stroke_color.as_deref()?, palette, None)
}

/// Resolve a color id (e.g. "Color/Red" or "Swatch/Black") to a
/// `Paint` via the palette and an optional CMYK ICC transform.
///
/// On targets where lcms2 isn't available (wasm32) or when no profile
/// is supplied, falls back to the naive math in
/// `idml-parse::graphic::to_linear_rgb` — keeping the existing
/// behaviour for hosts that don't yet ship an ICC profile.
pub fn color_id_to_paint(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<Paint> {
    let entry = palette.resolve(id)?;
    if let (Some(xform), idml_parse::ColorSpace::Cmyk) = (cmyk_xform, entry.space) {
        if entry.value.len() == 4 {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let cmyk = idml_color::Cmyk {
                    c: entry.value[0],
                    m: entry.value[1],
                    y: entry.value[2],
                    k: entry.value[3],
                };
                let idml_color::LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(cmyk);
                return Some(Paint::Solid(Color::rgba(r, g, b, 1.0)));
            }
            // wasm32: xform is unconstructable so this branch is unreachable.
            #[cfg(target_arch = "wasm32")]
            {
                let _ = xform;
            }
        }
    }
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

/// Map IDML `Justification` attribute values to `idml_text::Alignment`.
/// Unknown or missing values fall back to `Left`.
pub fn map_justification(j: Option<&str>) -> idml_text::Alignment {
    match j {
        Some("RightAlign") | Some("RightJustified") => idml_text::Alignment::Right,
        Some("CenterAlign") | Some("CenterJustified") => idml_text::Alignment::Center,
        Some("FullyJustified") | Some("LeftJustified") => idml_text::Alignment::Justify,
        _ => idml_text::Alignment::Left,
    }
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
