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

//! Concept 3 — print-grade PDF export.
//!
//! A SECOND backend over the same resolved per-page display list the
//! GPU renderer consumes (`paged_compose::DisplayList`): text stays
//! text (subset CID fonts + ToUnicode, via the glyph-run
//! side-channel), vectors stay vectors, placed images pass through
//! (DCTDecode, no re-encode), colour preserves native spaces
//! (DeviceCMYK / ICCBased / Separation / Lab) through Concept 2's
//! CMM in export-convert mode, transparency stays LIVE (PDF/X-4 —
//! no flattener). Built directly on `pdf-writer` (+ `subsetter` for
//! fonts, `xmp-writer` for conformance metadata); `typst-pdf` is
//! the reading reference, never a dependency.
//!
//! Output is DETERMINISTIC: no wall-clock, ref ids allocated in walk
//! order, XMP ids derived from content; the same input yields the
//! same bytes (golden-file discipline).

pub mod color;
pub mod gstate;
pub mod image;
pub mod marks;
pub mod page;
pub mod path;
pub mod text;
pub mod transparency;
pub mod writer;

use paged_compose::{GlyphRunTable, Paint};

/// Conformance target. `Pdf17` is a plain, valid PDF 1.7; `PdfX4`
/// adds the ISO 15930-7 requirements (OutputIntent, TrimBox,
/// Trapped, XMP conformance keys, PDF 1.6 header, all fonts
/// embedded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PdfStandard {
    #[default]
    Pdf17,
    PdfX4,
}

/// Concept 2's two colour policies (E5): `PreserveNumbers` leaves
/// CMYK channels untouched (pure-K stays pure-K — the default);
/// `ConvertToDestination` routes everything through the CMM to the
/// output-intent space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportColorPolicy {
    #[default]
    PreserveNumbers,
    ConvertToDestination,
}

/// The caller configures the CMM with this BEFORE building
/// [`ExportInput`] (`IccCmm::configure_export(profile, policy.into())`)
/// — the exporter itself only holds `&dyn Cmm` and cannot mutate it.
impl From<ExportColorPolicy> for paged_color::ExportPolicy {
    fn from(p: ExportColorPolicy) -> Self {
        match p {
            ExportColorPolicy::PreserveNumbers => Self::PreserveNumbers,
            ExportColorPolicy::ConvertToDestination => Self::ConvertToDestination,
        }
    }
}

/// Printer's marks selection. All drawn OUTSIDE the trim, by the
/// exporter only — they never touch the document scene.
#[derive(Debug, Clone, Copy, Default)]
pub struct MarkOptions {
    pub crop_marks: bool,
    pub registration_marks: bool,
    pub color_bars: bool,
    pub page_info: bool,
    /// Mark stroke weight in pt (default 0.25).
    pub weight_pt: f32,
    /// Offset of marks from the bleed edge in pt (default 6).
    pub offset_pt: f32,
}

/// Bleed per edge in pt. `None` ⇒ use the document's declared bleed
/// (designmap `DocumentPreference`), which may be zero.
#[derive(Debug, Clone, Copy, Default)]
pub struct BleedOptions {
    pub override_pt: Option<[f32; 4]>, // top, inside/left, bottom, outside/right
}

/// Image handling. Downsampling is OFF by default ("preserve").
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageOptions {
    /// Resample colour/grey images above this effective ppi down to
    /// it (bicubic). `None` = never resample.
    pub downsample_ppi: Option<f32>,
    /// Re-encode resampled images as JPEG at this quality (1-100);
    /// `None` = lossless Flate.
    pub jpeg_quality: Option<u8>,
}



/// What to do with fonts whose fsType forbids embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestrictedFontPolicy {
    /// Keep those runs as vector outlines + surface a diagnostic
    /// (the safe default — never silently non-compliant).
    #[default]
    Outline,
    /// Fail the export with `ExportError::FontNotEmbeddable`.
    Fail,
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub standard: PdfStandard,
    pub color_policy: ExportColorPolicy,
    /// Name of the output condition for the OutputIntent (e.g.
    /// "Coated FOGRA39"). Required for PDF/X-4 (with profile bytes
    /// in [`ExportProfiles::output_intent`]).
    pub output_condition: Option<String>,
    /// 0-based page indices to export; `None` = all pages.
    pub page_range: Option<(usize, usize)>,
    pub marks: MarkOptions,
    pub bleed: BleedOptions,
    pub images: ImageOptions,
    pub restricted_fonts: RestrictedFontPolicy,
    /// Raster resolution for effect soft-mask stamps (shadows,
    /// glows). 150 ppi default — industry practice for shadow
    /// smasks.
    pub effect_dpi: f32,
    /// Document title for the Info dict / XMP (from document
    /// metadata, NOT wall-clock-derived).
    pub title: Option<String>,
}

