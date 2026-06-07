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

//! Document-level writer state: deterministic ref allocation, the
//! page tree, interned resource pools (ExtGStates, colour spaces,
//! fonts, images, shadings, XObjects), and the finish path
//! (catalog, Info, OutputIntent, XMP).

use std::collections::BTreeMap;

use pdf_writer::{Finish, Name, Pdf, Rect, Ref};

use crate::text::FontPool;
use crate::{ExportDiagnostic, ExportError, ExportInput, PdfStandard};

/// Deterministic ref allocator — ids handed out strictly in
/// call order so the same input yields the same xref layout.
pub struct RefAllocator {
    next: i32,
}

impl Default for RefAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl RefAllocator {
    pub fn new() -> Self {
        Self { next: 1 }
    }
    pub fn alloc(&mut self) -> Ref {
        let r = Ref::new(self.next);
        self.next += 1;
        r
    }
}

/// One finished page's bookkeeping.
pub struct FinishedPage {
    pub page_ref: Ref,
    pub content_ref: Ref,
    pub media_box: Rect,
    pub trim_box: Rect,
    pub bleed_box: Rect,
    /// Resource names used on this page, mapped to object refs.
    pub resources: PageResources,
    /// The page's ONE indirect /Resources dictionary — shared by the
    /// page content stream and every transparency-group form
    /// captured from it (allocated before the walk, written at
    /// finish when the resource set is complete).
    pub resources_ref: Ref,
    /// W1.4 — Link annotations harvested from the display list's
    /// `LinkRegionTable`. Each is already in y-up media coords; the
    /// finish pass allocates one annotation object per entry and lists
    /// them in the page's `/Annots`. Empty when the build didn't
    /// collect link regions (the common case).
    pub link_annots: Vec<LinkAnnot>,
}

/// W1.4 — one resolved Link annotation, in PDF y-up media coords.
pub struct LinkAnnot {
    /// `[x0 y0 x1 y1]` annotation rect (media coords, y-up).
    pub rect: Rect,
    pub target: LinkAnnotTarget,
}

/// Where a [`LinkAnnot`] points.
pub enum LinkAnnotTarget {
    /// External URI — written as an `/Action /URI`.
    Uri(String),
    /// In-document page (flat 0-based body-page index) — written as a
    /// `/Action /GoTo` with a `/Fit` destination on that page.
    Page(u32),
}

/// A transparency-group / soft-mask Form XObject captured during the
/// page walk, written after the walk completes (it references the
/// page's shared /Resources by ref).
pub struct PendingForm {
    pub form_ref: Ref,
    pub data: Vec<u8>,
    pub bbox: Rect,
    pub group: PendingFormGroup,
}

pub enum PendingFormGroup {
    /// Non-isolated transparency group, blending colour space
    /// inherited from the parent (the blend/opacity groups).
    TransparencyInherit,
    /// Isolated DeviceGray group (luminosity soft masks).
    LuminosityGray,
}

/// Per-page resource dictionaries (name → object ref). Names are
/// allocated per page ("Gs0", "Cs0", "F0", "Im0", "Sh0", "Xo0")
/// in first-use order — deterministic.
#[derive(Default)]
pub struct PageResources {
    pub ext_g_states: BTreeMap<String, Ref>,
    pub color_spaces: BTreeMap<String, Ref>,
    pub fonts: BTreeMap<String, Ref>,
    pub x_objects: BTreeMap<String, Ref>,
    pub shadings: BTreeMap<String, Ref>,
}

/// Whole-document writer state, alive across the session.
pub struct DocState {
    pub pdf: Pdf,
    pub refs: RefAllocator,
    pub catalog_ref: Ref,
    pub page_tree_ref: Ref,
    pub pages: Vec<FinishedPage>,
    /// Interned ICC profile streams (key = which profile).
    pub icc_refs: BTreeMap<&'static str, Ref>,
    /// Interned ExtGState dicts keyed by their canonical encoding.
    pub gs_pool: BTreeMap<String, Ref>,
    /// Interned Separation colour spaces keyed by colorant name.
    pub separation_pool: BTreeMap<String, Ref>,
    /// The font subset pool (glyph usage accumulated across pages,
    /// written at finish).
    pub fonts: FontPool,
}

