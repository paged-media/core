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

//! The engine's performance WORKLOADS.
//!
//! Two performance bugs in one week hid from every measurement core had,
//! for the same reason: the measured workload never took the expensive
//! path. A per-paragraph hash of whole font files cost ~17 s per rebuild
//! in the editor, but the native baseline ran with no fonts registered and
//! skipped the hashing entirely; every repaint re-decoded the visible
//! photos, but no benchmark had a photo on screen. So a workload here must
//! carry what users' documents carry:
//!
//! * **real fonts, registered** — the whole `corpus/fonts` directory goes
//!   through the same font registry the editor's `RegisterFont` fills,
//!   variable faces included;
//! * **real images** — JPEG, PNG and WebP photos at plausible sizes,
//!   placed through `ReplaceImageBytes` exactly as `paged place` does;
//! * **a long threaded story** — one story through every body page, so a
//!   keystroke reflows across frames;
//! * **tables, styles, masters** — the 134-page `annual-base` supplies
//!   seven facing masters and the full style sheet.
//!
//! Everything is generated in-process and deterministic: no fixture bytes
//! are committed, and two runs build the same document.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use serde_json::json;

/// The corpus directory (fonts, profiles) relative to this crate.
pub fn corpus(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(rel)
}

/// Load options as the editor uses them: Inter as the default face, the
/// whole corpus font directory REGISTERED by family (the path a missing
/// registry silently skips), and the CMYK profile when present.
pub fn options() -> CanvasOptions {
    CanvasOptions {
        fonts: vec![std::fs::read(corpus("fonts/Inter.ttf")).expect("corpus/fonts/Inter.ttf")],
        font_registry: paged_canvas::font_registry_from_paths(&[corpus("fonts")]),
        cmyk_icc_profile: std::fs::read(corpus("profiles/default_cmyk.icc")).ok(),
        ..CanvasOptions::default()
    }
}

/// The annual-scale workload, loaded and authored.
pub struct Workload {
    pub model: CanvasModel,
    /// The story threaded through every body page.
    pub body_story: String,
    /// Its frames, in thread order (bare self ids).
    pub body_frames: Vec<String>,
    /// Rectangles holding a placed photo (bare self ids).
    pub image_frames: Vec<String>,
    /// One story per table frame.
    pub table_stories: Vec<String>,
    /// Characters in the body story.
    pub body_len: u32,
}

/// Body pages, 0-based: p11..=p126, the B-Body run of `annual-base`.
const BODY_PAGES: std::ops::RangeInclusive<usize> = 10..=125;
/// The live area inside BOTH the verso and recto margins (page-local pt).
const LIVE_TOP: f32 = 54.0;
const LIVE_BOTTOM: f32 = 639.0;
const LIVE_LEFT: f32 = 60.0;
const LIVE_RIGHT: f32 = 480.0;
/// Paragraphs in the body story — enough to run through every body frame.
const PARAGRAPHS: usize = 700;

fn mutation(op: &str, args: serde_json::Value) -> Mutation {
    serde_json::from_value(json!({ "op": op, "args": args }))
        .unwrap_or_else(|e| panic!("{op}: not a wire mutation: {e}"))
}

/// Apply one mutation and return what it minted.
fn apply(model: &mut CanvasModel, m: Mutation) -> Vec<paged_canvas::channel::MintedElement> {
    model
        .apply_mutation(&m)
        .unwrap_or_else(|e| panic!("workload mutation refused: {e:?}"))
        .minted
}

