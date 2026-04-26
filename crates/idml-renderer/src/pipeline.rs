//! End-to-end pipeline: `Document` → `DisplayList` → `RgbaImage`.
//!
//! Everything the inspect binary does minus the pretty-printing. This
//! is the thin, reusable top-level Rust API that hosts (WASM binding,
//! native tools, the fidelity harness) call into.
//!
//! The pipeline consumes `&idml_scene::Document` — parsing and resource
//! walking live in that crate so we stay focused on layout + emission.

use std::collections::HashMap;

use bytes::Bytes;
use idml_compose::{
    emit_drop_shadow_rect_transformed, emit_ellipse_transformed, emit_glyph_slice, emit_line,
    emit_paragraph, emit_rect, emit_rect_transformed, emit_stroke_ellipse_transformed,
    emit_stroke_rect, emit_stroke_rect_transformed, Color, DisplayList, DropShadow, Paint, Rect,
    Stroke, Transform, TtfOutliner,
};
use idml_parse::{graphic, Graphic, GraphicLine, Oval, Rectangle, TextFrame};
use idml_scene::Document;

use crate::AssetResolver;

/// Knobs the caller tunes when driving the full pipeline.
#[derive(Clone)]
pub struct PipelineOptions<'a> {
    /// Default font bytes. Used as a fallback for any paragraph
    /// whose `AppliedFont` doesn't resolve via `assets`. `None` plus
    /// no resolver hit → text is skipped.
    pub font: Option<&'a [u8]>,
    /// Asset resolver consulted per (family, style). When set, the
    /// pipeline pre-resolves every distinct font referenced in the
    /// document; runs without a hit fall back to `font`.
    pub assets: Option<&'a dyn AssetResolver>,
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
    /// Synthetic drop shadow applied to every TextFrame and
    /// Rectangle. Useful for tooling demos and as a stopgap until
    /// `<TransparencySetting>` parsing lands and per-frame effects
    /// flow from the IDML itself.
    pub frame_drop_shadow: Option<DropShadow>,
}

impl std::fmt::Debug for PipelineOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineOptions")
            .field("font", &self.font.map(|b| b.len()))
            .field("assets", &self.assets.is_some())
            .field("default_point_size", &self.default_point_size)
            .field("fallback_column_width_pt", &self.fallback_column_width_pt)
            .field("cmyk_icc_profile", &self.cmyk_icc_profile.map(|b| b.len()))
            .field("frame_drop_shadow", &self.frame_drop_shadow)
            .finish_non_exhaustive()
    }
}

