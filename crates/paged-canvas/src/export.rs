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

//! Concept 3 — the worker-side PDF export session (protocol 26).
//!
//! `ExportPdfBegin` re-runs `build_document` ONE-SHOT with the glyph
//! side-channel on and the splice caches OFF — the live canvas build
//! is never touched (its caches splice command ranges, which would
//! break the `command_index` parallelism the glyph table relies on),
//! and the canvas command stream stays byte-identical because the
//! flag never flips there. The session owns everything it needs
//! (built document, face bytes, profiles, CMM) so it can sit in the
//! worker's session map across messages; the main thread drives one
//! `ExportPdfPage` per call — real progress + cancellation on a
//! synchronous worker.

use std::collections::HashMap;

use crate::channel::ExportPdfWireOptions;
use crate::model::CanvasModel;
use paged_export_pdf::{
    BleedOptions, ExportColorPolicy, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles,
    ExportSession, FontByteSource, ImageOptions, MarkOptions, PdfStandard, RestrictedFontPolicy,
};

/// Result of [`CanvasExportSession::finish`]: the serialised PDF plus
/// both the human-readable summary lines and the structured preflight
/// findings (panels.md gap 20). A named struct keeps the `finish`
/// signature legible (and below clippy's tuple-complexity bar).
pub struct FinishedExport {
    pub pdf_bytes: Vec<u8>,
    /// Human-readable summary lines for the dialog's status row.
    pub diagnostics: Vec<String>,
    /// Structured findings (code + severity + page) for the grouped
    /// findings list.
    pub findings: Vec<crate::channel::PreflightFinding>,
}

/// Face bytes copied out of the model's font table at begin time —
/// owned, so the session never borrows the (mutable) model.
struct OwnedFontBytes(HashMap<u32, Vec<u8>>);

impl FontByteSource for OwnedFontBytes {
    fn font_bytes(&self, font_id: u32) -> Option<&[u8]> {
        self.0.get(&font_id).map(|b| b.as_slice())
    }
}

/// One in-flight export, parked on the worker between messages.
pub struct CanvasExportSession {
    built: paged_renderer::BuiltDocument,
    fonts: OwnedFontBytes,
    palette: paged_model::Graphic,
    cmm: paged_color::IccCmm,
    cmyk_working: Option<Vec<u8>>,
    output_intent: Option<Vec<u8>>,
    inks: ExportInkSettings,
    options: ExportOptions,
    doc_bleed: [f32; 4],
    doc_slug: [f32; 4],
    session: ExportSession,
}

