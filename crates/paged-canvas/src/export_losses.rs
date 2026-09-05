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

//! MEASURED IDML export losses — the twin diff behind
//! `CanvasModel::idml_export_losses`.
//!
//! # Why measured, not declared
//!
//! The loss list used to be a DECLARATION: it named the one construct
//! IDML cannot carry (opacity masks) and nothing else. Real InDesign
//! 2025 then opened a 134-page book the engine had authored and showed
//! empty frames where seven runtime tables and fourteen placed images
//! were, no sections, no guide, no hyperlinks — while the loss list was
//! empty and the harness's "no unexpected loss" gate was green. A
//! declaration is only as complete as the last person who edited it.
//!
//! So the list is now MEASURED: the document is exported through the
//! same writer a save uses, the bytes are re-parsed through the same
//! importer a load uses, and the re-parsed twin is compared against the
//! live scene construct by construct. Whatever the twin lacks is a loss,
//! named by element id, with the reason spelled out. When an exporter
//! lane lands (a `<Section>` emitter, say) the corresponding line simply
//! stops appearing — no ledger to keep in sync.
//!
//! The comparison is deliberately shallow: presence + the structural
//! shape a reader would notice (a table's rows×columns, a section's page
//! start, a condition's visibility). It is a save-time gate, not a
//! fidelity oracle; pixel parity is the fidelity harness's job.

use std::collections::{BTreeSet, HashMap, HashSet};

use paged_model::{
    GuideOrientation, HyperlinkDestinationKind, Paragraph, Section, Spread, Story, Table,
};
use paged_scene::{Document, ParsedStory};

/// Diff `scene` against `twin` (the scene as re-parsed from its own IDML
/// export). One human-readable line per element the twin lacks or
/// misrepresents, sorted for determinism.
pub(crate) fn diff(scene: &Document, twin: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    diff_sections(scene, twin, &mut out);
    diff_guides(scene, twin, &mut out);
    diff_hyperlinks(scene, twin, &mut out);
    diff_conditions(scene, twin, &mut out);
    diff_stories(scene, twin, &mut out);
    diff_page_items(scene, twin, &mut out);
    out.sort();
    out.dedup();
    out
}

// ---- sections -------------------------------------------------------------

fn diff_sections(scene: &Document, twin: &Document, out: &mut Vec<String>) {
    for s in &scene.designmap.sections {
        match twin
            .designmap
            .sections
            .iter()
            .find(|t| t.self_id == s.self_id)
        {
            None => out.push(format!(
                "section `{}` (PageStart `{}`) is missing from the exported designmap.xml — \
                 the IDML exporter does not yet serialise a <Section> the source archive lacks",
                s.self_id,
                s.page_start.as_deref().unwrap_or("?"),
            )),
            Some(t) => {
                let stale = section_differences(s, t);
                if !stale.is_empty() {
                    out.push(format!(
                        "section `{}` exports with stale attributes ({}) — the IDML exporter \
                         does not yet patch a changed <Section>",
                        s.self_id,
                        stale.join(", "),
                    ));
                }
            }
        }
    }
}

fn section_differences(s: &Section, t: &Section) -> Vec<String> {
    let mut d = Vec::new();
    if s.page_start != t.page_start {
        d.push(format!(
            "PageStart {:?} vs {:?}",
            s.page_start.as_deref().unwrap_or(""),
            t.page_start.as_deref().unwrap_or("")
        ));
    }
    if s.numbering_style != t.numbering_style {
        d.push(format!(
            "PageNumberStyle {:?} vs {:?}",
            s.numbering_style, t.numbering_style
        ));
    }
    if s.start_at != t.start_at {
        d.push(format!(
            "PageNumberStart {:?} vs {:?}",
            s.start_at, t.start_at
        ));
    }
    if s.continue_numbering != t.continue_numbering {
        d.push(format!(
            "ContinueNumbering {} vs {}",
            s.continue_numbering, t.continue_numbering
        ));
    }
    if s.section_prefix.as_deref().unwrap_or("") != t.section_prefix.as_deref().unwrap_or("") {
        d.push(format!(
            "SectionPrefix {:?} vs {:?}",
            s.section_prefix.as_deref().unwrap_or(""),
            t.section_prefix.as_deref().unwrap_or("")
        ));
    }
    if s.include_prefix != t.include_prefix {
        d.push(format!(
            "IncludeSectionPrefix {} vs {}",
            s.include_prefix, t.include_prefix
        ));
    }
    d
}

