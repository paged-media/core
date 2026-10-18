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
    /// ADR 182 — the page's marked content by MCID: the paragraph each
    /// marked text object belongs to. Empty unless the export is tagged.
    pub mcids: Vec<paged_compose::StructKey>,
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
    /// R-10 — a transparency group with IDML's `IsolateBlending` /
    /// `KnockoutGroup`: `/I true` and / or `/K true` on the group
    /// dictionary, blending colour space inherited.
    TransparencyFlags { isolated: bool, knockout: bool },
    /// Isolated DeviceGray group (luminosity soft masks).
    LuminosityGray,
    /// C-28 — isolated transparency group whose ALPHA channel drives
    /// an `/S /Alpha` soft mask. No `/CS` is written: for an alpha
    /// mask the group's colour is irrelevant by definition, and PDF
    /// 32000-1 §11.6.5.2 only requires the blending colour space for
    /// luminosity masks.
    AlphaGroup,
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
    /// Shading patterns — a gradient used as a STROKE colour (PDF has
    /// no shading stroke operator; the gradient rides the `/Pattern`
    /// colour space instead).
    pub patterns: BTreeMap<String, Ref>,
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

        // ADR 182 — the logical structure, before the page dicts that
        // point into it (`/StructParents`).
        let structure = if input.options.tagged {
            Some(write_structure(
                &mut self.pdf,
                &mut self.refs,
                &self.pages,
                input,
            ))
        } else {
            None
        };

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
                if !page.resources.patterns.is_empty() {
                    let mut d = res.patterns();
                    for (name, r) in &page.resources.patterns {
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
            if structure.is_some() {
                p.struct_parents(page_idx as i32);
            }
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

        // ADR 171 — the bookmarks, as `/Outlines`.
        let first_page = input.options.page_range.map_or(0, |(from, _)| from);
        let outlines_ref = write_outlines(
            &mut self.pdf,
            &mut self.refs,
            &input.options.bookmarks,
            &kids,
            first_page,
        );

        // Catalog.
        {
            let mut catalog = self.pdf.catalog(self.catalog_ref);
            catalog.pages(self.page_tree_ref);
            if let Some(o) = outlines_ref {
                catalog.outlines(o);
            }
            if let Some(oi) = output_intent_ref {
                catalog.insert(Name(b"OutputIntents")).array().item(oi);
            }
            if let Some(m) = metadata_ref {
                catalog.metadata(m);
            }
            if let Some(root) = structure {
                catalog.pair(Name(b"StructTreeRoot"), root);
                catalog.mark_info().marked(true);
                if let Some(lang) = &input.options.lang {
                    catalog.lang(pdf_writer::TextStr(lang));
                }
                catalog.viewer_preferences().display_doc_title(true);
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

/// ADR 182 — the logical structure InDesign writes into a tagged PDF
/// (asked of InDesign 20.0.1, `tagged-pdf-probe` + `pdf-structure.py`):
/// a `Document` holding one `Art`, one `Sect` per text story in document
/// order, and one `P` per paragraph in story order, each paragraph
/// referencing its text's marked content on every page it crosses. The
/// parent tree maps each page's MCIDs back to their paragraphs. Returns
/// the structure tree root.
fn write_structure(
    pdf: &mut Pdf,
    refs: &mut RefAllocator,
    pages: &[FinishedPage],
    input: &ExportInput<'_>,
) -> Ref {
    use pdf_writer::types::StructRole;
    // InDesign names each paragraph after its style and its containers
    // `Article` and `Story`, and role-maps the names to standard types
    // (ADR 182; `tagged-styles-probe`).
    let names = &input.doc.structure_names;
    let name_of = |key: &paged_compose::StructKey| -> Option<&str> {
        names
            .get(key.story as usize)
            .and_then(|s| s.get(key.paragraph as usize))
            .map(String::as_str)
            .filter(|n| !n.is_empty())
    };
    let mut roles: BTreeMap<String, StructRole> = BTreeMap::new();
    use std::collections::BTreeMap;
    // Paragraphs in order, each with its (page, mcid) marks.
    let mut paragraphs: BTreeMap<paged_compose::StructKey, Vec<(usize, i32)>> = BTreeMap::new();
    for (page_idx, page) in pages.iter().enumerate() {
        for (mcid, key) in page.mcids.iter().enumerate() {
            paragraphs
                .entry(*key)
                .or_default()
                .push((page_idx, mcid as i32));
        }
    }
    let root_ref = refs.alloc();
    let document_ref = refs.alloc();
    let article_ref = refs.alloc();
    // Each story paragraph: its own marks and, when it holds a table, its
    // cell paragraphs' (ADR 182 amended, `tagged-tables-probe`: InDesign
    // sets them as `Table` > `TR` > `TH`/`TD` > `P` under the paragraph).
    type CellMarks = BTreeMap<(u32, u32, u32, bool), Vec<(usize, i32)>>;
    let mut hosts: BTreeMap<(u32, u32, u32), (Vec<(usize, i32)>, CellMarks)> = BTreeMap::new();
    for (key, marks) in &paragraphs {
        let host = hosts
            .entry((key.story, key.paragraph, key.sub))
            .or_default();
        match key.cell {
            None => host.0.extend(marks.iter().copied()),
            Some(c) => host
                .1
                .entry((c.row, c.col, c.paragraph, c.header))
                .or_default()
                .extend(marks.iter().copied()),
        }
    }
    let mut stories: BTreeMap<u32, (Ref, Vec<Ref>)> = BTreeMap::new();
    let mut para_refs: BTreeMap<paged_compose::StructKey, Ref> = BTreeMap::new();
    let mut host_refs: BTreeMap<(u32, u32, u32), Ref> = BTreeMap::new();
    for &(story, paragraph, sub) in hosts.keys() {
        let entry = stories
            .entry(story)
            .or_insert_with(|| (refs.alloc(), Vec::new()));
        let r = refs.alloc();
        entry.1.push(r);
        host_refs.insert((story, paragraph, sub), r);
    }
    // Write one element's marked content as its children.
    fn write_marks(
        kids: &mut pdf_writer::writers::StructChildren<'_>,
        marks: &[(usize, i32)],
        pages: &[FinishedPage],
    ) {
        for &(page_idx, mcid) in marks {
            if Some(page_idx) == marks.first().map(|m| m.0) {
                kids.marked_content_id(mcid);
            } else {
                kids.marked_content_ref()
                    .page(pages[page_idx].page_ref)
                    .marked_content_id(mcid);
            }
        }
    }
    for (&(story, paragraph, sub), (own, cells)) in &hosts {
        let host_ref = host_refs[&(story, paragraph, sub)];
        let story_ref = stories[&story].0;
        let host_key = paged_compose::StructKey {
            story,
            paragraph,
            sub,
            cell: None,
        };
        para_refs.insert(host_key, host_ref);
        // The table, its rows, cells and cell paragraphs.
        let table_ref = (!cells.is_empty()).then(|| refs.alloc());
        if let Some(table_ref) = table_ref {
            type Cell = (bool, Vec<(u32, Vec<(usize, i32)>)>);
            let mut rows: BTreeMap<u32, BTreeMap<u32, Cell>> = BTreeMap::new();
            for (&(row, col, cpara, header), marks) in cells {
                let cell = rows.entry(row).or_default().entry(col).or_default();
                cell.0 = header;
                cell.1.push((cpara, marks.clone()));
            }
            let row_refs: Vec<Ref> = rows.keys().map(|_| refs.alloc()).collect();
            {
                let mut t = pdf.struct_element(table_ref);
                t.kind(StructRole::Table);
                t.parent(host_ref);
                let mut c = t.children();
                for rr in &row_refs {
                    c.struct_element(*rr);
                }
            }
            for ((&row, cols), &row_ref) in rows.iter().zip(&row_refs) {
                let cell_refs: Vec<Ref> = cols.keys().map(|_| refs.alloc()).collect();
                {
                    let mut tr = pdf.struct_element(row_ref);
                    tr.kind(StructRole::TR);
                    tr.parent(table_ref);
                    let mut c = tr.children();
                    for cr in &cell_refs {
                        c.struct_element(*cr);
                    }
                }
                for ((&col, (header, paras)), &cell_ref) in cols.iter().zip(&cell_refs) {
                    let p_refs: Vec<Ref> = paras.iter().map(|_| refs.alloc()).collect();
                    {
                        let mut td = pdf.struct_element(cell_ref);
                        td.kind(if *header {
                            StructRole::TH
                        } else {
                            StructRole::TD
                        });
                        td.parent(row_ref);
                        let mut c = td.children();
                        for pr in &p_refs {
                            c.struct_element(*pr);
                        }
                    }
                    for ((cpara, marks), &pr) in paras.iter().zip(&p_refs) {
                        para_refs.insert(
                            paged_compose::StructKey {
                                story,
                                paragraph,
                                sub,
                                cell: Some(paged_compose::CellKey {
                                    row,
                                    col,
                                    paragraph: *cpara,
                                    header: *header,
                                }),
                            },
                            pr,
                        );
                        let mut e = pdf.struct_element(pr);
                        e.kind(StructRole::P);
                        e.parent(cell_ref);
                        if let Some(&(first, _)) = marks.first() {
                            e.page(pages[first].page_ref);
                        }
                        let mut kids = e.children();
                        write_marks(&mut kids, marks, pages);
                    }
                }
            }
        }
        let mut e = pdf.struct_element(host_ref);
        match name_of(&host_key) {
            Some(name) => {
                e.custom_kind(Name(name.as_bytes()));
                let role = match input.doc.structure_roles.get(name).map(String::as_str) {
                    Some("H") => StructRole::StructuredHeading,
                    Some("H1") => StructRole::H1,
                    Some("H2") => StructRole::H2,
                    Some("H3") => StructRole::H3,
                    Some("H4") => StructRole::H4,
                    Some("H5") => StructRole::H5,
                    Some("H6") => StructRole::H6,
                    _ => StructRole::P,
                };
                roles.insert(name.to_string(), role);
            }
            None => {
                e.kind(StructRole::P);
            }
        }
        e.parent(story_ref);
        if let Some(&(first, _)) = own.first() {
            e.page(pages[first].page_ref);
        }
        let mut kids = e.children();
        write_marks(&mut kids, own, pages);
        if let Some(t) = table_ref {
            kids.struct_element(t);
        }
    }
    for (story_ref, kids) in stories.values() {
        let mut e = pdf.struct_element(*story_ref);
        e.custom_kind(Name(b"Story"));
        e.parent(article_ref);
        let mut c = e.children();
        for k in kids {
            c.struct_element(*k);
        }
    }
    {
        let mut e = pdf.struct_element(article_ref);
        e.custom_kind(Name(b"Article"));
        e.parent(document_ref);
        let mut c = e.children();
        for (story_ref, _) in stories.values() {
            c.struct_element(*story_ref);
        }
    }
    {
        let mut e = pdf.struct_element(document_ref);
        e.kind(StructRole::Document);
        e.parent(root_ref);
        e.children().struct_element(article_ref);
    }
    // Each page's parent-tree entry: its MCIDs' paragraphs, in MCID order.
    let parent_arrays: Vec<(usize, Ref)> = pages
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.mcids.is_empty())
        .map(|(i, _)| (i, refs.alloc()))
        .collect();
    {
        let mut root = pdf
            .indirect(root_ref)
            .start::<pdf_writer::writers::StructTreeRoot>();
        root.child(document_ref);
        {
            let mut map = root.role_map();
            map.insert(Name(b"Article"), StructRole::Art);
            map.insert(Name(b"Story"), StructRole::Sect);
            for (name, role) in &roles {
                map.insert(Name(name.as_bytes()), *role);
            }
        }
        {
            let mut tree = root.parent_tree();
            let mut nums = tree.nums();
            for &(page_idx, arr_ref) in &parent_arrays {
                nums.insert(page_idx as i32, arr_ref);
            }
        }
        root.parent_tree_next_key(pages.len() as i32);
    }
    for &(page_idx, arr_ref) in &parent_arrays {
        pdf.indirect(arr_ref)
            .array()
            .items(pages[page_idx].mcids.iter().map(|k| para_refs[k]));
    }
    root_ref
}

/// Write `bookmarks` as the document outline (ADR 171): every item nested
/// under the nearest earlier item one level up, its `/Count` its
/// descendants (all open, as InDesign writes them), its destination its
/// page fitted (`/Fit`). An item whose page is not exported is left out
/// with its descendants. `None` when nothing is written.
fn write_outlines(
    pdf: &mut Pdf,
    refs: &mut RefAllocator,
    bookmarks: &[crate::PdfBookmark],
    kids: &[Ref],
    first_page: usize,
) -> Option<Ref> {
    // The items that land on an exported page, with their parent (an
    // index into `items`), depth-first in document order.
    let mut items: Vec<(&crate::PdfBookmark, Option<usize>, Ref)> = Vec::new();
    let mut stack: Vec<(u32, usize)> = Vec::new();
    let mut skip_below: Option<u32> = None;
    for b in bookmarks {
        if skip_below.is_some_and(|d| b.depth > d) {
            continue;
        }
        skip_below = None;
        let page = b
            .page_index
            .checked_sub(first_page)
            .and_then(|i| kids.get(i));
        if page.is_none() {
            skip_below = Some(b.depth);
            continue;
        }
        while stack.last().is_some_and(|(d, _)| *d >= b.depth) {
            stack.pop();
        }
        let parent = stack.last().map(|(_, i)| *i);
        items.push((b, parent, refs.alloc()));
        stack.push((b.depth, items.len() - 1));
    }
    if items.is_empty() {
        return None;
    }
    let root = refs.alloc();
    let children = |parent: Option<usize>| -> Vec<usize> {
        (0..items.len()).filter(|&i| items[i].1 == parent).collect()
    };
    let descendants = |i: usize| -> i32 {
        let mut n = 0;
        let mut open = vec![i];
        while let Some(at) = open.pop() {
            for (k, item) in items.iter().enumerate() {
                if item.1 == Some(at) {
                    n += 1;
                    open.push(k);
                }
            }
        }
        n
    };
    let top = children(None);
    {
        let mut outline = pdf.outline(root);
        outline.first(items[top[0]].2);
        outline.last(items[*top.last().expect("non-empty")].2);
        outline.count(items.len() as i32);
    }
    for (i, (b, parent, id)) in items.iter().enumerate() {
        let siblings = children(*parent);
        let at = siblings.iter().position(|&s| s == i).expect("listed");
        let kids_of = children(Some(i));
        let page = kids[b.page_index - first_page];
        let mut item = pdf.outline_item(*id);
        item.title(pdf_writer::TextStr(&b.title));
        item.parent(parent.map_or(root, |p| items[p].2));
        if at > 0 {
            item.prev(items[siblings[at - 1]].2);
        }
        if let Some(next) = siblings.get(at + 1) {
            item.next(items[*next].2);
        }
        if let (Some(first), Some(last)) = (kids_of.first(), kids_of.last()) {
            item.first(items[*first].2);
            item.last(items[*last].2);
            item.count(descendants(i));
        }
        item.dest().page(page).fit();
    }
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_allocator_hands_out_dense_ids_from_one() {
        // The whole xref discipline rests on this: ids strictly
        // sequential from 1 in call order, so the same input yields
        // the same object numbering (and a dense xref table).
        let mut a = RefAllocator::new();
        assert_eq!(a.alloc(), Ref::new(1));
        assert_eq!(a.alloc(), Ref::new(2));
        assert_eq!(a.alloc(), Ref::new(3));

        // Default is the same as new() — no alternate starting point.
        let mut d = RefAllocator::default();
        assert_eq!(d.alloc(), Ref::new(1));
    }
}