/// ICC payloads the exporter embeds.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExportProfiles<'a> {
    /// The document's CMYK working space (ICCBased /N 4 tagging).
    pub cmyk_working: Option<&'a [u8]>,
    /// The destination/output-intent profile (PDF/X DestOutputProfile).
    pub output_intent: Option<&'a [u8]>,
    /// sRGB profile for ICCBased /N 3 tagging of RGB content.
    /// `None` ⇒ RGB content emits as DeviceRGB.
    pub srgb: Option<&'a [u8]>,
}

/// Ink-manager state the exporter honours (Concept 2 — AC-8: these
/// never modified the swatches; export is where they take effect).
#[derive(Debug, Clone, Default)]
pub struct ExportInkSettings {
    /// Spot swatch ids to output as process (their alternate).
    pub convert_to_process: Vec<String>,
    /// Spot swatch id → alias-target spot swatch id.
    pub aliases: Vec<(String, String)>,
    pub use_standard_lab_for_spots: bool,
}

/// One non-fatal export finding (restricted font, missing image,
/// unembeddable resource). Surfaced to preflight, never silent.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportDiagnostic {
    FontNotEmbeddable { font_id: u32, runs_outlined: usize },
    ImageMissingBytes { image_index: u32 },
}

/// Severity of a preflight finding (panels.md gap 20). Errors block a
/// faithful export; warnings note a degraded-but-shipped outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    Warning,
    Error,
}

impl FindingSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingSeverity::Warning => "warning",
            FindingSeverity::Error => "error",
        }
    }
}

impl ExportDiagnostic {
    /// Stable lower-snake code for the finding (panels.md gap 20) so a
    /// panel can group/icon without parsing `message`.
    pub fn code(&self) -> &'static str {
        match self {
            ExportDiagnostic::FontNotEmbeddable { .. } => "font_not_embeddable",
            ExportDiagnostic::ImageMissingBytes { .. } => "image_missing_bytes",
        }
    }

    /// Severity. Both current findings are warnings: the font path
    /// keeps the run as vector outlines (visually faithful, just not
    /// selectable/searchable) and the image path falls back to a
    /// placeholder — neither aborts the export. A future
    /// embed-or-fail policy would raise the font case to `Error`.
    pub fn severity(&self) -> FindingSeverity {
        match self {
            ExportDiagnostic::FontNotEmbeddable { .. } => FindingSeverity::Warning,
            ExportDiagnostic::ImageMissingBytes { .. } => FindingSeverity::Warning,
        }
    }

    /// Human-readable summary for the dialog.
    pub fn message(&self) -> String {
        match self {
            ExportDiagnostic::FontNotEmbeddable {
                font_id,
                runs_outlined,
            } => format!(
                "font {font_id} forbids embedding; {runs_outlined} run(s) kept as outlines"
            ),
            ExportDiagnostic::ImageMissingBytes { image_index } => {
                format!("placed image {image_index} had no usable bytes; placeholder drawn")
            }
        }
    }
}

/// Structured preflight finding (panels.md gap 20): the raw export
/// diagnostic enriched with severity + the body-page index it was
/// raised on (when known). The export reply carries these so the
/// dialog can render a grouped, severity-coloured findings list and
/// jump to the offending page.
#[derive(Debug, Clone, PartialEq)]
pub struct PreflightFinding {
    pub code: &'static str,
    pub severity: FindingSeverity,
    pub message: String,
    /// Flat body-page index the finding was raised on; `None` for
    /// findings raised at document `finish()` (no single page).
    pub page_index: Option<usize>,
}