// ---- guides ---------------------------------------------------------------

fn twin_spread<'a>(twin: &'a Document, src: &str, self_id: Option<&str>) -> Option<&'a Spread> {
    twin.spreads
        .iter()
        .find(|p| p.src == src)
        .or_else(|| {
            self_id.and_then(|id| {
                twin.spreads
                    .iter()
                    .find(|p| p.spread.self_id.as_deref() == Some(id))
            })
        })
        .map(|p| &p.spread)
}

fn guide_key(g: &paged_model::RulerGuide) -> (u8, i64, u32) {
    let o = match g.orientation {
        GuideOrientation::Vertical => 0,
        GuideOrientation::Horizontal => 1,
    };
    // Compare at 1/1000 pt — the writer's `format_f32` precision.
    ((o), (g.location * 1000.0).round() as i64, g.page_index)
}

fn guide_label(g: &paged_model::RulerGuide) -> String {
    format!(
        "{:?} at {}pt, page index {}",
        g.orientation, g.location, g.page_index
    )
}

fn diff_guides(scene: &Document, twin: &Document, out: &mut Vec<String>) {
    for parsed in &scene.spreads {
        let Some(t) = twin_spread(twin, &parsed.src, parsed.spread.self_id.as_deref()) else {
            if !parsed.spread.guides.is_empty() {
                out.push(format!(
                    "spread `{}` is missing from the export — its {} guide(s) are lost with it",
                    parsed.src,
                    parsed.spread.guides.len()
                ));
            }
            continue;
        };
        // Multiset match: identical guides are indistinguishable, and the
        // model addresses guides by position only.
        let mut remaining: Vec<(u8, i64, u32)> = t.guides.iter().map(guide_key).collect();
        for g in &parsed.spread.guides {
            let key = guide_key(g);
            if let Some(pos) = remaining.iter().position(|k| *k == key) {
                remaining.swap_remove(pos);
            } else {
                out.push(format!(
                    "guide ({}) on spread `{}` is missing from the export — the IDML exporter \
                     does not yet serialise <Guide> elements the source spread lacks",
                    guide_label(g),
                    parsed.src
                ));
            }
        }
        for key in remaining {
            if let Some(g) = t.guides.iter().find(|g| guide_key(g) == key) {
                out.push(format!(
                    "guide ({}) on spread `{}` was removed from the model but the export still \
                     carries it — the IDML exporter does not yet drop deleted <Guide> elements",
                    guide_label(g),
                    parsed.src
                ));
            }
        }
    }
}

// ---- hyperlinks -----------------------------------------------------------

fn destination_label(k: &HyperlinkDestinationKind) -> String {
    match k {
        HyperlinkDestinationKind::Url(u) => format!("URL `{u}`"),
        HyperlinkDestinationKind::Page(p) => format!("page `{p}`"),
        HyperlinkDestinationKind::TextAnchor(t) => format!("text anchor `{t}`"),
    }
}

/// Every `hyperlink_source` id tagged on any run of `doc` (top-level
/// story paragraphs + table-cell paragraphs).
fn run_sources(doc: &Document) -> HashSet<String> {
    fn walk(paras: &[Paragraph], out: &mut HashSet<String>) {
        for p in paras {
            for r in &p.runs {
                if let Some(s) = &r.hyperlink_source {
                    out.insert(s.clone());
                }
            }
            if let Some(t) = &p.table {
                for c in &t.cells {
                    walk(&c.paragraphs, out);
                }
            }
        }
    }
    let mut out = HashSet::new();
    for s in &doc.stories {
        walk(&s.story.paragraphs, &mut out);
    }
    out
}

