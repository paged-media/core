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

//! thoughts ADR 027 §4 — the digest gate lane. Every scripted edit is
//! applied to a live model (caches, grow hint, every incremental path), and
//! after EVERY op the model is compared with a cold build of the same scene:
//! page set, every page's display-list digest, `story_layout` and the
//! diagnostics (`CanvasModel::digest_gate_check`). Incremental layout is
//! then tested, not assumed.
//!
//! The scripts type, delete, paste, restyle, change keeps / break-before /
//! span / grow rules, and undo and redo, on the documents the ADR 027 plan
//! measured: `reflow` and `docx-pagination` growing, `keeps`,
//! `span-columns`, `footnotes`, the cross-story `numbering` lists,
//! `variables`, `navigation`, and a long DOCX-shaped growing story. The
//! 134-page annual joins when `PAGED_DIGEST_GATE_ANNUAL` names its `.paged`
//! (it is not in this repository).

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

fn corpus(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(rel)
}

fn inter() -> Vec<u8> {
    std::fs::read(corpus("fonts/Inter.ttf")).expect("read Inter.ttf")
}

fn load(bytes: &[u8]) -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![inter()],
        ..CanvasOptions::default()
    };
    let m = CanvasModel::load("doc", bytes, opts).expect("load");
    m.digest_gate_check()
        .unwrap_or_else(|e| panic!("load is not a cold build: {e}"));
    m
}

fn sample(build: fn() -> paged_gen::package::Sample) -> Vec<u8> {
    paged_gen::write_idml(&build()).expect("write_idml")
}

/// `docx-pagination` with its grow rules, optionally with `extra` Word-like
/// paragraphs (3 sentences, about 5 lines each) appended to section 1: the
/// long DOCX-shaped story of the ADR 027 plan (2 000 → 232 Letter pages).
fn docx(extra: usize) -> Vec<u8> {
    use paged_gen::samples::docx_pagination::{section_story_id, sections};
    let idml = sample(paged_gen::samples::docx_pagination::build);
    let mut doc = idml_import::import_idml_doc(&idml).expect("import");
    let ids: Vec<String> = (0..sections().len() as u32).map(section_story_id).collect();
    for s in doc.stories.iter_mut() {
        if ids.contains(&s.self_id) {
            s.story.grow = Some(paged_model::FlowGrowRule {
                copy_frame_options: true,
                ..Default::default()
            });
        }
        if s.self_id == ids[0] && extra > 0 {
            let template = s.story.paragraphs[0].clone();
            for n in 0..extra {
                let mut p = template.clone();
                p.keep_with_next = None;
                p.runs.truncate(1);
                p.runs[0].text = format!(
                    "Inserted paragraph {n} opens with a sentence of ordinary prose that a \
                     Word document would carry. A second sentence continues the thought at \
                     about the same length, so the paragraph wraps. The third sentence closes \
                     it after roughly five lines of ten point text in the section frame, and a \
                     closing clause adds the words a real paragraph of a report would carry \
                     before the next one begins on a line of its own, with one more remark \
                     on the figures in the appendix to round it off. A last sentence notes \
                     where the next section picks the argument up again."
                );
                s.story.paragraphs.push(p);
            }
        }
    }
    paged_store::package::wrap_document(&doc, "Long.docx", 612.0, 792.0).expect("wrap")
}

/// The story with the most paragraphs that was laid out.
fn main_story(m: &CanvasModel) -> String {
    m.scene()
        .stories
        .iter()
        .max_by_key(|s| {
            (
                !m.built().story_layout(&s.self_id).is_empty(),
                s.story.paragraphs.len(),
            )
        })
        .map(|s| s.self_id.clone())
        .expect("a story")
}

fn paragraph_count(m: &CanvasModel, story: &str) -> usize {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .map_or(0, |s| s.story.paragraphs.len())
}

fn para_len(p: &paged_model::Paragraph) -> u32 {
    p.runs.iter().map(|r| r.text.len() as u32).sum()
}

