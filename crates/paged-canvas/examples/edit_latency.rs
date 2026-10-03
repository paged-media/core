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

//! ADR 027 plan §1 — per-keystroke edit latency, measured the way
//! the plan did: `CanvasModel::load` with the corpus font directory
//! registered and Inter as the default face, then `apply_mutation(InsertText)`
//! of one character at an advancing caret. Median over `--edits` edits after
//! one warm-up edit.
//!
//! ```text
//! cargo run --release -p paged-canvas --example edit_latency -- long
//! cargo run --release -p paged-canvas --example edit_latency -- annual PATH.paged
//! cargo run --release -p paged-canvas --example edit_latency -- reflow|docx|paste
//! ```
//!
//! `save-long PATH` writes the long story's `.paged` (for the wasm harness,
//! which drives the same edit through `handleMessage`) and prints the story
//! id and the caret offset typed at.
//!
//! `long` is the plan's long DOCX-shaped story: `docx-pagination` with 2 000
//! Word-like paragraphs appended to section 1 (about 232 Letter pages),
//! typed on page 3. `paste` pastes 60 paragraphs on page 3 of it.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};

fn corpus(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(rel)
}

fn options(cmyk: bool) -> CanvasOptions {
    CanvasOptions {
        fonts: vec![std::fs::read(corpus("fonts/Inter.ttf")).expect("Inter.ttf")],
        font_registry: paged_canvas::font_registry_from_paths(&[corpus("fonts")]),
        cmyk_icc_profile: if cmyk {
            std::fs::read(corpus("profiles/default_cmyk.icc")).ok()
        } else {
            None
        },
        ..CanvasOptions::default()
    }
}

/// `docx-pagination` with its grow rules and `extra` paragraphs appended to
/// section 1.
fn docx(extra: usize) -> Vec<u8> {
    use paged_gen::samples::docx_pagination::{section_story_id, sections};
    let idml = paged_gen::write_idml(&paged_gen::samples::docx_pagination::build()).expect("idml");
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

/// Byte offset of paragraph `idx` of `story` in `InsertText`'s convention.
fn text_offset(m: &CanvasModel, story: &str, idx: usize) -> u32 {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    s.story.paragraphs[..idx]
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.len() as u32).sum::<u32>() + 1)
        .sum()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Type `edits + 1` characters into paragraph `idx` of `story`; report the
/// medians of the last `edits`.
fn type_into(name: &str, m: &mut CanvasModel, story: &str, idx: usize, edits: usize) {
    // After the paragraph's first character, on a char boundary.
    let first = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .and_then(|s| {
            s.story.paragraphs[idx]
                .runs
                .iter()
                .find_map(|r| r.text.chars().next())
        })
        .map_or(0, |c| c.len_utf8() as u32);
    let base = text_offset(m, story, idx) + first;
    println!("{name}: typing into {story} at byte offset {base}");
    let (mut wall, mut build) = (Vec::new(), Vec::new());
    for i in 0..=edits {
        let t = std::time::Instant::now();
        m.apply_mutation(&Mutation::InsertText {
            story_id: story.to_string(),
            offset: base + i as u32,
            text: "x".to_string(),
            cell: None,
        })
        .expect("insert");
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if i > 0 {
            wall.push(ms);
            build.push(m.last_rebuild_stats().build_ms);
        }
    }
    let stats = m.last_rebuild_stats();
    let built = &m.built().stats;
    println!(
        "{name}: {} pages, per keystroke wall {:.1} ms, build {:.1} ms (median of {edits}; \
         fastest {:.1} ms); last edit: {} frames laid out, {} stories resumed, {} reused, \
         {} of {} pages changed, {} adopted",
        stats.pages,
        median(wall.clone()),
        median(build),
        wall.iter().copied().fold(f64::INFINITY, f64::min),
        built.frames_emitted,
        built.stories_resumed,
        built.body_stories_reused,
        m.dirty_page_ids().len(),
        stats.pages,
        built.pages_adopted,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let which = args.first().map(String::as_str).unwrap_or("long");
    let edits: usize = std::env::var("EDITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    match which {
        "annual" => {
            let path = args.get(1).expect("annual needs the .paged path");
            let bytes = std::fs::read(path).expect("read annual");
            let t = std::time::Instant::now();
            let mut m = CanvasModel::load("annual", &bytes, options(true)).expect("load");
            println!("annual: load {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            // The plan's edit: Story/u38, a body story on page 3 (or
            // STORY=<id>), typed into its first paragraph.
            let wanted = std::env::var("STORY").unwrap_or_else(|_| "Story/u38".to_string());
            let story = (wanted, 0usize);
            type_into("annual", &mut m, &story.0, story.1, edits);
        }
        "reflow" => {
            let idml = paged_gen::write_idml(&paged_gen::samples::reflow::build()).expect("idml");
            let mut m = CanvasModel::load("reflow", &idml, options(false)).expect("load");
            let story = m.built().pages[0].story_layout[0].story_id.clone();
            m.apply_mutation(&Mutation::SetFlowGrowRule {
                story_id: story.clone(),
                grow: true,
                max_pages: None,
                copy_frame_options: None,
            })
            .expect("grow");
            type_into("reflow", &mut m, &story, 40, edits);
        }
        "docx" => {
            let mut m = CanvasModel::load("docx", &docx(0), options(false)).expect("load");
            let story = paged_gen::samples::docx_pagination::section_story_id(0);
            type_into("docx-pagination", &mut m, &story, 60, edits);
        }
        "long" | "paste" => {
            let t = std::time::Instant::now();
            let mut m = CanvasModel::load("long", &docx(2000), options(false)).expect("load");
            println!(
                "long story: load {:.0} ms, {} pages",
                t.elapsed().as_secs_f64() * 1e3,
                m.built().pages.len()
            );
            let story = paged_gen::samples::docx_pagination::section_story_id(0);
            // Paragraph 115 sits on page 3 (P108-P120 of the fixture).
            if which == "long" {
                type_into("long story", &mut m, &story, 115, edits);
            } else {
                let text: String = (0..60)
                    .map(|n| format!("Pasted paragraph {n} carries a sentence or two of text.\n"))
                    .collect();
                let at = text_offset(&m, &story, 115) + 1; // ASCII paragraph
                let pages = m.built().pages.len();
                let t = std::time::Instant::now();
                m.apply_mutation(&Mutation::InsertText {
                    story_id: story,
                    offset: at,
                    text,
                    cell: None,
                })
                .expect("paste");
                println!(
                    "paste on page 3: {:.0} ms wall, {pages} -> {} pages",
                    t.elapsed().as_secs_f64() * 1e3,
                    m.built().pages.len()
                );
            }
        }
        "save-long" => {
            let path = args.get(1).expect("save-long needs an output path");
            let bytes = docx(2000);
            std::fs::write(path, &bytes).expect("write");
            let m = CanvasModel::load("long", &bytes, options(false)).expect("load");
            let story = paged_gen::samples::docx_pagination::section_story_id(0);
            println!("story={story} offset={}", text_offset(&m, &story, 115) + 1);
        }
        other => panic!("unknown document {other:?}"),
    }
}