impl DocState {
    pub fn new(_input: &ExportInput<'_>) -> Self {
        let mut refs = RefAllocator::new();
        let catalog_ref = refs.alloc();
        let page_tree_ref = refs.alloc();
        Self {
            pdf: Pdf::new(),
            refs,
            catalog_ref,
            page_tree_ref,
            pages: Vec::new(),
            icc_refs: BTreeMap::new(),
            gs_pool: BTreeMap::new(),
            separation_pool: BTreeMap::new(),
            fonts: FontPool::default(),
        }
    }

    /// Write the trailing document-level objects and serialise.
    pub fn finish(
        mut self,
        input: &ExportInput<'_>,
        diagnostics: &mut Vec<ExportDiagnostic>,
    ) -> Result<Vec<u8>, ExportError> {
        // Fonts: subset + embed everything the pages used.
        let font_refs = crate::text::write_fonts(
            &mut self.pdf,
            &mut self.refs,
            &mut self.fonts,
            input,
            diagnostics,
        )?;
        // Patch font resource refs now that subsets exist.
        for page in &mut self.pages {
            for (name, slot) in page.resources.fonts.iter_mut() {
                if let Some(real) = font_refs.get(name) {
                    *slot = *real;
                }
            }
        }

        // Page objects.
        let kids: Vec<Ref> = self.pages.iter().map(|p| p.page_ref).collect();
        {
            let mut tree = self.pdf.pages(self.page_tree_ref);
            tree.kids(kids.iter().copied());
            tree.count(self.pages.len() as i32);
        }

        // W1.4 — Link annotations. Allocate one annotation object per
        // harvested region first (so we can list their refs in each
        // page's `/Annots`), then write the annotation dicts. GoTo
        // targets resolve to the target page object's `page_ref`, which
        // every page already owns. `page_ref` lookup is by flat index
        // (kids[i]); an out-of-range index drops the annotation rather
        // than emit a dangling /GoTo.
        let mut annots_per_page: Vec<Vec<Ref>> = Vec::with_capacity(self.pages.len());
        for page in &self.pages {
            let mut refs = Vec::with_capacity(page.link_annots.len());
            for _ in &page.link_annots {
                refs.push(self.refs.alloc());
            }
            annots_per_page.push(refs);
        }
        for (page_idx, page) in self.pages.iter().enumerate() {
            for (annot_idx, annot) in page.link_annots.iter().enumerate() {
                let annot_ref = annots_per_page[page_idx][annot_idx];
                let mut a = self.pdf.annotation(annot_ref);
                a.subtype(pdf_writer::types::AnnotationType::Link);
                a.rect(annot.rect);
                // No visible border (InDesign exports invisible link
                // hotspots) — a zero-width border array.
                a.insert(Name(b"Border")).array().items([0i32, 0, 0]);
                match &annot.target {
                    LinkAnnotTarget::Uri(url) => {
                        let mut action = a.action();
                        action.action_type(pdf_writer::types::ActionType::Uri);
                        action.uri(pdf_writer::Str(url.as_bytes()));
                    }
                    LinkAnnotTarget::Page(target_idx) => {
                        if let Some(target_page) = self.pages.get(*target_idx as usize) {
                            let target_ref = target_page.page_ref;
                            let mut action = a.action();
                            action.action_type(pdf_writer::types::ActionType::GoTo);
                            action.destination().page(target_ref).fit();
                        }
                    }
                }
            }
        }

        for (page_idx, page) in self.pages.iter().enumerate() {
            // The shared indirect /Resources dict (page + its forms).
            {
                let mut res = self
                    .pdf
                    .indirect(page.resources_ref)
                    .start::<pdf_writer::writers::Resources>();
                if !page.resources.ext_g_states.is_empty() {
                    let mut d = res.ext_g_states();
                    for (name, r) in &page.resources.ext_g_states {
                        d.pair(Name(name.as_bytes()), *r);
                    }
                }
                if !page.resources.color_spaces.is_empty() {
                    let mut d = res.color_spaces();
                    for (name, r) in &page.resources.color_spaces {
                        d.pair(Name(name.as_bytes()), *r);
                    }
                }
                if !page.resources.fonts.is_empty() {
                    let mut d = res.fonts();
                    for (name, r) in &page.resources.fonts {
                        d.pair(Name(name.as_bytes()), *r);
                    }
                }
                if !page.resources.x_objects.is_empty() {
                    let mut d = res.x_objects();
                    for (name, r) in &page.resources.x_objects {
                        d.pair(Name(name.as_bytes()), *r);
                    }
                }
                if !page.resources.shadings.is_empty() {
                    let mut d = res.shadings();
                    for (name, r) in &page.resources.shadings {
                        d.pair(Name(name.as_bytes()), *r);
                    }
                }
            }
            let mut p = self.pdf.page(page.page_ref);
            p.parent(self.page_tree_ref);
            p.media_box(page.media_box);
            p.trim_box(page.trim_box);
            p.bleed_box(page.bleed_box);
            p.crop_box(page.media_box);
            p.contents(page.content_ref);
            p.pair(Name(b"Resources"), page.resources_ref);
            // W1.4 — list this page's Link annotation objects.
            let annot_refs = &annots_per_page[page_idx];
            if !annot_refs.is_empty() {
                p.annotations(annot_refs.iter().copied());
            }
        }

        // OutputIntent (required for X-4; emitted whenever a
        // destination profile is supplied).
        let output_intent_ref = match input.profiles.output_intent {
            Some(bytes) => {
                let profile_ref = self.refs.alloc();
                let mut stream = self.pdf.icc_profile(profile_ref, bytes);
                stream.n(4);
                stream.finish();
                let oi_ref = self.refs.alloc();
                let condition = input
                    .options
                    .output_condition
                    .as_deref()
                    .unwrap_or("Custom");
                let mut oi = self
                    .pdf
                    .indirect(oi_ref)
                    .start::<pdf_writer::writers::OutputIntent>();
                oi.subtype(pdf_writer::types::OutputIntentSubtype::PDFX);
                oi.output_condition_identifier(pdf_writer::TextStr(condition));
                oi.output_condition(pdf_writer::TextStr(condition));
                oi.dest_output_profile(profile_ref);
                oi.finish();
                Some(oi_ref)
            }
            None => None,
        };

        // XMP metadata (X-4 conformance keys; deterministic ids).
        let metadata_ref = if input.options.standard == PdfStandard::PdfX4 {
            let xmp = crate::xmp_packet(input);
            let meta_ref = self.refs.alloc();
            let stream = self.pdf.metadata(meta_ref, xmp.as_bytes());
            stream.finish();
            Some(meta_ref)
        } else {
            None
        };

        // Catalog.
        {
            let mut catalog = self.pdf.catalog(self.catalog_ref);
            catalog.pages(self.page_tree_ref);
            if let Some(oi) = output_intent_ref {
                catalog.insert(Name(b"OutputIntents")).array().item(oi);
            }
            if let Some(m) = metadata_ref {
                catalog.metadata(m);
            }
        }

        // Info dict — deterministic (no wall-clock); Trapped is a
        // definite value (PDF/X requirement).
        {
            let info_ref = self.refs.alloc();
            let mut info = self.pdf.document_info(info_ref);
            if let Some(title) = &input.options.title {
                info.title(pdf_writer::TextStr(title));
            }
            info.creator(pdf_writer::TextStr("paged"));
            info.producer(pdf_writer::TextStr("paged-export-pdf"));
            if input.options.standard == PdfStandard::PdfX4 {
                info.trapped(pdf_writer::types::TrappingStatus::NotTrapped);
            }
            info.finish();
        }

        if input.options.standard == PdfStandard::PdfX4 {
            self.pdf.set_version(1, 6);
        } else {
            self.pdf.set_version(1, 7);
        }
        Ok(self.pdf.finish())
    }
}