/// Byte offset of paragraph `idx` in `InsertText`'s convention (each
/// paragraph break is one byte).
fn text_offset(m: &CanvasModel, story: &str, idx: usize) -> u32 {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    s.story.paragraphs[..idx]
        .iter()
        .map(|p| para_len(p) + 1)
        .sum()
}

/// Byte offset of the `chars`-th character of paragraph `idx` (clamped to
/// the paragraph), so a caret never lands inside a multi-byte character.
fn caret(m: &CanvasModel, story: &str, idx: usize, chars: usize) -> u32 {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    let text: String = s.story.paragraphs[idx]
        .runs
        .iter()
        .map(|r| r.text.as_str())
        .collect();
    let within = text
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i);
    text_offset(m, story, idx) + within as u32
}

/// `[start, end)` of paragraph `idx` in the property setter's convention
/// (characters, no break characters).
fn char_range(m: &CanvasModel, story: &str, idx: usize) -> (u32, u32) {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    let len = |p: &paged_model::Paragraph| -> u32 {
        p.runs.iter().map(|r| r.text.chars().count() as u32).sum()
    };
    let start: u32 = s.story.paragraphs[..idx].iter().map(len).sum();
    (start, start + len(&s.story.paragraphs[idx]))
}

/// One scripted step: its name and the mutations it applies.
enum Step {
    Op(&'static str, Box<Mutation>),
    Undo,
    Redo,
}

struct Gate {
    model: CanvasModel,
    name: &'static str,
    checked: usize,
    refused: Vec<&'static str>,
    /// Pages the last op reported as changed.
    last_reported: usize,
}

fn page_digests(m: &CanvasModel) -> std::collections::HashMap<String, u64> {
    m.built()
        .pages
        .iter()
        .map(|p| (p.id.0.clone(), p.list.digest()))
        .collect()
}

impl Gate {
    fn new(name: &'static str, bytes: &[u8]) -> Self {
        Self {
            model: load(bytes),
            name,
            checked: 0,
            refused: Vec::new(),
            last_reported: 0,
        }
    }

    fn step(&mut self, step: Step) {
        let label = match &step {
            Step::Op(l, _) => *l,
            Step::Undo => "undo",
            Step::Redo => "redo",
        };
        let before = page_digests(&self.model);
        let reported: Option<Vec<String>> = match step {
            Step::Op(_, op) => self
                .model
                .apply_mutation(&op)
                .ok()
                .map(|o| o.page_ids.into_iter().map(|p| p.0).collect()),
            Step::Undo => self
                .model
                .undo()
                .map(|o| o.page_ids.into_iter().map(|p| p.0).collect()),
            Step::Redo => self
                .model
                .redo()
                .map(|o| o.page_ids.into_iter().map(|p| p.0).collect()),
        };
        let Some(reported) = reported else {
            self.refused.push(label);
            return;
        };
        self.model
            .digest_gate_check()
            .unwrap_or_else(|e| panic!("{}: after `{label}`: {e}", self.name));
        // ADR 027 §7 — the pages an op reports as changed (what the GPU
        // re-encodes) must cover every page whose display list changed.
        for (id, digest) in page_digests(&self.model) {
            if !reported.contains(&id) {
                assert_eq!(
                    before.get(&id),
                    Some(&digest),
                    "{}: after `{label}`: page {id} changed but was not reported dirty",
                    self.name
                );
            }
        }
        self.last_reported = reported.len();
        self.checked += 1;
    }

    fn set_paragraph(&mut self, label: &'static str, story: &str, idx: usize, path: P, value: V) {
        let idx = idx.min(paragraph_count(&self.model, story).saturating_sub(1));
        let (start, end) = char_range(&self.model, story, idx);
        self.step(Step::Op(
            label,
            Box::new(Mutation::SetElementProperty {
                element_id: ElementId::StoryRange {
                    story_id: story.to_string(),
                    start,
                    end,
                },
                path,
                value,
            }),
        ));
    }