fn diff_hyperlinks(scene: &Document, twin: &Document, out: &mut Vec<String>) {
    let scene_sources = run_sources(scene);
    let twin_sources = run_sources(twin);
    for d in &scene.designmap.hyperlink_destinations {
        if !twin
            .designmap
            .hyperlink_destinations
            .iter()
            .any(|t| t.self_id == d.self_id)
        {
            out.push(format!(
                "hyperlink destination `{}` ({}) is missing from the exported designmap.xml — \
                 the IDML exporter does not yet serialise a <Hyperlink*Destination> the source \
                 archive lacks",
                d.self_id,
                destination_label(&d.kind)
            ));
        }
    }
    for h in &scene.designmap.hyperlinks {
        match twin
            .designmap
            .hyperlinks
            .iter()
            .find(|t| t.self_id == h.self_id)
        {
            None => out.push(format!(
                "hyperlink `{}` (source `{}`, destination `{}`) is missing from the exported \
                 designmap.xml — the IDML exporter does not yet serialise a <Hyperlink> the \
                 source archive lacks",
                h.self_id,
                h.source.as_deref().unwrap_or("?"),
                h.destination.as_deref().unwrap_or("?"),
            )),
            Some(t) => {
                if h.destination != t.destination {
                    out.push(format!(
                        "hyperlink `{}` exports pointing at `{}` instead of `{}` — the IDML \
                         exporter does not yet patch a changed <Hyperlink>",
                        h.self_id,
                        t.destination.as_deref().unwrap_or(""),
                        h.destination.as_deref().unwrap_or(""),
                    ));
                }
            }
        }
        if let Some(src) = &h.source {
            if scene_sources.contains(src) && !twin_sources.contains(src) {
                out.push(format!(
                    "hyperlink `{}`'s text source `{}` wraps no run in the exported stories — \
                     the IDML exporter does not yet serialise <HyperlinkTextSource> around the \
                     linked run",
                    h.self_id, src
                ));
            }
        }
    }
}

// ---- conditions -----------------------------------------------------------

fn diff_conditions(scene: &Document, twin: &Document, out: &mut Vec<String>) {
    for (id, c) in &scene.styles.conditions {
        match twin.styles.conditions.get(id) {
            None => out.push(format!(
                "condition `{}` (`{}`) is missing from the export — the IDML exporter does not \
                 yet serialise a <Condition> the source archive lacks",
                id,
                c.name.as_deref().unwrap_or("")
            )),
            Some(t) => {
                let want = c.visible.unwrap_or(true);
                let got = t.visible.unwrap_or(true);
                if want != got {
                    out.push(format!(
                        "condition `{id}` exports Visible=\"{got}\" but the model says \
                         Visible=\"{want}\" — the IDML exporter does not yet patch a changed \
                         <Condition>"
                    ));
                }
            }
        }
    }
    for id in scene.styles.condition_sets.keys() {
        if !twin.styles.condition_sets.contains_key(id) {
            out.push(format!(
                "condition set `{id}` is missing from the export — the IDML exporter does not \
                 yet serialise a <ConditionSet> the source archive lacks"
            ));
        }
    }
}

// ---- stories: tables ------------------------------------------------------

/// The twin story for a scene story. A story minted post-parse is named
/// `Story/u<n>`; its entry is `Stories/Story_Story_u<n>.xml`, and
/// `derive_story_id` hands the reopened story the SANITIZED id
/// `Story_u<n>` — so the twin is matched on either spelling.
fn twin_story<'a>(twin: &'a Document, id: &str) -> Option<&'a ParsedStory> {
    let sanitized = id.replace('/', "_");
    twin.stories
        .iter()
        .find(|s| s.self_id == id)
        .or_else(|| twin.stories.iter().find(|s| s.self_id == sanitized))
}