impl Default for PipelineOptions<'_> {
    fn default() -> Self {
        Self {
            font: None,
            assets: None,
            default_point_size: 12.0,
            fallback_column_width_pt: None,
            fallback_frame_fill: Paint::Solid(Color::rgba(0.92, 0.92, 0.92, 1.0)),
            fallback_text_paint: Paint::Solid(Color::BLACK),
            cmyk_icc_profile: None,
            frame_drop_shadow: None,
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
    //
    // `spread_page_ranges[i]` is the half-open page-index range
    // owned by `document.spreads[i]`; frames within a spread route
    // only to that range, since each IDML spread has its own
    // coordinate system and two spreads' page bounds can collide.
    let mut page_geometries: Vec<PageGeom> = Vec::new();
    let mut spread_page_ranges: Vec<std::ops::Range<usize>> =
        Vec::with_capacity(document.spreads.len());
    for parsed in &document.spreads {
        total_stats.spreads += 1;
        let start = pages.len();
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
        spread_page_ranges.push(start..pages.len());
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
            total_stats.frames += 1;
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
                None, // master items don't carry a drop shadow today.
            );
        }
        for rect in &master.spread.rectangles {
            total_stats.frames += 1;
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
                None,
            );
        }
    }

    // Frame pass: route every frame to the page whose bounds contain
    // Per-frame (by Self id) page lookup. The story pass builds
    // each story's frame chain via Document::frame_chain and uses
    // this map to find each chain entry's page so threaded stories
    // can route line emission across pages.
    let mut frame_to_page: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // Per-page (URI → ImageId) cache so multiple rectangles on the
    // same page sharing an image share a single ImageId in the
    // page's display list.
    let mut page_image_caches: Vec<HashMap<String, idml_compose::ImageId>> =
        (0..pages.len()).map(|_| HashMap::new()).collect();
    // Renderer-scoped (URI → DecodedImage) cache so an image
    // referenced from multiple pages is decoded once. The cached
    // DecodedImage is cloned into each page's image pool — the
    // memcpy is cheap; the saved decode (PNG/JPEG → RGBA) is not.
    let mut decoded_image_cache: HashMap<String, idml_compose::DecodedImage> = HashMap::new();
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let range = spread_page_ranges[spread_idx].clone();
        let local_geoms = &page_geometries[range.clone()];
        for frame in &spread.text_frames {
            total_stats.frames += 1;
            let local_idx = page_for_frame(&frame.bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            if let Some(self_id) = frame.self_id.clone() {
                frame_to_page.insert(self_id, page_idx);
            }
            emit_text_frame_into(
                &mut pages[page_idx],
                frame,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                options.frame_drop_shadow,
            );
        }
        for rect in &spread.rectangles {
            total_stats.frames += 1;
            let local_idx = page_for_frame(&rect.bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            emit_rectangle_into(
                &mut pages[page_idx],
                rect,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                options.frame_drop_shadow,
            );
            // The image draws on top of the rectangle's solid fill.
            // Per-page cache: shares ImageId across same-URI
            // rectangles on this page. Renderer-scoped cache:
            // shares the decoded RGBA across pages.
            emit_rectangle_image(
                &mut pages[page_idx],
                rect,
                options,
                &mut page_image_caches[page_idx],
                &mut decoded_image_cache,
            );
        }
        for oval in &spread.ovals {
            total_stats.frames += 1;
            let local_idx = page_for_frame(&oval.bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            emit_oval_into(
                &mut pages[page_idx],
                oval,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
        for line in &spread.graphic_lines {
            total_stats.frames += 1;
            let local_idx = page_for_frame(&line.bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            emit_line_into(&mut pages[page_idx], line, palette, cmyk_xform.as_ref());
        }
    }

    // Story pass: layout text into its hosting frame's page.
    //
    // The font table pre-resolves every distinct (family, style)
    // referenced anywhere in the document so each paragraph picks up
    // the right TTF without re-querying the resolver. Per paragraph
    // we still build `Face`s on demand — `rustybuzz::Face::from_slice`
    // is cheap (parses font tables, no allocation churn).
    let font_table = FontTable::build(document, options);

    for parsed in &document.stories {
        total_stats.stories += 1;
        let chain = document.frame_chain(&parsed.self_id);
        if chain.is_empty() {
            continue;
        }
        let chain_pages: Vec<usize> = chain
            .iter()
            .map(|f| {
                f.self_id
                    .as_deref()
                    .and_then(|id| frame_to_page.get(id).copied())
                    .unwrap_or(0)
            })
            .collect();
        let mut emitter = StoryEmitter::new(
            document,
            options,
            palette,
            cmyk_xform.as_ref(),
            &font_table,
            chain,
            chain_pages,
        );
        for paragraph in &parsed.story.paragraphs {
            emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
        }
        emitter.apply_vertical_justification(&mut pages);
    }

    Ok(BuiltDocument {
        pages,
        stats: total_stats,
    })
}

/// Emits a story's paragraphs into the page list, flowing across
/// the frame chain on overflow and applying TextFramePreference
/// vertical justification once the story finishes.
///
/// Carries all the per-story mutable state the build_document loop
/// previously held inline:
///  - frame_idx + y_cursor: which frame is currently filling and
///    where the next baseline goes inside it.
///  - frame_cmd_ranges + frame_max_baseline_64: tracked during
///    emission so the post-story vertical-justification shift can
///    target this story's commands without touching frame outlines.
struct StoryEmitter<'a> {
    document: &'a Document,
    options: &'a PipelineOptions<'a>,
    palette: &'a Graphic,
    /// Reserved for the upcoming CMYK text-fill path. The current
    /// per-glyph paint picker resolves through `palette` directly.
    #[allow(dead_code)]
    cmyk_xform: Option<&'a idml_color::IccTransform>,
    font_table: &'a FontTable,
    chain: Vec<&'a TextFrame>,
    chain_pages: Vec<usize>,
    column_width_pt: Option<f32>,
    frame_idx: usize,
    y_cursor: i32,
    frame_cmd_ranges: Vec<Option<(usize, usize)>>,
    frame_max_baseline_64: Vec<i32>,
}

impl<'a> StoryEmitter<'a> {
    fn new(
        document: &'a Document,
        options: &'a PipelineOptions<'a>,
        palette: &'a Graphic,
        cmyk_xform: Option<&'a idml_color::IccTransform>,
        font_table: &'a FontTable,
        chain: Vec<&'a TextFrame>,
        chain_pages: Vec<usize>,
    ) -> Self {
        // Head frame's L+R insets shrink the column width. Threaded
        // frames usually share the same insets; honouring per-frame
        // insets requires recomputing the column width when
        // crossing frame boundaries.
        let head_insets = chain[0].inset_spacing.unwrap_or([0.0; 4]);
        let column_width_pt = options.fallback_column_width_pt.or_else(|| {
            chain
                .first()
                .map(|f| (f.bounds.width() - head_insets[1] - head_insets[3]).max(0.0))
        });
        let len = chain.len();
        Self {
            document,
            options,
            palette,
            cmyk_xform,
            font_table,
            chain,
            chain_pages,
            column_width_pt,
            frame_idx: 0,
            y_cursor: -1,
            frame_cmd_ranges: vec![None; len],
            frame_max_baseline_64: vec![0; len],
        }
    }

    fn emit_paragraph(
        &mut self,
        paragraph: &idml_parse::Paragraph,
        pages: &mut [BuiltPage],
        total_stats: &mut PipelineStats,
    ) {
        emit_paragraph_into_chain(self, paragraph, pages, total_stats);
    }

    fn apply_vertical_justification(&self, pages: &mut [BuiltPage]) {
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            let vj = frame.vertical_justification.as_deref();
            if vj.is_none() || vj == Some("TopAlign") {
                continue;
            }
            let frame_height_64 =
                (frame.bounds.height() * idml_text::shape::ADVANCE_PRECISION).round() as i32;
            // Approximate used height = last baseline; a future
            // batch can fold in the descender of the last line.
            let used_64 = self.frame_max_baseline_64[i];
            let slack_64 = (frame_height_64 - used_64).max(0);
            let dy_64 = match vj {
                Some("CenterAlign") => slack_64 / 2,
                Some("BottomAlign") => slack_64,
                _ => 0,
            };
            if dy_64 == 0 {
                continue;
            }
            let dy_pt = dy_64 as f32 / idml_text::shape::ADVANCE_PRECISION;
            for cmd in &mut pages[self.chain_pages[i]].list.commands[start..end] {
                cmd.transform_mut().0[5] += dy_pt;
            }
        }
    }
}

/// Body of `StoryEmitter::emit_paragraph`. Lives as a free fn so
/// the long, branching layout/emit pipeline isn't visually
/// indented under `impl`. The free fn has full mutable access to
/// the emitter state via `&mut StoryEmitter`.
fn emit_paragraph_into_chain(
    em: &mut StoryEmitter,
    paragraph: &idml_parse::Paragraph,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) {
    total_stats.paragraphs += 1;
    total_stats.runs += paragraph.runs.len();
    pages[em.chain_pages[em.frame_idx]].stats.paragraphs += 1;
    pages[em.chain_pages[em.frame_idx]].stats.runs += paragraph.runs.len();

    let resolved_runs: Vec<idml_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);

    // Resolve every run's font bytes up front so the borrows for
    // `Face` construction below all live in the same scope.
    let mut bytes_pool: Vec<bytes::Bytes> = Vec::with_capacity(paragraph.runs.len());
    for resolved in &resolved_runs {
        let Some(b) = em
            .font_table
            .bytes_for(resolved.font.as_deref(), resolved.font_style.as_deref())
        else {
            continue;
        };
        bytes_pool.push(b);
    }
    if bytes_pool.is_empty() || bytes_pool.len() != paragraph.runs.len() {
        return;
    }

    let mut shaping_faces: Vec<rustybuzz::Face> = Vec::with_capacity(bytes_pool.len());
    let mut outline_faces: Vec<ttf_parser::Face> = Vec::with_capacity(bytes_pool.len());
    for b in &bytes_pool {
        let Some(rf) = rustybuzz::Face::from_slice(b.as_ref(), 0) else {
            continue;
        };
        let Ok(of) = ttf_parser::Face::parse(b.as_ref(), 0) else {
            continue;
        };
        shaping_faces.push(rf);
        outline_faces.push(of);
    }
    if shaping_faces.len() != paragraph.runs.len() {
        return;
    }

    let font_ids: Vec<u32> = bytes_pool.iter().map(|b| fnv_1a_u32(b.as_ref())).collect();

    let styled_runs: Vec<idml_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| idml_text::StyledRun {
            text: &run.text,
            face: &shaping_faces[i],
            point_size: resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size),
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
        })
        .collect();

    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let Some(col_pt) = em.column_width_pt else {
        return;
    };
    let mut lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification.as_deref());

    if em.y_cursor < 0 {
        let head_font_metrics = font_ids
            .first()
            .and_then(|id| em.font_table.metrics_for(*id));
        em.y_cursor = first_baseline_for_frame(
            em.chain[0],
            paragraph_size,
            lopts.first_baseline,
            head_font_metrics,
        );
    } else {
        let space_before_64 =
            resolved_paragraph.space_before.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
        em.y_cursor += space_before_64.round() as i32;
    }
    lopts.first_baseline = em.y_cursor;

    let mut laid_out = idml_text::layout_runs(&styled_runs, &lopts);

    // FirstLineIndent shifts the first line's glyphs after
    // breaking — Knuth-Plass can't model a per-line x-shift, so
    // it's a post-layout pass.
    if let Some(indent_pt) = resolved_paragraph.first_line_indent {
        let indent_64 = (indent_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32;
        if indent_64 != 0 {
            if let Some(line) = laid_out.lines.first_mut() {
                for g in &mut line.glyphs {
                    g.x += indent_64;
                }
            }
        }
    }

    let has_any_tab = paragraph.runs.iter().any(|r| r.text.contains('\t'));
    if has_any_tab {
        let tab_stops: Vec<(f32, idml_text::layout::TabAlignment)> = resolved_paragraph
            .tab_list
            .iter()
            .map(|t| (t.position, map_tab_alignment(t.alignment.as_deref())))
            .collect();
        let paragraph_text: String = paragraph.runs.iter().map(|r| r.text.as_str()).collect();
        for line in laid_out.lines.iter_mut() {
            idml_text::layout::apply_tab_stops(line, &paragraph_text, &tab_stops, 36.0);
        }
    }

    let picker = build_run_paint_picker_resolved(
        paragraph,
        &resolved_runs,
        em.palette,
        em.options.fallback_text_paint,
    );

    let space_after_64 =
        resolved_paragraph.space_after.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
    for mut line in laid_out.lines.into_iter() {
        let line_h = idml_text::layout::max_line_height_for_glyphs(&line.glyphs)
            .unwrap_or(lopts.line_height);
        let frame_height_64 = (em.chain[em.frame_idx].bounds.height()
            * idml_text::shape::ADVANCE_PRECISION)
            .round() as i32;
        if line.baseline_y > frame_height_64 && em.frame_idx + 1 < em.chain.len() {
            let prev_baseline = line.baseline_y;
            em.frame_idx += 1;
            let new_baseline =
                (paragraph_size * 0.8 * idml_text::shape::ADVANCE_PRECISION).round() as i32;
            let dy = new_baseline - prev_baseline;
            for g in &mut line.glyphs {
                g.y += dy;
            }
            line.baseline_y = new_baseline;
        }

        let target_page = em.chain_pages[em.frame_idx];
        pages[target_page].stats.glyphs += line.glyphs.len();
        pages[target_page].stats.lines += 1;
        total_stats.glyphs += line.glyphs.len();
        total_stats.lines += 1;

        let frame = em.chain[em.frame_idx];
        let (ox, oy) = pages[target_page].spread_origin;
        let frame_insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let text_origin_pt = (
            frame.bounds.left - ox + frame_insets[1],
            frame.bounds.top - oy,
        );

        let before_cmds = pages[target_page].list.commands.len();

        let mut start = 0;
        while start < line.glyphs.len() {
            let fid = line.glyphs[start].font_id;
            let mut end = start + 1;
            while end < line.glyphs.len() && line.glyphs[end].font_id == fid {
                end += 1;
            }
            let face_idx = match font_ids.iter().position(|f| *f == fid) {
                Some(i) => i,
                None => {
                    start = end;
                    continue;
                }
            };
            let outliner = TtfOutliner::new(&outline_faces[face_idx]);
            emit_glyph_slice(
                &line.glyphs[start..end],
                fid,
                line.glyphs[start].point_size,
                |cluster| picker.pick(cluster),
                text_origin_pt,
                &outliner,
                &mut pages[target_page].list,
            );
            start = end;
        }
        emit_line_decorations(
            &line,
            &picker,
            (frame.bounds.left - ox, frame.bounds.top - oy),
            &mut pages[target_page].list,
        );

        let after_cmds = pages[target_page].list.commands.len();
        let frame_idx = em.frame_idx;
        match &mut em.frame_cmd_ranges[frame_idx] {
            Some((_, e)) => *e = after_cmds,
            None => em.frame_cmd_ranges[frame_idx] = Some((before_cmds, after_cmds)),
        }
        if line.baseline_y > em.frame_max_baseline_64[frame_idx] {
            em.frame_max_baseline_64[frame_idx] = line.baseline_y;
        }

        em.y_cursor = line.baseline_y + line_h;
    }
    em.y_cursor += space_after_64.round() as i32;
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
    drop_shadow: Option<DropShadow>,
) {
    page.stats.frames += 1;
    let r = Rect {
        x: frame.bounds.left,
        y: frame.bounds.top,
        w: frame.bounds.width(),
        h: frame.bounds.height(),
    };
    let outer = frame_outer_transform(page, frame.item_transform);
    if let Some(shadow) =
        resolve_frame_shadow(frame.drop_shadow.as_ref(), drop_shadow, palette, cmyk_xform)
    {
        emit_drop_shadow_rect_transformed(r, outer, shadow, &mut page.list);
    }
    let fill = frame
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);
    emit_rect_transformed(r, outer, fill, &mut page.list);
    if let Some(stroke) = frame
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    {
        let width = frame.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_rect_transformed(r, outer, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

fn emit_oval_into(
    page: &mut BuiltPage,
    oval: &Oval,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let r = Rect {
        x: oval.bounds.left,
        y: oval.bounds.top,
        w: oval.bounds.width(),
        h: oval.bounds.height(),
    };
    let outer = frame_outer_transform(page, oval.item_transform);
    // Ovals don't yet have a dedicated shadow primitive — use the
    // bounding-rect stamp as a stopgap. Replace once the rasterizer
    // grows shadowed-ellipse support.
    if let Some(shadow) = resolve_frame_shadow(oval.drop_shadow.as_ref(), None, palette, cmyk_xform)
    {
        emit_drop_shadow_rect_transformed(r, outer, shadow, &mut page.list);
    }
    let fill = oval
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);
    emit_ellipse_transformed(r, outer, fill, &mut page.list);
    if let Some(stroke) = oval
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    {
        let width = oval.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_ellipse_transformed(r, outer, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

fn emit_line_into(
    page: &mut BuiltPage,
    line: &GraphicLine,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let Some(stroke_paint) = line
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    else {
        return;
    };
    let width = line.stroke_weight.unwrap_or(1.0);
    if width <= 0.0 {
        return;
    }
    let (ox, oy) = page.spread_origin;
    emit_line(
        line.bounds.left - ox,
        line.bounds.top - oy,
        line.bounds.right - ox,
        line.bounds.bottom - oy,
        Stroke::new(width),
        stroke_paint,
        &mut page.list,
    );
}

fn emit_rectangle_into(
    page: &mut BuiltPage,
    rect: &Rectangle,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
    drop_shadow: Option<DropShadow>,
) {
    page.stats.frames += 1;
    let r = Rect {
        x: rect.bounds.left,
        y: rect.bounds.top,
        w: rect.bounds.width(),
        h: rect.bounds.height(),
    };
    let outer = frame_outer_transform(page, rect.item_transform);
    if let Some(shadow) =
        resolve_frame_shadow(rect.drop_shadow.as_ref(), drop_shadow, palette, cmyk_xform)
    {
        emit_drop_shadow_rect_transformed(r, outer, shadow, &mut page.list);
    }
    let fill = rect
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);
    emit_rect_transformed(r, outer, fill, &mut page.list);
    if let Some(stroke) = rect
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    {
        let width = rect.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_rect_transformed(r, outer, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

/// Resolve, decode, and emit a placed image for a rectangle. Skips
/// silently if `assets` is unset, the resolver returns `None`, or
/// decoding fails — IDMLs without their linked assets should still
/// produce a usable render of the surrounding geometry.
fn emit_rectangle_image(
    page: &mut BuiltPage,
    rect: &Rectangle,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) {
    let Some(uri) = rect.image_link.as_deref() else {
        return;
    };
    let r = Rect {
        x: rect.bounds.left,
        y: rect.bounds.top,
        w: rect.bounds.width(),
        h: rect.bounds.height(),
    };
    let outer = frame_outer_transform(page, rect.item_transform);
    let id = if let Some(cached) = page_image_cache.get(uri) {
        *cached
    } else {
        // Cross-page decode cache hit: clone the cached RGBA into
        // this page's image pool.
        let decoded = if let Some(d) = decoded_cache.get(uri) {
            d.clone()
        } else {
            let Some(resolver) = options.assets else {
                return;
            };
            let Some(bytes) = resolver.resolve_image(uri) else {
                tracing::warn!(uri, "image resolver returned no bytes; skipping");
                return;
            };
            let Some(d) = decode_image_bytes(bytes.as_ref()) else {
                tracing::warn!(uri, "image decode failed; skipping");
                return;
            };
            decoded_cache.insert(uri.to_string(), d.clone());
            d
        };
        let id = page.list.push_image(decoded);
        page_image_cache.insert(uri.to_string(), id);
        id
    };
    idml_compose::emit_image_at(r, outer, id, &mut page.list);
}

/// Decode raw image bytes to RGBA8. Format detection is via magic
/// bytes (`image::load_from_memory`). Returns `None` for any decode
/// or buffer-shape failure.
fn decode_image_bytes(bytes: &[u8]) -> Option<idml_compose::DecodedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(idml_compose::DecodedImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

/// First-baseline y (1/64 pt) for the head frame of a story,
/// honouring `<TextFramePreference FirstBaselineOffset>` and the
/// top inset. `default_64` is the renderer's heuristic baseline
/// (LayoutOptions::new gives `point_size * 0.8 * 64`) used for
/// `AscentOffset` (the IDML default) and any unrecognised value.
/// `metrics` carries the head font's OS/2 / hhea metrics; when
/// present, `CapHeight` and `XHeight` policies use the font's
/// real values instead of a 70% / 50% heuristic.
fn first_baseline_for_frame(
    frame: &TextFrame,
    point_size: f32,
    default_64: i32,
    metrics: Option<&FontMetrics>,
) -> i32 {
    const CAP_HEIGHT_FALLBACK: f32 = 0.70;
    const X_HEIGHT_FALLBACK: f32 = 0.50;
    let top_inset_64 = frame
        .inset_spacing
        .map(|i| (i[0] * idml_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    let pt_to_64 = |pt: f32| (pt * idml_text::shape::ADVANCE_PRECISION).round() as i32;
    let em_fraction_to_64 = |frac: f32| pt_to_64(point_size * frac);
    let policy_offset_64 = match frame.first_baseline_offset.as_deref() {
        Some("CapHeight") => em_fraction_to_64(
            metrics
                .and_then(|m| m.cap_height)
                .unwrap_or(CAP_HEIGHT_FALLBACK),
        ),
        Some("XHeight") => em_fraction_to_64(
            metrics
                .and_then(|m| m.x_height)
                .unwrap_or(X_HEIGHT_FALLBACK),
        ),
        Some("EmBoxHeight") => pt_to_64(point_size),
        // FixedHeight / LeadingOffset use MinimumFirstBaselineOffset
        // verbatim. Falls back to default when missing.
        Some("FixedHeight") | Some("LeadingOffset") => frame
            .minimum_first_baseline_offset
            .map(pt_to_64)
            .unwrap_or(default_64),
        // AscentOffset (IDML default) and unrecognised values: use
        // the font's ascender if available; otherwise fall through
        // to the renderer heuristic the LayoutOptions gave us.
        _ => metrics
            .map(|m| em_fraction_to_64(m.ascender))
            .unwrap_or(default_64),
    };
    top_inset_64 + policy_offset_64
}

/// Build the outer affine that maps a frame's local-space rect into
/// page-space pixels: page-origin offset composed with the frame's
/// `ItemTransform` (identity when absent). Identity ItemTransform is
/// the common case — the result collapses to a pure translation, so
/// the rasterizer's axis-aligned fast paths still apply.
fn frame_outer_transform(page: &BuiltPage, item_transform: Option<[f32; 6]>) -> Transform {
    let (ox, oy) = page.spread_origin;
    let page_origin = Transform::translate(-ox, -oy);
    match item_transform {
        Some(m) => page_origin.compose(&Transform(m)),
        None => page_origin,
    }
}

/// Resolve the effective shadow for a frame. Per-frame IDML shadow
/// wins; the synthetic `fallback` (from `PipelineOptions`) is used
/// when the frame carries none. Returns `None` for fully-transparent
/// shadows so callers don't emit a no-op.
fn resolve_frame_shadow(
    frame_shadow: Option<&idml_parse::DropShadowSetting>,
    fallback: Option<DropShadow>,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<DropShadow> {
    frame_shadow
        .and_then(|s| convert_setting_to_shadow(s, palette, cmyk_xform))
        .or(fallback)
}

/// Convert an IDML `<DropShadowSetting>` to a compose-layer `DropShadow`.
/// The parser already drops `Mode="None"` settings, so we only have
/// to filter out fully-transparent shadows here.
fn convert_setting_to_shadow(
    setting: &idml_parse::DropShadowSetting,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<DropShadow> {
    let opacity = (setting.opacity_pct / 100.0).clamp(0.0, 1.0);
    if opacity == 0.0 {
        return None;
    }
    let color = setting
        .effect_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
        .and_then(paint_as_solid)
        .unwrap_or(Color::BLACK);
    Some(DropShadow {
        offset_x: setting.x_offset,
        offset_y: setting.y_offset,
        blur_radius: setting.size,
        color,
        opacity,
    })
}

/// Pull the inner `Color` out of a solid paint, returning `None`
/// for gradient (or future image) paints. Used wherever a context
/// can only consume a flat colour (drop shadow, per-glyph paint).
fn paint_as_solid(p: Paint) -> Option<Color> {
    match p {
        Paint::Solid(c) => Some(c),
        _ => None,
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

/// Solid-paint resolver. Used by per-cluster glyph paint pickers
/// (where embedding gradient stops per glyph would be wasteful) and
/// by callers that don't have a `&mut DisplayList`.
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
            #[cfg(target_arch = "wasm32")]
            {
                let _ = xform;
            }
        }
    }
    let [r, g, b] = graphic::to_linear_rgb(entry)?;
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// Resolver that also handles gradient swatches.
///
/// Gradient ids resolve to a `Paint::LinearGradient` whose stops live
/// in `list.gradients`. Solid colours fall through to
/// `color_id_to_paint`. Used for frame fills (which can carry
/// gradient swatches); not used for per-glyph paints.
pub fn color_id_to_paint_with_list(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut DisplayList,
) -> Option<Paint> {
    if let Some(grad) = palette.gradients.get(id) {
        let stops: Vec<idml_compose::GradientStop> = grad
            .stops
            .iter()
            .filter_map(|s| {
                let color = color_id_to_paint(&s.stop_color, palette, cmyk_xform)
                    .and_then(paint_as_solid)?;
                Some(idml_compose::GradientStop {
                    offset: (s.location_pct / 100.0).clamp(0.0, 1.0),
                    color,
                })
            })
            .collect();
        if stops.len() < 2 {
            return None;
        }
        // Default endpoints: top-to-bottom across the unit square.
        // Frame-level GradientFillStart / Length / Angle attributes
        // override these — that wiring lands when the spread parser
        // captures those fields.
        let id = list.push_linear_gradient(idml_compose::LinearGradient {
            start: (0.0, 0.0),
            end: (0.0, 1.0),
            stops,
        });
        return Some(Paint::LinearGradient(id));
    }
    color_id_to_paint(id, palette, cmyk_xform)
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

/// Like [`build_run_paint_picker`] but uses each run's cascaded
/// `fill_color` (so a run that only carries an `AppliedCharacterStyle`
/// still picks up the right paint).
fn build_run_paint_picker_resolved(
    paragraph: &idml_parse::Paragraph,
    resolved_runs: &[idml_scene::ResolvedRunAttrs],
    palette: &Graphic,
    default: Paint,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len());
    let mut cursor: u32 = 0;
    for (i, run) in paragraph.runs.iter().enumerate() {
        let paint = resolved_runs[i]
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

/// Walk a laid-out line's glyphs and emit horizontal stroke
/// commands for any underlined or struck-through ranges. The stroke
/// uses the run's resolved fill colour (per cluster, via the same
/// picker as the glyphs themselves) so coloured text gets coloured
/// decoration.
fn emit_line_decorations(
    line: &idml_text::layout::LaidOutLine,
    picker: &RunPaintPicker,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use idml_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() {
        return;
    }
    // Two passes — underline (12% of em below baseline) then
    // strikethrough (30% above) — so a glyph with both gets two
    // stripes. Offsets are crude approximations until we read the
    // font's `OS/2` table for the spec'd y_offset / strikeout_pos.
    const UNDERLINE_OFFSET_EM: f32 = 0.12;
    const STRIKETHRU_OFFSET_EM: f32 = -0.30;
    type Pred = fn(&idml_text::PositionedGlyph) -> bool;
    let underline: Pred = |g| g.underline;
    let strikethru: Pred = |g| g.strikethru;
    for (predicate, y_offset_factor) in [
        (underline, UNDERLINE_OFFSET_EM),
        (strikethru, STRIKETHRU_OFFSET_EM),
    ] {
        let mut start = 0;
        while start < line.glyphs.len() {
            if !predicate(&line.glyphs[start]) {
                start += 1;
                continue;
            }
            let mut end = start + 1;
            while end < line.glyphs.len() && predicate(&line.glyphs[end]) {
                end += 1;
            }
            let g0 = &line.glyphs[start];
            let g_last = &line.glyphs[end - 1];
            let x_start_pt = frame_origin_pt.0 + (g0.x as f32) / ADVANCE_PRECISION;
            let x_end_pt =
                frame_origin_pt.0 + ((g_last.x + g_last.x_advance) as f32) / ADVANCE_PRECISION;
            let baseline_pt = frame_origin_pt.1 + (line.baseline_y as f32) / ADVANCE_PRECISION;
            let y_pt = baseline_pt + g0.point_size * y_offset_factor;
            let stroke_w = (g0.point_size * 0.06_f32).max(0.4);
            // Decoration paint matches the run's fill at the start
            // glyph's cluster.
            let paint = picker.pick(g0.cluster);
            idml_compose::emit_line(
                x_start_pt,
                y_pt,
                x_end_pt,
                y_pt,
                Stroke::new(stroke_w),
                paint,
                list,
            );
            start = end;
        }
    }
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

/// Map IDML `<TabStop Alignment="...">` values to the layout
/// crate's `TabAlignment`.
fn map_tab_alignment(a: Option<&str>) -> idml_text::layout::TabAlignment {
    match a {
        Some("RightAlign") => idml_text::layout::TabAlignment::Right,
        Some("CenterAlign") => idml_text::layout::TabAlignment::Center,
        Some("CharacterAlign") => idml_text::layout::TabAlignment::Decimal,
        _ => idml_text::layout::TabAlignment::Left,
    }
}

/// Per-render font cache. Pre-resolves every distinct (family, style)
/// pair referenced anywhere in the document via the configured
/// `AssetResolver`. Falls back to `options.font` when nothing
/// resolves. Also extracts OS/2 / hhea metrics per font_id at
/// build time so baseline math doesn't have to re-parse the font
/// per paragraph.
struct FontTable {
    cache: HashMap<(String, Option<String>), Bytes>,
    fallback: Option<Bytes>,
    /// Metrics keyed by `fnv_1a_u32(bytes)` (same id the rest of
    /// the pipeline uses for glyph-cache routing).
    metrics: HashMap<u32, FontMetrics>,
}

/// Per-font metrics the renderer reads at baseline-placement time.
/// All values are scale-free (unit = font units / `units_per_em`)
/// so callers can multiply by `point_size` to get pt.
#[derive(Debug, Clone, Copy)]
struct FontMetrics {
    /// `OS/2.sCapHeight`, fraction of em. `None` when the font
    /// doesn't expose it (legacy fonts without the OS/2 v2+ field).
    cap_height: Option<f32>,
    /// `OS/2.sxHeight`, fraction of em.
    x_height: Option<f32>,
    /// `hhea.ascender`, fraction of em. Always present.
    ascender: f32,
}

impl FontTable {
    fn build(document: &Document, options: &PipelineOptions) -> Self {
        let fallback = options.font.map(Bytes::copy_from_slice);
        let mut cache: HashMap<(String, Option<String>), Bytes> = HashMap::new();
        if let Some(resolver) = options.assets {
            // Walk every run in every story and collect distinct
            // keys before calling the resolver — `resolve_font`
            // may be a JS Promise wrapper or a disk read, so
            // deduping matters. Each run's effective (family,
            // style) comes from the cascade (run direct > applied
            // character style > applied paragraph style) so a run
            // that only carries `AppliedCharacterStyle` still
            // requests the right font.
            let mut keys: std::collections::HashSet<(String, Option<String>)> =
                std::collections::HashSet::new();
            for parsed in &document.stories {
                for paragraph in &parsed.story.paragraphs {
                    for run in &paragraph.runs {
                        let resolved = document.resolved_run_attrs(paragraph, run);
                        let Some(family) = resolved.font else {
                            continue;
                        };
                        keys.insert((family, resolved.font_style));
                    }
                }
            }
            cache.reserve(keys.len());
            for key in keys {
                if let Some(bytes) = resolver.resolve_font(&key.0, key.1.as_deref()) {
                    cache.insert(key, bytes);
                }
            }
        }
        // Parse metrics for every distinct byte buffer we ended up
        // caching, plus the fallback. Keyed by the same fnv hash
        // emit_paragraph uses for font_id — so the lookup is direct.
        let mut metrics: HashMap<u32, FontMetrics> = HashMap::new();
        let mut record = |bytes: &[u8]| {
            let id = fnv_1a_u32(bytes);
            if metrics.contains_key(&id) {
                return;
            }
            if let Some(m) = parse_font_metrics(bytes) {
                metrics.insert(id, m);
            }
        };
        for b in cache.values() {
            record(b.as_ref());
        }
        if let Some(b) = fallback.as_ref() {
            record(b.as_ref());
        }
        Self {
            cache,
            fallback,
            metrics,
        }
    }

    /// Look up the bytes a paragraph should shape with.
    /// Resolver hit > options.font fallback. `None` means no font
    /// is available — caller skips the paragraph.
    fn bytes_for(&self, family: Option<&str>, style: Option<&str>) -> Option<Bytes> {
        if let Some(family) = family {
            // Direct (family, style) hit, then bare-family hit, so
            // a doc that only registers "Body Font" still picks up
            // its bold runs.
            if let Some(b) = self
                .cache
                .get(&(family.to_string(), style.map(str::to_string)))
            {
                return Some(b.clone());
            }
            if let Some(b) = self.cache.get(&(family.to_string(), None)) {
                return Some(b.clone());
            }
        }
        self.fallback.clone()
    }

    fn metrics_for(&self, font_id: u32) -> Option<&FontMetrics> {
        self.metrics.get(&font_id)
    }
}

fn parse_font_metrics(bytes: &[u8]) -> Option<FontMetrics> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return None;
    }
    Some(FontMetrics {
        cap_height: face.capital_height().map(|v| v as f32 / upem),
        x_height: face.x_height().map(|v| v as f32 / upem),
        ascender: face.ascender() as f32 / upem,
    })
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