/// Deterministic prose: a fixed word list stepped by an LCG, so the text
/// has realistic word lengths, punctuation and hyphenation candidates and
/// is identical on every run.
pub fn prose(paragraphs: usize) -> Vec<String> {
    const WORDS: &[&str] = &[
        "the",
        "press",
        "counted",
        "every",
        "copy",
        "that",
        "left",
        "dock",
        "edition",
        "opened",
        "screen",
        "arithmetic",
        "between",
        "figures",
        "ledgers",
        "carry",
        "nothing",
        "rounded",
        "upward",
        "typography",
        "measure",
        "leading",
        "baseline",
        "composition",
        "paragraph",
        "justification",
        "hyphenation",
        "margin",
        "gutter",
        "column",
        "spread",
        "folio",
        "running",
        "head",
        "colophon",
        "specimen",
        "a",
        "of",
        "and",
        "in",
        "to",
        "is",
        "it",
        "with",
        "as",
        "for",
        "on",
        "was",
        "by",
        "from",
        "its",
        "were",
        "which",
        "this",
        "proofreading",
        "registration",
        "overprinting",
        "transparency",
        "interletterspacing",
    ];
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    (0..paragraphs)
        .map(|_| {
            let words = 40 + next() % 110;
            let mut p = String::new();
            for w in 0..words {
                let word = WORDS[next() % WORDS.len()];
                if w == 0 {
                    let mut c = word.chars();
                    p.extend(c.next().map(|f| f.to_ascii_uppercase()));
                    p.push_str(c.as_str());
                } else {
                    p.push(' ');
                    p.push_str(word);
                }
                if w + 1 < words && next() % 13 == 0 {
                    p.push(',');
                }
            }
            p.push('.');
            p
        })
        .collect()
}

