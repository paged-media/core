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

//! Fonts that arrive after the load. The model's font table used to be
//! built once, at load, from the families the stories asked for then, so
//! two things laid out in the default face whatever was registered:
//!
//! - a font registered while the document was open (`RegisterFont` was
//!   accepted and ignored until the next load), and
//! - a family an edit brought in (a Word document opened standalone is an
//!   empty skeleton; every family arrives with the pour), even when that
//!   family was registered before the load.
//!
//! Each case checks the incremental model against a cold load given the
//! same fonts up front: the page digests, the line geometry and the
//! diagnostics must be equal (the ADR 027 digest gate, whose cold build
//! now harvests its own font table).

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, FontEntry, Mutation};
use paged_mutate::{PropertyPath as P, Value as V};

fn corpus(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(rel)
}

fn inter() -> Vec<u8> {
    std::fs::read(corpus("fonts/Inter.ttf")).expect("read Inter.ttf")
}

/// Lora, as its own `name` table spells it (the same entry a host's font
/// scan registers).
fn lora() -> FontEntry {
    let mut entries = paged_canvas::font_registry_from_paths(&[corpus("fonts/Lora.ttf")]);
    assert_eq!(entries.len(), 1, "Lora.ttf scans to one face");
    let entry = entries.remove(0);
    assert_eq!(entry.family, "Lora");
    entry
}

/// `docx-pagination`: one story per section, every run in the styles'
/// Inter. With `lora_story`, that section's runs ask for Lora instead.
fn document(lora_story: Option<u32>) -> Vec<u8> {
    use paged_gen::samples::docx_pagination::{build, section_story_id};
    let idml = paged_gen::write_idml(&build()).expect("write_idml");
    let mut doc = idml_import::import_idml_doc(&idml).expect("import");
    if let Some(i) = lora_story {
        let id = section_story_id(i);
        let story = doc
            .stories
            .iter_mut()
            .find(|s| s.self_id == id)
            .expect("the section story");
        for p in &mut story.story.paragraphs {
            for r in &mut p.runs {
                r.font = Some("Lora".to_string());
                r.font_style = None;
            }
        }
    }
    paged_store::package::wrap_document(&doc, "Late.docx", 612.0, 792.0).expect("wrap")
}

fn load(bytes: &[u8], registry: Vec<FontEntry>) -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![inter()],
        font_registry: registry,
        ..CanvasOptions::default()
    };
    let m = CanvasModel::load("doc", bytes, opts).expect("load");
    m.digest_gate_check()
        .unwrap_or_else(|e| panic!("load is not a cold build: {e}"));
    m
}

/// Every line of a story: (paragraph, line, every cluster's x).
type Geometry = Vec<(u32, u32, Vec<u32>)>;

/// The line geometry of `story`, which a face change moves.
fn geometry(m: &CanvasModel, story: &str) -> Geometry {
    m.built()
        .story_layout(story)
        .iter()
        .map(|l| {
            (
                l.paragraph_idx,
                l.line_idx,
                l.clusters.iter().map(|c| c.x_pt.to_bits()).collect(),
            )
        })
        .collect()
}

/// The incremental model equals a cold load of its scene with `registry`
/// given up front: the same pages, digests, line geometry, diagnostics.
fn assert_equals_cold_load(m: &CanvasModel, registry: Vec<FontEntry>, what: &str) {
    m.digest_gate_check()
        .unwrap_or_else(|e| panic!("{what}: incremental != cold build: {e}"));
    let saved = paged_store::package::wrap_document(m.scene(), "Saved.docx", 612.0, 792.0)
        .expect("wrap the edited scene");
    let cold = load(&saved, registry);
    let digests = |m: &CanvasModel| -> Vec<(String, u64)> {
        m.built()
            .pages
            .iter()
            .map(|p| (p.id.0.clone(), p.list.digest()))
            .collect()
    };
    assert_eq!(digests(m), digests(&cold), "{what}: page digests");
    for parsed in &m.scene().stories {
        assert_eq!(
            geometry(m, &parsed.self_id),
            geometry(&cold, &parsed.self_id),
            "{what}: line geometry of {}",
            parsed.self_id
        );
    }
    assert_eq!(
        format!("{:?}", m.built().diagnostics.items),
        format!("{:?}", cold.built().diagnostics.items),
        "{what}: diagnostics"
    );
}

