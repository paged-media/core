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
        for page in &self.pages {
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
                let mut oi = self.pdf.indirect(oi_ref).start::<pdf_writer::writers::OutputIntent>();
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