/// A deterministic photo-like image (smooth gradients plus grain, so the
/// codecs do real work) encoded as JPEG, PNG or WebP by `seed % 3`.
pub fn photo(seed: u32, width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(width, height, |x, y| {
        let n = (x.wrapping_mul(73_856_093)
            ^ y.wrapping_mul(19_349_663)
            ^ seed.wrapping_mul(83_492_791))
            % 23;
        let r = (x * 255 / width) as u8 ^ (seed as u8).wrapping_mul(37);
        let g = (y * 255 / height) as u8;
        let b = (((x + y) * 255) / (width + height)) as u8;
        image::Rgb([r.wrapping_add(n as u8), g.wrapping_add(n as u8), b])
    });
    let format = match seed % 3 {
        0 => image::ImageFormat::Jpeg,
        1 => image::ImageFormat::Png,
        _ => image::ImageFormat::WebP,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .expect("encode workload photo");
    out.into_inner()
}

/// Load `annual-base` with [`options`] and author the content.
///
/// `photo_px` is the long edge of the placed photos: the budget tests use
/// small ones (the counters don't care about size), the benches use
/// photo-sized ones.
pub fn build(photo_px: u32) -> Workload {
    let idml =
        paged_gen::write_idml(&paged_gen::samples::annual_base::build()).expect("annual-base");
    let mut model = CanvasModel::load("perf-annual", &idml, options()).expect("annual-base loads");
    let pages: Vec<_> = model.page_ids().cloned().collect();

    // ── 1. Every frame, one batch: a body text frame per body page, the
    // photo rectangle on every fourth, a table frame on every tenth.
    let mut ops = Vec::new();
    let mut roles = Vec::new(); // (handle, role)
    let bind = |ops: &mut Vec<Mutation>, roles: &mut Vec<(String, &'static str)>, role| {
        let handle = format!("p{}", roles.len());
        ops.push(mutation("bindCreated", json!({ "handle": handle })));
        roles.push((handle, role));
    };
    for (n, page) in pages
        .iter()
        .enumerate()
        .filter(|(i, _)| BODY_PAGES.contains(i))
    {
        let (text_bottom, extra) = if n % 4 == 0 {
            (340.0, Some(("insertFrame", 352.0, "image")))
        } else if n % 10 == 5 {
            (480.0, Some(("insertTextFrame", 492.0, "table")))
        } else {
            (LIVE_BOTTOM, None)
        };
        ops.push(mutation(
            "insertTextFrame",
            json!({ "pageId": page, "bounds": [LIVE_TOP, LIVE_LEFT, text_bottom, LIVE_RIGHT] }),
        ));
        bind(&mut ops, &mut roles, "body");
        if let Some((op, top, role)) = extra {
            ops.push(mutation(
                op,
                json!({ "pageId": page, "bounds": [top, LIVE_LEFT, LIVE_BOTTOM, LIVE_RIGHT] }),
            ));
            bind(&mut ops, &mut roles, role);
        }
    }
    let minted = apply(&mut model, mutation("batch", json!({ "ops": ops })));
    let mut body_frames = Vec::new();
    let mut body_stories = Vec::new();
    let mut image_frames = Vec::new();
    let mut table_stories = Vec::new();
    // A batch of frame inserts only takes the translatable lane, which
    // reports its mints in ORDER without handles; the mixed lane names
    // them. Name first, order as the fallback (the editor driver's rule).
    assert_eq!(minted.len(), roles.len(), "one mint per bound frame");
    for (i, (handle, role)) in roles.iter().enumerate() {
        let m = minted
            .iter()
            .find(|m| m.handle.as_deref() == Some(handle.as_str()))
            .unwrap_or(&minted[i]);
        let id = m.element.raw_id().to_string();
        match *role {
            "body" => {
                body_frames.push(id);
                body_stories.push(m.story_id.clone().expect("a text frame mints its story"));
            }
            "image" => image_frames.push(id),
            _ => table_stories.push(m.story_id.clone().expect("a text frame mints its story")),
        }
    }

    // ── 2. Thread the body frames into one story.
    let links: Vec<Mutation> = body_frames
        .windows(2)
        .map(|w| mutation("linkFrames", json!({ "from": w[0], "to": w[1] })))
        .collect();
    apply(&mut model, mutation("batch", json!({ "ops": links })));
    let body_story = body_stories[0].clone();

    // ── 3. The prose, then its styles: body text throughout, a Head 2
    // every twelfth paragraph.
    let paragraphs = prose(PARAGRAPHS);
    let text = paragraphs.join("\n");
    let body_len = text.chars().count() as u32;
    apply(
        &mut model,
        mutation(
            "insertText",
            json!({ "storyId": body_story, "offset": 0, "text": text }),
        ),
    );
    let mut styles = vec![mutation(
        "applyStyle",
        json!({ "storyId": body_story, "start": 0, "end": body_len,
                "style": paged_gen::samples::annual_base::STYLE_BODY, "scope": "paragraph" }),
    )];
    let mut at = 0u32;
    for (i, p) in paragraphs.iter().enumerate() {
        let len = p.chars().count() as u32;
        if i % 12 == 0 {
            styles.push(mutation(
                "applyStyle",
                json!({ "storyId": body_story, "start": at, "end": at + len,
                        "style": paged_gen::samples::annual_base::STYLE_HEAD_2, "scope": "paragraph" }),
            ));
        }
        at += len + 1;
    }
    apply(&mut model, mutation("batch", json!({ "ops": styles })));

    // ── 4. Tables.
    let tables: Vec<Mutation> = table_stories
        .iter()
        .map(|s| {
            mutation(
                "insertTable",
                json!({ "storyId": s, "rows": 6, "cols": 4, "headerRows": 1 }),
            )
        })
        .collect();
    apply(&mut model, mutation("batch", json!({ "ops": tables })));

    // ── 5. Photos, through the same door `paged place` uses.
    let photos: Vec<Mutation> = image_frames
        .iter()
        .enumerate()
        .map(|(i, id): (usize, &String)| Mutation::ReplaceImageBytes {
            element_id: id.clone(),
            bytes: Some(photo(i as u32, photo_px, photo_px * 2 / 3).into()),
        })
        .collect();
    apply(&mut model, Mutation::Batch { ops: photos });

    Workload {
        model,
        body_story,
        body_frames,
        image_frames,
        table_stories,
        body_len,
    }
}