impl PreflightFinding {
    fn from_diag(d: &ExportDiagnostic, page_index: Option<usize>) -> Self {
        Self {
            code: d.code(),
            severity: d.severity(),
            message: d.message(),
            page_index,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("PDF/X-4 requires an output intent profile")]
    MissingOutputIntent,
    #[error("font {font_id} forbids embedding (fsType) and policy is Fail")]
    FontNotEmbeddable { font_id: u32 },
    #[error("page index {0} out of range")]
    PageOutOfRange(usize),
    #[error("font subsetting failed: {0}")]
    Subset(String),
    #[error("export session in wrong state: {0}")]
    SessionState(&'static str),
}

/// Resolves a `font_id` (the display list's glyph-table id space)
/// to the ORIGINAL face bytes. Both the renderer's `FontTable` and
/// `CanvasModel` can implement this.
pub trait FontByteSource {
    fn font_bytes(&self, font_id: u32) -> Option<&[u8]>;
}

impl FontByteSource for paged_renderer::FontTable {
    fn font_bytes(&self, font_id: u32) -> Option<&[u8]> {
        self.face_data(font_id)
    }
}

/// Everything the exporter consumes. The display list alone cannot
/// recover spot ink names/alternates or true source colour spaces,
/// so the scene palette + the CMM ride along.
pub struct ExportInput<'a> {
    pub doc: &'a paged_renderer::BuiltDocument,
    pub palette: &'a paged_parse::graphic::Graphic,
    /// `None` ⇒ text stays as vector outlines (no font embedding).
    pub fonts: Option<&'a dyn FontByteSource>,
    pub cmm: &'a dyn paged_color::Cmm,
    pub profiles: ExportProfiles<'a>,
    pub inks: ExportInkSettings,
    pub options: ExportOptions,
    /// Document bleed/slug parsed from the designmap (pt, TLBR);
    /// the options' override wins when present.
    pub doc_bleed: [f32; 4],
    pub doc_slug: [f32; 4],
}

pub struct ExportResult {
    pub bytes: Vec<u8>,
    pub diagnostics: Vec<ExportDiagnostic>,
    /// panels.md gap 20 — `diagnostics` enriched with severity + the
    /// body-page index each was raised on. Parallel to `diagnostics`
    /// (same length, same order).
    pub findings: Vec<PreflightFinding>,
    pub pages_exported: usize,
}

/// One-shot export over every page in range.
pub fn export_pdf(input: ExportInput<'_>) -> Result<ExportResult, ExportError> {
    let mut session = ExportSession::begin(&input)?;
    while session.pages_remaining() > 0 {
        session.export_next_page(&input)?;
    }
    session.finish(&input)
}

/// Incremental export session — one page per `export_next_page`
/// call, so a synchronous worker can interleave progress replies and
/// honour cancellation between pages (the protocol-26 wire drives
/// this from the main thread).
///
/// The session holds NO borrow of the inputs (so a worker can park
/// it in a map across messages and own the BuiltDocument beside it);
/// instead the SAME logical [`ExportInput`] must be passed to every
/// call — the writer state indexes into `input.doc.pages` and pools
/// fonts/profiles across pages, so swapping inputs mid-session
/// produces a corrupt document.
pub struct ExportSession {
    state: writer::DocState,
    page_indices: Vec<usize>,
    next: usize,
    diagnostics: Vec<ExportDiagnostic>,
    /// panels.md gap 20 — body-page index each diagnostic was raised
    /// on, parallel to `diagnostics`. `None` for findings raised at
    /// document `finish()`. Stamped by diffing the diagnostics length
    /// before/after each `export_next_page`.
    finding_pages: Vec<Option<usize>>,
}

impl ExportSession {
    pub fn begin(input: &ExportInput<'_>) -> Result<Self, ExportError> {
        if input.options.standard == PdfStandard::PdfX4
            && input.profiles.output_intent.is_none()
        {
            return Err(ExportError::MissingOutputIntent);
        }
        let total = input.doc.pages.len();
        let page_indices: Vec<usize> = match input.options.page_range {
            Some((from, to)) => {
                if from >= total || to >= total || from > to {
                    return Err(ExportError::PageOutOfRange(to.max(from)));
                }
                (from..=to).collect()
            }
            None => (0..total).collect(),
        };
        let state = writer::DocState::new(input);
        Ok(Self {
            state,
            page_indices,
            next: 0,
            diagnostics: Vec::new(),
            finding_pages: Vec::new(),
        })
    }

    pub fn page_count(&self) -> usize {
        self.page_indices.len()
    }

    pub fn pages_done(&self) -> usize {
        self.next
    }

    pub fn pages_remaining(&self) -> usize {
        self.page_indices.len() - self.next
    }