    /// Type `n` characters at an advancing caret inside paragraph `idx`.
    fn type_chars(&mut self, story: &str, idx: usize, n: usize) {
        let base = caret(&self.model, story, idx, 1);
        for (i, ch) in "typing".chars().cycle().take(n).enumerate() {
            self.step(Step::Op(
                "type",
                Box::new(Mutation::InsertText {
                    story_id: story.to_string(),
                    offset: base + i as u32,
                    text: ch.to_string(),
                    cell: None,
                }),
            ));
        }
    }

    /// The common edit script, on `story` around paragraph `idx`.
    fn script(&mut self, story: &str, idx: usize) {
        let idx = idx.min(paragraph_count(&self.model, story).saturating_sub(1));
        self.type_chars(story, idx, 6);
        // Backspace, then delete a word.
        let (at, next) = (
            caret(&self.model, story, idx, 3),
            caret(&self.model, story, idx, 4),
        );
        self.step(Step::Op(
            "backspace",
            Box::new(Mutation::DeleteRange {
                story_id: story.to_string(),
                start: at,
                end: next,
                cell: None,
            }),
        ));
        let (at, end) = (
            caret(&self.model, story, idx, 3),
            caret(&self.model, story, idx, 8),
        );
        self.step(Step::Op(
            "delete word",
            Box::new(Mutation::DeleteRange {
                story_id: story.to_string(),
                start: at,
                end,
                cell: None,
            }),
        ));
        // Paste three paragraphs, enough to move lines across a frame.
        let at = caret(&self.model, story, idx, 2);
        self.step(Step::Op(
            "paste",
            Box::new(Mutation::InsertText {
                story_id: story.to_string(),
                offset: at,
                text: "Pasted one runs long enough to wrap onto a second line in most frames.\n\
                       Pasted two.\nPasted three closes the paste "
                    .to_string(),
                cell: None,
            }),
        ));
        // Character and paragraph formatting on the edited paragraph.
        let (start, end) = char_range(&self.model, story, idx);
        self.step(Step::Op(
            "font size",
            Box::new(Mutation::SetElementProperty {
                element_id: ElementId::StoryRange {
                    story_id: story.to_string(),
                    start,
                    end: end.min(start + 4),
                },
                path: P::CharacterFontSize,
                value: V::Length(Some(18.0)),
            }),
        ));
        self.set_paragraph(
            "space before",
            story,
            idx,
            P::ParagraphSpaceBefore,
            V::Length(Some(14.0)),
        );
        // ADR 028 pagination rules.
        self.set_paragraph(
            "keep with next",
            story,
            idx,
            P::ParagraphKeepWithNext,
            V::Length(Some(2.0)),
        );
        self.set_paragraph(
            "keep together",
            story,
            idx + 1,
            P::ParagraphKeepAllLinesTogether,
            V::Bool(true),
        );
        self.set_paragraph(
            "start next frame",
            story,
            idx + 2,
            P::ParagraphStartParagraph,
            V::Text("NextFrame".into()),
        );
        self.set_paragraph(
            "span columns",
            story,
            idx,
            P::ParagraphSpanColumnType,
            V::Text("SpanColumns".into()),
        );
        // A style DEFINITION change re-cascades every paragraph using it.
        let style = self
            .model
            .scene()
            .stories
            .iter()
            .find(|s| s.self_id == story)
            .and_then(|s| s.story.paragraphs[idx].paragraph_style.clone());
        if let Some(style_id) = style {
            self.step(Step::Op(
                "style leading",
                Box::new(Mutation::SetStyleProperty {
                    collection: StyleCollection::Paragraph,
                    style_id,
                    path: P::ParagraphSpaceAfter,
                    value: V::Length(Some(9.0)),
                }),
            ));
        }
        self.step(Step::Undo);
        self.step(Step::Undo);
        self.step(Step::Redo);
        self.type_chars(story, idx, 3);
    }

    fn grow_toggle(&mut self, story: &str) {
        for grow in [false, true] {
            self.step(Step::Op(
                "grow rule",
                Box::new(Mutation::SetFlowGrowRule {
                    story_id: story.to_string(),
                    grow,
                    max_pages: None,
                    copy_frame_options: Some(true),
                }),
            ));
        }
    }