impl CanvasExportSession {
    /// Build the one-shot scene + writer state. Returns the session
    /// and its page count.
    pub fn begin(
        model: &CanvasModel,
        wire: &ExportPdfWireOptions,
    ) -> Result<(Self, usize), String> {
        let mut options = wire_to_options(wire)?;

        // One-shot build: glyph side-channel ON, splice caches OFF
        // (stable command indices), proof state ignored (export uses
        // the WORKING space, never the proof simulation).
        let built = model
            .build_for_export()
            .map_err(|e| format!("export build failed: {e}"))?;
        if wire.bookmarks {
            options.bookmarks = pdf_bookmarks(model.scene(), &built);
        }

        // Face bytes for every font the glyph tables reference, keyed
        // by `face_id` — the id that resolves to BYTES. `font_id` is
        // the glyph-outline cache key with the wght mixed in; keying
        // this map on it meant `face_data` missed every time, the
        // `retain` below dropped the whole map, and the exporter fell
        // through to drawing outlines. Every PDF the engine produced
        // carried no text layer because of these two lines.
        let mut face_bytes: HashMap<u32, Vec<u8>> = HashMap::new();
        for page in &built.pages {
            if let Some(table) = &page.list.glyph_runs {
                for entry in &table.entries {
                    let Some(face_id) = entry.face_id else {
                        continue;
                    };
                    face_bytes.entry(face_id).or_insert_with(|| {
                        model
                            .font_table()
                            .face_data(face_id)
                            .map(|b| b.to_vec())
                            .unwrap_or_default()
                    });
                }
            }
        }
        face_bytes.retain(|_, v| !v.is_empty());

        // Profiles: the working space is the model's ACTIVE bytes;
        // the output intent resolves by registry name, falling back
        // to the working space (the common "export to the working
        // condition" case).
        let cmyk_working = model.active_cmyk_profile().map(|b| b.to_vec());
        let output_intent = match &wire.output_intent_profile {
            Some(name) => match model.registered_profile(name) {
                Some(bytes) => Some(bytes.to_vec()),
                None => return Err(format!("output intent profile not registered: {name}")),
            },
            None => cmyk_working.clone(),
        };
        if options.standard == PdfStandard::PdfX4 && output_intent.is_none() {
            return Err("PDF/X-4 requires an output intent profile".into());
        }

        // The CMM, configured for export (policy + destination).
        let settings = model.color_settings_state();
        let mut cmm = paged_color::IccCmm::new(
            cmyk_working.as_deref(),
            paged_color::DisplaySetup {
                intent: settings.intent,
                bpc: settings.bpc,
            },
        );
        cmm.configure_export(output_intent.as_deref(), options.color_policy.into());

        // Ink Manager settings (Concept 2 AC-8: they take effect at
        // output time, never on the swatches).
        let mut inks = ExportInkSettings::default();
        for (id, s) in model.ink_settings_map() {
            if s.convert_to_process {
                inks.convert_to_process.push(id.clone());
            }
            if let Some(target) = &s.alias_to {
                inks.aliases.push((id.clone(), target.clone()));
            }
        }
        inks.use_standard_lab_for_spots = model.use_standard_lab_for_spots();

        let pref = model.document_preference();
        let doc_bleed = [
            pref.bleed_top,
            pref.bleed_inside_or_left,
            pref.bleed_bottom,
            pref.bleed_outside_or_right,
        ];
        let doc_slug = [
            pref.slug_top,
            pref.slug_inside_or_left,
            pref.slug_bottom,
            pref.slug_right_or_outside,
        ];

        let palette = model.palette().clone();
        let fonts = OwnedFontBytes(face_bytes);

        // The core session holds no borrows — build it from a
        // temporary input over the locals, then move everything in.
        let session = {
            let input = ExportInput {
                doc: &built,
                palette: &palette,
                fonts: Some(&fonts),
                cmm: &cmm,
                profiles: ExportProfiles {
                    cmyk_working: cmyk_working.as_deref(),
                    output_intent: output_intent.as_deref(),
                    srgb: None,
                },
                inks: inks.clone(),
                options: options.clone(),
                doc_bleed,
                doc_slug,
            };
            ExportSession::begin(&input).map_err(|e| e.to_string())?
        };
        let pages = session.page_count();
        Ok((
            Self {
                built,
                fonts,
                palette,
                cmm,
                cmyk_working,
                output_intent,
                inks,
                options,
                doc_bleed,
                doc_slug,
                session,
            },
            pages,
        ))
    }

    /// Export ONE page. Returns (done, total).
    pub fn export_next_page(&mut self) -> Result<(usize, usize), String> {
        let input = ExportInput {
            doc: &self.built,
            palette: &self.palette,
            fonts: Some(&self.fonts),
            cmm: &self.cmm,
            profiles: ExportProfiles {
                cmyk_working: self.cmyk_working.as_deref(),
                output_intent: self.output_intent.as_deref(),
                srgb: None,
            },
            inks: self.inks.clone(),
            options: self.options.clone(),
            doc_bleed: self.doc_bleed,
            doc_slug: self.doc_slug,
        };
        self.session
            .export_next_page(&input)
            .map_err(|e| e.to_string())?;
        Ok((self.session.pages_done(), self.session.page_count()))
    }

    pub fn pages_done(&self) -> usize {
        self.session.pages_done()
    }

    pub fn page_count(&self) -> usize {
        self.session.page_count()
    }

    pub fn pages_remaining(&self) -> usize {
        self.session.pages_remaining()
    }

    /// Serialise the finished document. Consumes the session. Returns
    /// the PDF bytes, the human-readable summary lines, and the
    /// structured preflight findings (panels.md gap 20).
    pub fn finish(self) -> Result<FinishedExport, String> {
        let input = ExportInput {
            doc: &self.built,
            palette: &self.palette,
            fonts: Some(&self.fonts),
            cmm: &self.cmm,
            profiles: ExportProfiles {
                cmyk_working: self.cmyk_working.as_deref(),
                output_intent: self.output_intent.as_deref(),
                srgb: None,
            },
            inks: self.inks.clone(),
            options: self.options.clone(),
            doc_bleed: self.doc_bleed,
            doc_slug: self.doc_slug,
        };
        let result = self.session.finish(&input).map_err(|e| e.to_string())?;
        // Human-readable summary lines (the dialog's status line).
        let diagnostics = result.findings.iter().map(|f| f.message.clone()).collect();
        // panels.md gap 20 — structured findings for the grouped list.
        let findings = result
            .findings
            .iter()
            .map(|f| crate::channel::PreflightFinding {
                code: f.code.to_string(),
                severity: f.severity.as_str().to_string(),
                message: f.message.clone(),
                page_index: f.page_index.map(|p| p as u32),
            })
            .collect();
        Ok(FinishedExport {
            pdf_bytes: result.bytes,
            diagnostics,
            findings,
        })
    }
}