    pub fn export_next_page(&mut self, input: &ExportInput<'_>) -> Result<(), ExportError> {
        let Some(&page_index) = self.page_indices.get(self.next) else {
            return Err(ExportError::SessionState("no pages remaining"));
        };
        let page = &input.doc.pages[page_index];
        page::export_page(&mut self.state, input, page, &mut self.diagnostics)?;
        // Stamp every diagnostic this page raised with the body-page
        // index, so the preflight finding can deep-link to it. Newly
        // pushed slots (if any) fill with this page index; existing
        // slots from prior pages keep theirs.
        self.finding_pages
            .resize(self.diagnostics.len(), Some(page_index));
        self.next += 1;
        Ok(())
    }

    pub fn finish(mut self, input: &ExportInput<'_>) -> Result<ExportResult, ExportError> {
        if self.pages_remaining() > 0 {
            return Err(ExportError::SessionState("pages remaining; export them first"));
        }
        let bytes = self.state.finish(input, &mut self.diagnostics)?;
        // Document-`finish` diagnostics have no single page.
        self.finding_pages.resize(self.diagnostics.len(), None);
        let findings = self
            .diagnostics
            .iter()
            .zip(self.finding_pages.iter())
            .map(|(d, page)| PreflightFinding::from_diag(d, *page))
            .collect();
        Ok(ExportResult {
            bytes,
            diagnostics: self.diagnostics,
            findings,
            pages_exported: self.next,
        })
    }
}

/// Re-export for consumers assembling glyph tables.
pub use paged_compose::GlyphRunEntry;

/// The PDF/X-4 XMP packet. DETERMINISTIC: ids derive from a content
/// hash of the export inputs; dates are a fixed epoch (PDF/X allows
/// any valid date — golden-file byte-identity wins).
pub(crate) fn xmp_packet(input: &ExportInput<'_>) -> String {
    let mut xmp = xmp_writer::XmpWriter::new();
    if let Some(title) = &input.options.title {
        xmp.title([(None, title.as_str())]);
    }
    xmp.creator(["paged"]);
    xmp.pdf_version("1.6");
    // Fixed epoch — never wall-clock (determinism, concept AC-9).
    let date = xmp_writer::DateTime::date(2000, 1, 1);
    xmp.create_date(date);
    xmp.modify_date(date);
    // Deterministic ids from the document content.
    let hash = content_hash(input);
    let id = format!("uuid:paged-{hash:032x}");
    xmp.document_id(&id);
    xmp.instance_id(&id);
    // PDF/X conformance key.
    xmp.pdfx_version("PDF/X-4");
    xmp.finish(None)
}

fn content_hash(input: &ExportInput<'_>) -> u128 {
    // FNV-1a over the page dimensions + command counts — stable
    // across runs for the same document, cheap, and unique enough
    // for an instance id.
    let mut h: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    let prime: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut feed = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= b as u128;
            h = h.wrapping_mul(prime);
        }
    };
    for page in &input.doc.pages {
        feed(page.width_pt.to_bits() as u64);
        feed(page.height_pt.to_bits() as u64);
        feed(page.list.commands.len() as u64);
    }
    h
}

/// True when this paint needs the CMYK overprint graphics state.
pub(crate) fn paint_is_cmyk(paint: &Paint) -> bool {
    matches!(paint, Paint::Cmyk { .. })
}

#[allow(unused)]
pub(crate) fn glyph_table_of(list: &paged_compose::DisplayList) -> Option<&GlyphRunTable> {
    list.glyph_runs.as_ref()
}

#[cfg(test)]
mod preflight_tests {
    use super::*;

    #[test]
    fn preflight_finding_maps_code_severity_and_page() {
        // panels.md gap 20 — structured findings carry a stable code,
        // a severity, and the page they were raised on.
        let font = ExportDiagnostic::FontNotEmbeddable {
            font_id: 7,
            runs_outlined: 3,
        };
        let f = PreflightFinding::from_diag(&font, Some(2));
        assert_eq!(f.code, "font_not_embeddable");
        assert_eq!(f.severity, FindingSeverity::Warning);
        assert_eq!(f.page_index, Some(2));
        assert!(f.message.contains('7') && f.message.contains('3'));

        let img = ExportDiagnostic::ImageMissingBytes { image_index: 4 };
        let f = PreflightFinding::from_diag(&img, None);
        assert_eq!(f.code, "image_missing_bytes");
        assert_eq!(f.severity, FindingSeverity::Warning);
        assert_eq!(f.page_index, None);
        assert_eq!(FindingSeverity::Warning.as_str(), "warning");
        assert_eq!(FindingSeverity::Error.as_str(), "error");
    }
}