    fn finish(self) {
        assert!(
            self.checked >= 10,
            "{}: only {} ops reached the gate (refused: {:?})",
            self.name,
            self.checked,
            self.refused
        );
        eprintln!(
            "{}: {} ops gated, refused {:?}",
            self.name, self.checked, self.refused
        );
    }
}

fn run_sample(name: &'static str, bytes: &[u8], at: usize) {
    let mut g = Gate::new(name, bytes);
    let story = main_story(&g.model);
    g.script(&story, at);
    g.finish();
}

#[test]
fn reflow_grows_and_stays_equal_to_a_cold_build() {
    let mut g = Gate::new("reflow", &sample(paged_gen::samples::reflow::build));
    let story = main_story(&g.model);
    g.grow_toggle(&story);
    g.script(&story, 40);
    g.grow_toggle(&story);
    g.finish();
}

#[test]
fn docx_pagination_stays_equal_to_a_cold_build() {
    let mut g = Gate::new("docx-pagination", &docx(0));
    let story = main_story(&g.model);
    // P054 carries keepNext. Typing after it reuses the settled keep
    // break (ADR 027 plan step 3); the script around it does not.
    g.type_chars(&story, 60, 4);
    assert_eq!(
        g.model.built().stats.keep_seeds_used,
        1,
        "typing after P054 starts from its settled break"
    );
    g.script(&story, 50);
    g.type_chars(&story, 70, 2);
    g.grow_toggle(&story);
    g.finish();
}

#[test]
fn widow_control_on_a_growing_chain_stays_equal_to_a_cold_build() {
    // ADR 028 — Word's widow control on every paragraph, growing. Typing in
    // the last paragraph reuses every break settled before it.
    let mut g = Gate::new(
        "keeps-reflow",
        &sample(paged_gen::samples::keeps_reflow::build),
    );
    let story = main_story(&g.model);
    g.grow_toggle(&story);
    g.type_chars(&story, 43, 2);
    assert_eq!(
        g.model.built().stats.keep_seeds_used,
        1,
        "typing in the last paragraph starts from the breaks before it"
    );
    g.script(&story, 21);
    g.type_chars(&story, 5, 3);
    g.grow_toggle(&story);
    g.finish();
}

/// `keeps-reflow` (Word's widow control on every paragraph) with its
/// story repeated `copies` times and growing as Word's sections do.
fn widow_story(copies: usize) -> Vec<u8> {
    let idml = sample(paged_gen::samples::keeps_reflow::build);
    let mut doc = idml_import::import_idml_doc(&idml).expect("import");
    let id = paged_gen::ids::self_id("keeps-reflow", "BodyStory", 0);
    let s = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == id)
        .expect("body story");
    s.story.grow = Some(paged_model::FlowGrowRule {
        copy_frame_options: true,
        ..Default::default()
    });
    let one = s.story.paragraphs.clone();
    for _ in 1..copies {
        s.story.paragraphs.extend(one.iter().cloned());
    }
    paged_store::package::wrap_document(&doc, "Widow.docx", 172.0, 318.0).expect("wrap")
}