fn section(i: u32) -> String {
    paged_gen::samples::docx_pagination::section_story_id(i)
}

fn char_len(m: &CanvasModel, story: &str) -> u32 {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story")
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.chars().count() as u32)
        .sum()
}

#[test]
fn a_font_registered_after_load_lays_its_runs_out() {
    let bytes = document(Some(0));
    let mut m = load(&bytes, Vec::new());
    let story = section(0);
    let before = geometry(&m, &story);
    let others: Vec<(String, Geometry)> = m
        .scene()
        .stories
        .iter()
        .filter(|s| s.self_id != story)
        .map(|s| (s.self_id.clone(), geometry(&m, &s.self_id)))
        .collect();
    assert!(!before.is_empty(), "the Lora story is laid out");

    let affected = m.register_font(lora()).expect("register Lora");
    assert_eq!(
        affected,
        vec![story.clone()],
        "only the story asking for Lora is re-laid out"
    );
    assert_ne!(
        geometry(&m, &story),
        before,
        "the Lora story no longer lays out in the fallback face"
    );
    for (id, g) in &others {
        assert_eq!(&geometry(&m, id), g, "{id} does not ask for Lora");
    }
    assert_equals_cold_load(&m, vec![lora()], "late registration");

    // A second face nobody asks for changes nothing and rebuilds nothing.
    let rebuilds = m.last_rebuild_stats().rebuilds;
    let mut unused = lora();
    unused.family = "Nobody Uses This".to_string();
    assert!(m.register_font(unused).expect("register").is_empty());
    assert_eq!(m.last_rebuild_stats().rebuilds, rebuilds, "no rebuild");
}

#[test]
fn clearing_the_registry_returns_the_runs_to_the_fallback() {
    let bytes = document(Some(0));
    let fallback = geometry(&load(&bytes, Vec::new()), &section(0));
    let mut m = load(&bytes, vec![lora()]);
    assert_ne!(geometry(&m, &section(0)), fallback);
    let affected = m.clear_font_registry().expect("clear");
    assert_eq!(affected, vec![section(0)]);
    assert_eq!(geometry(&m, &section(0)), fallback);
    assert_equals_cold_load(&m, Vec::new(), "cleared registry");
}

/// The Word case: Lora is registered BEFORE the load, but no story asks
/// for it until an edit sets a range's family.
#[test]
fn an_edit_that_brings_in_a_registered_family_relays_the_range() {
    let bytes = document(None);
    let mut m = load(&bytes, vec![lora()]);
    let story = section(1);
    let before = geometry(&m, &story);
    let end = char_len(&m, &story);
    let set_family = |family: &str| Mutation::SetElementProperty {
        element_id: ElementId::StoryRange {
            story_id: story.clone(),
            start: 0,
            end,
        },
        path: P::CharacterFontFamily,
        value: V::Text(family.to_string()),
    };
    m.apply_mutation(&set_family("Lora"))
        .expect("set the family to Lora");
    let lora_geometry = geometry(&m, &story);
    assert_ne!(lora_geometry, before, "the range re-lays out in Lora");
    assert_equals_cold_load(&m, vec![lora()], "family edit");

    // Back to Inter (the fallback here) by edit: the layout returns.
    m.apply_mutation(&set_family("Inter"))
        .expect("set the family back");
    assert_eq!(geometry(&m, &story), before);
    assert_equals_cold_load(&m, vec![lora()], "family edit back");
}

#[test]
fn undo_and_redo_of_a_font_edit_stay_equal_to_a_cold_load() {
    let bytes = document(None);
    let mut m = load(&bytes, vec![lora()]);
    let story = section(0);
    let before = geometry(&m, &story);
    let end = char_len(&m, &story);
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::StoryRange {
            story_id: story.clone(),
            start: 0,
            end: end.min(40),
        },
        path: P::CharacterFontFamily,
        value: V::Text("Lora".to_string()),
    })
    .expect("set the family");
    let after = geometry(&m, &story);
    assert_ne!(after, before);

    m.undo().expect("undo");
    assert_eq!(geometry(&m, &story), before, "undo restores the layout");
    assert_equals_cold_load(&m, vec![lora()], "undo");

    m.redo().expect("redo");
    assert_eq!(geometry(&m, &story), after, "redo re-applies the face");
    assert_equals_cold_load(&m, vec![lora()], "redo");
}