/// Every table in a story, outermost first, nested (cell) tables after.
fn tables_of(story: &Story) -> Vec<&Table> {
    fn walk<'a>(paras: &'a [Paragraph], out: &mut Vec<&'a Table>) {
        for p in paras {
            if let Some(t) = &p.table {
                out.push(t);
            }
        }
        for p in paras {
            if let Some(t) = &p.table {
                for c in &t.cells {
                    walk(&c.paragraphs, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&story.paragraphs, &mut out);
    out
}

fn table_shape(t: &Table) -> (usize, usize) {
    let rows = if t.rows.is_empty() {
        (t.header_row_count + t.body_row_count + t.footer_row_count) as usize
    } else {
        t.rows.len()
    };
    let cols = if t.columns.is_empty() {
        t.column_count as usize
    } else {
        t.columns.len()
    };
    (rows, cols)
}

fn diff_stories(scene: &Document, twin: &Document, out: &mut Vec<String>) {
    for s in &scene.stories {
        let scene_tables = tables_of(&s.story);
        let Some(t) = twin_story(twin, &s.self_id) else {
            let text_chars: usize = s
                .story
                .paragraphs
                .iter()
                .flat_map(|p| p.runs.iter())
                .map(|r| r.text.chars().count())
                .sum();
            out.push(format!(
                "story `{}` is missing from the export entirely ({} characters, {} table(s)) — \
                 its text, tables and links are all lost",
                s.self_id,
                text_chars,
                scene_tables.len()
            ));
            continue;
        };
        let minted = s.src.is_empty();
        // The text itself, paragraph by paragraph: a run the exporter
        // re-serialised from the wrong model run, or a mark it dropped,
        // shows up here as a character-count / paragraph-count mismatch.
        // Empty paragraphs are left out: the parser drops a paragraph
        // with neither text nor table on read, so one the model still
        // holds (a frame's initial empty paragraph) is a read-side
        // asymmetry, not lost text.
        let paragraph_texts = |paras: &[Paragraph]| -> Vec<String> {
            paras
                .iter()
                .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
                .filter(|t| !t.is_empty())
                .collect()
        };
        let scene_paras = paragraph_texts(&s.story.paragraphs);
        let twin_paras = paragraph_texts(&t.story.paragraphs);
        if scene_paras != twin_paras {
            let scene_chars: usize = scene_paras.iter().map(|p| p.chars().count()).sum();
            let twin_chars: usize = twin_paras.iter().map(|p| p.chars().count()).sum();
            out.push(format!(
                "story `{}` exports with different text: {} paragraph(s) / {} character(s) in \
                 the model, {} / {} in the export — the IDML exporter re-serialised a run it \
                 could not match (a split run, a lost paragraph mark)",
                s.self_id,
                scene_paras.len(),
                scene_chars,
                twin_paras.len(),
                twin_chars
            ));
        }
        let twin_tables = tables_of(&t.story);
        let mut twin_by_id: HashMap<&str, &Table> = HashMap::new();
        for tt in &twin_tables {
            if let Some(id) = tt.self_id.as_deref() {
                twin_by_id.insert(id, tt);
            }
        }
        for (i, st) in scene_tables.iter().enumerate() {
            let id = st.self_id.clone().unwrap_or_else(|| format!("#{i}"));
            let (rows, cols) = table_shape(st);
            // By id when it has one; the i-th table otherwise.
            let twin_table = match st.self_id.as_deref() {
                Some(id) => twin_by_id.get(id).copied(),
                None => twin_tables.get(i).copied(),
            };
            match twin_table {
                None => out.push(format!(
                    "table `{}` ({}×{}) in story `{}` is missing from the export — the IDML \
                     exporter does not yet serialise a table inside a {}",
                    id,
                    rows,
                    cols,
                    s.self_id,
                    if minted {
                        "minted story"
                    } else {
                        "source story that does not carry it"
                    }
                )),
                Some(tt) => {
                    let (tr, tc) = table_shape(tt);
                    if (tr, tc) != (rows, cols) {
                        out.push(format!(
                            "table `{}` in story `{}` exports as {}×{} but the model has {}×{} \
                             — the IDML exporter does not yet patch table structure",
                            id, s.self_id, tr, tc, rows, cols
                        ));
                    }
                }
            }
        }
    }
}

// ---- page items: placed images + groups -----------------------------------

/// The image lanes of one graphic frame, whatever its shape.
struct ImageFrame<'a> {
    id: &'a str,
    bytes: Option<usize>,
    link: Option<&'a str>,
    has_image_element: bool,
}

fn image_frames(spread: &Spread) -> Vec<ImageFrame<'_>> {
    let mut out = Vec::new();
    for r in &spread.rectangles {
        if let Some(id) = r.self_id.as_deref() {
            out.push(ImageFrame {
                id,
                bytes: r.image_bytes.as_ref().map(|b| b.len()),
                link: r.image_link.as_deref(),
                has_image_element: r.has_image_element,
            });
        }
    }
    for o in &spread.ovals {
        if let Some(id) = o.self_id.as_deref() {
            out.push(ImageFrame {
                id,
                bytes: o.image_bytes.as_ref().map(|b| b.len()),
                link: o.image_link.as_deref(),
                has_image_element: o.has_image_element,
            });
        }
    }
    for p in &spread.polygons {
        if let Some(id) = p.self_id.as_deref() {
            out.push(ImageFrame {
                id,
                bytes: p.image_bytes.as_ref().map(|b| b.len()),
                link: p.image_link.as_deref(),
                has_image_element: p.has_image_element,
            });
        }
    }
    out
}

fn diff_page_items(scene: &Document, twin: &Document, out: &mut Vec<String>) {
    for parsed in &scene.spreads {
        let Some(t) = twin_spread(twin, &parsed.src, parsed.spread.self_id.as_deref()) else {
            continue;
        };
        let twin_frames: HashMap<&str, ImageFrame<'_>> =
            image_frames(t).into_iter().map(|f| (f.id, f)).collect();
        for f in image_frames(&parsed.spread) {
            if f.bytes.is_none() && f.link.is_none() {
                continue; // no image on this frame: nothing to lose
            }
            let Some(tf) = twin_frames.get(f.id) else {
                out.push(format!(
                    "page item `{}` on spread `{}` is missing from the export — the placed \
                     image it carried is lost with it",
                    f.id, parsed.src
                ));
                continue;
            };
            if let Some(n) = f.bytes {
                if tf.bytes.is_none() {
                    match (f.link, tf.link) {
                        (Some(uri), Some(_)) => out.push(format!(
                            "image bytes on `{}` ({} bytes) are not in the export — IDML cannot \
                             embed pixels; the frame links to `{}` instead, so InDesign re-reads \
                             the asset and any edit to the pixels is lost. Save as `.paged` to keep \
                             them",
                            f.id, n, uri
                        )),
                        _ => out.push(format!(
                            "image placed from bytes on `{}` ({} bytes) is not in the export — an \
                             image placed from bytes has no IDML link to point at (IDML cannot embed \
                             pixels), so the frame exports empty. Save as `.paged` to keep it, or \
                             write the bytes as a sibling file and link it",
                            f.id, n
                        )),
                    }
                }
            }
            if let Some(uri) = f.link {
                if tf.link != Some(uri) && !(tf.has_image_element && tf.bytes.is_some()) {
                    out.push(format!(
                        "image link `{}` on `{}` is missing from the export — the IDML exporter \
                         does not yet serialise <Image>/<Link> for a placed image",
                        uri, f.id
                    ));
                }
            }
        }
        // Groups.
        let twin_groups: BTreeSet<&str> = t
            .groups
            .iter()
            .filter_map(|g| g.self_id.as_deref())
            .collect();
        for g in &parsed.spread.groups {
            let Some(id) = g.self_id.as_deref() else {
                continue;
            };
            if !twin_groups.contains(id) {
                out.push(format!(
                    "group `{}` ({} member(s)) on spread `{}` is missing from the export — the \
                     IDML exporter does not yet serialise <Group> on a minted spread",
                    id,
                    g.members.len(),
                    parsed.src
                ));
            }
        }
    }
}

/// The opacity-mask lines — the one construct IDML genuinely cannot
/// carry. Text kept verbatim from the declared-loss era: the harness
/// matches on it.
pub(crate) fn opacity_mask_losses(scene: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for parsed in &scene.spreads {
        let mut ids: Vec<(&String, &paged_model::OpacityMask)> =
            parsed.spread.opacity_masks.iter().collect();
        // HashMap iteration order is not stable; sort so the
        // reported list is deterministic across runs.
        ids.sort_by(|a, b| a.0.cmp(b.0));
        for (target, mask) in ids {
            out.push(format!(
                "opacity mask on `{target}` (artwork `{}`) is a paged-native construct — \
                 IDML has no opacity-mask element, so the mask is dropped and its artwork \
                 exports as an ordinary item. Save as `.paged` to keep it.",
                mask.mask_item
            ));
        }
    }
    out
}