#[test]
fn a_long_widow_controlled_story_stays_equal_to_a_cold_build() {
    // ADR 028 — every page break of this story is a keep decision, so an
    // edit moves breaks after it. About 40 pages.
    let mut g = Gate::new("long widow-controlled story", &widow_story(5));
    let story = main_story(&g.model);
    assert!(g.model.built().pages.len() > 30, "the story grew");
    // Typing that keeps its line count resumes at the edit and stops at
    // the next paragraph, taking the previous build's breaks after it.
    g.type_chars(&story, 30, 2);
    let stats = &g.model.built().stats;
    assert_eq!(stats.stories_resumed, 1, "the edited story resumed");
    assert!(
        stats.frames_emitted <= 2,
        "typing on page 4 laid out {} frames",
        stats.frames_emitted
    );
    // A word at a time until the paragraph gains a line: every page break
    // after it moves and is decided again.
    let lines_before = g.model.built().story_layout(&story).len();
    let at = caret(&g.model, &story, 31, 1);
    let mut typed = 0;
    while g.model.built().story_layout(&story).len() == lines_before && typed < 60 {
        g.step(Step::Op(
            "type a word",
            Box::new(Mutation::InsertText {
                story_id: story.clone(),
                offset: at,
                text: " typing".to_string(),
                cell: None,
            }),
        ));
        typed += 7;
    }
    assert!(typed < 60, "the typing wrapped onto another line");
    g.step(Step::Op(
        "delete the typing",
        Box::new(Mutation::DeleteRange {
            story_id: story.clone(),
            start: at,
            end: at + typed,
            cell: None,
        }),
    ));
    // A one-line paragraph at the foot of page 4, whose keeps left three
    // lines free: page 5 still opens with the heading P22, one paragraph
    // later, so the flow rejoins and the previous build's breaks are taken
    // over shifted by a paragraph.
    let end_of_p21 = text_offset(&g.model, &story, 21) - 1;
    g.step(Step::Op(
        "new paragraph",
        Box::new(Mutation::InsertText {
            story_id: story.clone(),
            offset: end_of_p21,
            text: "\nNew".to_string(),
            cell: None,
        }),
    ));
    let stats = &g.model.built().stats;
    assert_eq!(stats.stories_resumed, 1, "the new paragraph resumed");
    assert!(
        stats.frames_emitted <= 2,
        "a paragraph absorbed by page 4 laid out {} frames",
        stats.frames_emitted
    );
    g.type_chars(&story, 60, 2);
    g.step(Step::Undo);
    g.step(Step::Undo);
    g.script(&story, 12);
    g.type_chars(&story, 150, 3);
    g.finish();
}

#[test]
fn keeps_stays_equal_to_a_cold_build() {
    run_sample("keeps", &sample(paged_gen::samples::keeps::build), 2);
}

#[test]
fn span_columns_stays_equal_to_a_cold_build() {
    run_sample(
        "span-columns",
        &sample(paged_gen::samples::span_columns::build),
        1,
    );
}

#[test]
fn footnotes_stay_equal_to_a_cold_build() {
    run_sample(
        "footnotes",
        &sample(paged_gen::samples::footnotes::build),
        1,
    );
}

#[test]
fn cross_story_numbering_stays_equal_to_a_cold_build() {
    // Every story of the numbering fixture, so the edited one is inside a
    // continued list some of the time.
    let bytes = sample(paged_gen::samples::numbering::build);
    let mut g = Gate::new("numbering", &bytes);
    let stories: Vec<String> = g
        .model
        .scene()
        .stories
        .iter()
        .filter(|s| !g.model.built().story_layout(&s.self_id).is_empty())
        .map(|s| s.self_id.clone())
        .collect();
    for story in &stories {
        g.type_chars(story, 0, 2);
        g.step(Step::Op(
            "new list item",
            Box::new(Mutation::InsertText {
                story_id: story.clone(),
                offset: caret(&g.model, story, 0, 1),
                text: "\nInserted item".to_string(),
                cell: None,
            }),
        ));
    }
    g.step(Step::Undo);
    g.finish();
}

#[test]
fn variables_stay_equal_to_a_cold_build() {
    run_sample(
        "variables",
        &sample(paged_gen::samples::variables::build),
        0,
    );
    // Typing into every story, headings included: a running header picks
    // its text up after layout, so the post-layout pass must re-resolve
    // exactly the frames that print it (ADR 027 plan step 5).
    let mut g = Gate::new(
        "variables, every story",
        &sample(paged_gen::samples::variables::build),
    );
    let stories: Vec<String> = g
        .model
        .scene()
        .stories
        .iter()
        .map(|s| s.self_id.clone())
        .collect();
    for story in &stories {
        for idx in 0..paragraph_count(&g.model, story).min(3) {
            g.type_chars(story, idx, 1);
        }
    }
    g.finish();
}

#[test]
fn navigation_stays_equal_to_a_cold_build() {
    run_sample(
        "navigation",
        &sample(paged_gen::samples::navigation::build),
        0,
    );
}