/// ADR 171 — the document's bookmarks as PDF outline items, in document
/// order: each nested under its parent (`bookmark_parents`) and going to
/// the page its destination names (a page destination's page; a text or
/// paragraph destination's paragraph as laid out, else its story's first
/// page). A bookmark whose destination resolves to no page is left out.
pub(crate) fn pdf_bookmarks(
    scene: &paged_scene::Document,
    built: &paged_renderer::BuiltDocument,
) -> Vec<paged_export_pdf::PdfBookmark> {
    let dm = &scene.designmap;
    let laid_out = built.paragraph_pages();
    let page_of_id: HashMap<&str, usize> = built
        .pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    let mut anchors: HashMap<&str, (&str, usize)> = HashMap::new();
    for s in &scene.stories {
        for d in &s.story.destinations {
            anchors.insert(
                d.self_id.as_str(),
                (s.self_id.as_str(), d.paragraph as usize),
            );
        }
    }
    let story_first_page = |story: &str| -> Option<usize> {
        laid_out.get(story).and_then(|m| m.values().min().copied())
    };
    let page_of = |dest: &str| -> Option<usize> {
        if let Some((story, para)) = anchors.get(dest) {
            return laid_out
                .get(*story)
                .and_then(|m| m.get(para).copied())
                .or_else(|| story_first_page(story));
        }
        match &dm
            .hyperlink_destinations
            .iter()
            .find(|d| d.self_id == dest)?
            .kind
        {
            paged_model::HyperlinkDestinationKind::Page(page) => {
                page_of_id.get(page.as_str()).copied()
            }
            paged_model::HyperlinkDestinationKind::TextAnchor(story) => story_first_page(story),
            paged_model::HyperlinkDestinationKind::Url(_) => None,
        }
    };
    let depth_of = |id: &str| -> u32 {
        let mut depth = 0;
        let mut at = dm.bookmark_parents.get(id);
        while let Some(parent) = at {
            depth += 1;
            at = dm.bookmark_parents.get(parent);
        }
        depth
    };
    dm.bookmarks
        .iter()
        .filter_map(|b| {
            let page_index = b.destination.as_deref().and_then(page_of)?;
            Some(paged_export_pdf::PdfBookmark {
                title: b.name.clone().unwrap_or_default(),
                depth: depth_of(&b.self_id),
                page_index,
            })
        })
        .collect()
}

fn wire_to_options(wire: &ExportPdfWireOptions) -> Result<ExportOptions, String> {
    let standard = match wire.standard.as_deref() {
        None | Some("pdf17") => PdfStandard::Pdf17,
        Some("pdfx4") => PdfStandard::PdfX4,
        Some(other) => return Err(format!("unknown standard: {other}")),
    };
    let color_policy = match wire.color_policy.as_deref() {
        None | Some("preserveNumbers") => ExportColorPolicy::PreserveNumbers,
        Some("convertToDestination") => ExportColorPolicy::ConvertToDestination,
        Some(other) => return Err(format!("unknown color policy: {other}")),
    };
    let restricted_fonts = match wire.restricted_font_policy.as_deref() {
        None | Some("outline") => RestrictedFontPolicy::Outline,
        Some("fail") => RestrictedFontPolicy::Fail,
        Some(other) => return Err(format!("unknown restricted-font policy: {other}")),
    };
    let page_range = match (wire.page_from, wire.page_to) {
        (Some(from), Some(to)) => Some((from as usize, to as usize)),
        (Some(from), None) => Some((from as usize, from as usize)),
        (None, Some(to)) => Some((0, to as usize)),
        (None, None) => None,
    };
    Ok(ExportOptions {
        bookmarks: Vec::new(),
        standard,
        color_policy,
        output_condition: wire.output_condition.clone(),
        page_range,
        marks: MarkOptions {
            crop_marks: wire.crop_marks,
            registration_marks: wire.registration_marks,
            color_bars: wire.color_bars,
            page_info: wire.page_info,
            offset_pt: wire.marks_offset_pt.unwrap_or(0.0),
            weight_pt: 0.0, // 0 ⇒ the exporter's default hairline
        },
        bleed: BleedOptions {
            override_pt: wire.bleed_override_pt,
        },
        images: ImageOptions {
            downsample_ppi: wire.downsample_ppi,
            jpeg_quality: None,
        },
        restricted_fonts,
        effect_dpi: wire.effect_dpi.unwrap_or(150.0),
        title: wire.title.clone(),
    })
}
