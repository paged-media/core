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
}

impl Gate {
    fn new(name: &'static str, bytes: &[u8]) -> Self {
        Self {
            model: load(bytes),
            name,
            checked: 0,
            refused: Vec::new(),
        }
    }

    fn step(&mut self, step: Step) {
        let label = match &step {
            Step::Op(l, _) => *l,
            Step::Undo => "undo",
            Step::Redo => "redo",
        };
        let applied = match step {
            Step::Op(_, op) => self.model.apply_mutation(&op).is_ok(),
            Step::Undo => self.model.undo().is_some(),
            Step::Redo => self.model.redo().is_some(),
        };
        if !applied {
            self.refused.push(label);
            return;
        }
        self.model
            .digest_gate_check()
            .unwrap_or_else(|e| panic!("{}: after `{label}`: {e}", self.name));
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
    // P054 carries keepNext; edit around the page-1/2 boundary.
    g.script(&story, 50);
    g.grow_toggle(&story);
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
    // 150 extra paragraphs: about 20 pages, long enough that an edit on
    // page 3 has many pages after it, short enough for a debug test binary.
    let mut g = Gate::new("long docx story", &docx(150));
    let story = main_story(&g.model);
    assert!(g.model.built().pages.len() > 12, "the story grew");
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
    let longest = main_story(&g.model);
    g.type_chars(&longest, 1, 3);
    g.finish();
}