#[test]
fn a_long_docx_story_stays_equal_to_a_cold_build() {
    // 80 extra paragraphs: about 12 pages, long enough that an edit on
    // page 3 has many pages after it, short enough for a debug test binary.
    let mut g = Gate::new("long docx story", &docx(80));
    let story = main_story(&g.model);
    assert!(g.model.built().pages.len() > 8, "the story grew");
    // ADR 029 / plan acceptance: typing on page 3 lays out at most two
    // frames (the story resumes at the edit and stops where the flow
    // rejoins the previous build's).
    g.type_chars(&story, 115, 3);
    let stats = &g.model.built().stats;
    assert_eq!(stats.stories_resumed, 1, "the edited story resumed");
    assert!(
        stats.frames_emitted <= 2,
        "typing on page 3 laid out {} frames",
        stats.frames_emitted
    );
    assert!(
        g.last_reported <= 2,
        "typing on page 3 changed {} pages",
        g.last_reported
    );
    // Typing on until the paragraph wraps onto another line, then deleting
    // it again: the paragraphs after it move by a line, so the early stop
    // must see the moved flow (not just the same frame and glyphs).
    // A word at a time, so after the first the page already holds every
    // glyph typed (the early stop compares path-buffer prints).
    let lines_before = g.model.built().story_layout(&story).len();
    let at = caret(&g.model, &story, 116, 1);
    let mut typed = 0;
    while g.model.built().story_layout(&story).len() == lines_before && typed < 180 {
        g.step(Step::Op(
            "type a word",
            Box::new(Mutation::InsertText {
                story_id: story.clone(),
                offset: at,
                text: "typing".to_string(),
                cell: None,
            }),
        ));
        typed += 6;
    }
    assert!(typed < 180, "the typing wrapped onto another line");
    g.step(Step::Op(
        "delete the typing",
        Box::new(Mutation::DeleteRange {
            story_id: story.clone(),
            start: at,
            end: at + typed as u32,
            cell: None,
        }),
    ));
    assert_eq!(lines_before, g.model.built().story_layout(&story).len());
    g.script(&story, 115);
    g.finish();
}

#[test]
fn the_annual_stays_equal_to_a_cold_build() {
    let Ok(path) = std::env::var("PAGED_DIGEST_GATE_ANNUAL") else {
        eprintln!("PAGED_DIGEST_GATE_ANNUAL unset: the annual lane is skipped");
        return;
    };
    let bytes = std::fs::read(&path).expect("read the annual");
    let opts = CanvasOptions {
        fonts: vec![inter()],
        font_registry: paged_canvas::font_registry_from_paths(&[corpus("fonts")]),
        cmyk_icc_profile: std::fs::read(corpus("profiles/default_cmyk.icc")).ok(),
        ..CanvasOptions::default()
    };
    let model = CanvasModel::load("annual", &bytes, opts).expect("load the annual");
    let mut g = Gate {
        model,
        name: "annual",
        checked: 0,
        refused: Vec::new(),
        last_reported: 0,
    };
    // The story on page 3 (the plan's edit), then the longest story.
    let page3 = g.model.built().pages[2].id.clone();
    let story = g
        .model
        .built()
        .pages
        .iter()
        .find(|p| p.id == page3)
        .and_then(|p| p.story_layout.first())
        .map(|l| l.story_id.clone())
        .expect("text on page 3");
    g.script(&story, 0);
    // Story/u38, the body story the plan types into: only its own entry
    // is dropped, every other story is spliced from the cache.
    if g.model
        .scene()
        .stories
        .iter()
        .any(|s| s.self_id == "Story/u38")
    {
        g.type_chars("Story/u38", 0, 4);
        g.step(Step::Undo);
    }
    let longest = main_story(&g.model);
    g.type_chars(&longest, 1, 3);
    eprintln!(
        "annual: last build reused {} of {} body stories",
        g.model.built().stats.body_stories_reused,
        g.model.built().stats.stories
    );
    g.finish();
}