/// A font registered after load, on a document whose fonts the edit
/// introduced first: the late face reaches the edited range too.
#[test]
fn a_late_font_reaches_a_family_an_edit_introduced() {
    let bytes = document(None);
    let mut m = load(&bytes, Vec::new());
    let story = section(1);
    let end = char_len(&m, &story);
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::StoryRange {
            story_id: story.clone(),
            start: 0,
            end,
        },
        path: P::CharacterFontFamily,
        value: V::Text("Lora".to_string()),
    })
    .expect("set the family");
    let substituted = geometry(&m, &story);
    assert_equals_cold_load(&m, Vec::new(), "edit, Lora not registered");
    let affected = m.register_font(lora()).expect("register");
    assert_eq!(affected, vec![story.clone()]);
    assert_ne!(geometry(&m, &story), substituted);
    assert_equals_cold_load(&m, vec![lora()], "late registration after edit");
}

/// The 134-page annual (not in this repository; `PAGED_LATE_FONTS_ANNUAL`
/// names its `.paged`): load it with every corpus face except JetBrains
/// Mono, register JetBrains Mono afterwards, and report what that costs and
/// how many stories it re-lays out. Run release: the numbers are the point.
#[test]
fn the_annual_late_registration_cost() {
    let Ok(path) = std::env::var("PAGED_LATE_FONTS_ANNUAL") else {
        eprintln!("PAGED_LATE_FONTS_ANNUAL unset: the annual measurement is skipped");
        return;
    };
    let bytes = std::fs::read(&path).expect("read the annual");
    let all = paged_canvas::font_registry_from_paths(&[corpus("fonts")]);
    let (late, early): (Vec<FontEntry>, Vec<FontEntry>) =
        all.into_iter().partition(|e| e.family == "JetBrains Mono");
    assert!(!late.is_empty(), "the corpus carries JetBrains Mono");
    let opts = |registry: Vec<FontEntry>| CanvasOptions {
        fonts: vec![inter()],
        font_registry: registry,
        cmyk_icc_profile: std::fs::read(corpus("profiles/default_cmyk.icc")).ok(),
        ..CanvasOptions::default()
    };
    let t = std::time::Instant::now();
    let mut m = CanvasModel::load("annual", &bytes, opts(early.clone())).expect("load");
    eprintln!(
        "annual: cold load {:.0} ms, {} pages, {} stories",
        t.elapsed().as_secs_f64() * 1e3,
        m.built().pages.len(),
        m.scene().stories.len()
    );

    // An edit that changes no font: what the per-commit font check adds.
    let t = std::time::Instant::now();
    let keys = paged_renderer::FontTable::story_font_keys(m.scene());
    eprintln!(
        "annual: font key walk {:.2} ms ({} stories)",
        t.elapsed().as_secs_f64() * 1e3,
        keys.len()
    );
    // A no-op rebuild for the baseline cost of a rebuild on warm caches.
    let t = std::time::Instant::now();
    m.rebuild_after_mutation().expect("warm rebuild");
    eprintln!(
        "annual: warm rebuild (nothing changed) {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );

    let mut affected_total = Vec::new();
    let t = std::time::Instant::now();
    for face in late.clone() {
        affected_total.extend(m.register_font(face).expect("register"));
    }
    affected_total.sort();
    affected_total.dedup();
    eprintln!(
        "annual: late registration of {} JetBrains Mono faces {:.0} ms, re-laid out {} of {} stories: {:?}",
        late.len(),
        t.elapsed().as_secs_f64() * 1e3,
        affected_total.len(),
        m.scene().stories.len(),
        affected_total
    );
    assert!(!affected_total.is_empty(), "the annual uses JetBrains Mono");

    let mut registry = early;
    registry.extend(late);
    let cold = CanvasModel::load("annual-cold", &bytes, opts(registry)).expect("cold load");
    let digests =
        |m: &CanvasModel| -> Vec<u64> { m.built().pages.iter().map(|p| p.list.digest()).collect() };
    assert_eq!(
        digests(&m),
        digests(&cold),
        "the annual after a late registration equals a cold load with every face"
    );
}
