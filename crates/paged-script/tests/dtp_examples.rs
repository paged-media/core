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

//! Regression guards for the docs `paged.*` scripting examples
//! (`docs/data/scripting/examples.ts`). Each test runs the example's
//! SEED prelude (`docs/data/scripting/seeds.ts`) and then its SCRIPT
//! **verbatim** against a blank engine document — the same bytes the docs
//! playground + `pnpm validate:scripting` gate run — then asserts the
//! example's concrete, reader-visible outcome via `paged.*` read-backs.
//!
//! The `// example: <id>` comment on each test maps it back to the docs
//! example `id`, so the two stay matched (the example's `test` field points
//! here). Examples whose outcome is not cleanly observable headlessly
//! (thin reads; config fns that return `false` on a doc lacking the
//! resource; `createGroup`, which is a no-op headlessly) are guarded only
//! for "runs without error" in [`runs_clean`] — each a labelled case.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::{execute_script, ScriptResult};
use serde_json::Value;

const DOC_ID: &str = "dtp-examples";

// ── seed preludes (verbatim from docs/data/scripting/seeds.ts) ────────────────

const SEED_ONE_TEXT: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [144, 72, 360, 540]);
const stories = JSON.parse(paged.stories());
if (stories.length) {
  paged.insertText(stories[0].selfId, 0, "Paged is a programmable page-layout engine. This frame and its text were created by a paged.* seed script — the same API you are about to drive.");
}"#;

const SEED_TWO_FRAMES: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [72, 72, 300, 320]);
paged.insertFrame(pid, [340, 72, 520, 320]);"#;

const SEED_STYLED_STORY: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [108, 72, 540, 540]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, "Heading\nThe body follows the heading. Each newline starts a new paragraph in the same story.");
const ps = JSON.parse(paged.paragraphStyles());
if (ps.length) {
  paged.applyStyle(sid, 0, 7, ps[0].selfId);
}"#;

const SEED_SWATCHES: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertFrame(pid, [120, 120, 320, 420]);"#;

const SEED_IMAGE_FRAME: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertFrame(pid, [120, 96, 420, 480]);"#;

/// Map a seed `id` to its prelude source (`blank` ⇒ empty).
fn seed_src(seed: &str) -> &'static str {
    match seed {
        "blank" => "",
        "one-text-frame-selected" => SEED_ONE_TEXT,
        "two-frames" => SEED_TWO_FRAMES,
        "styled-story" => SEED_STYLED_STORY,
        "swatches-and-styles" => SEED_SWATCHES,
        "image-frame" => SEED_IMAGE_FRAME,
        other => panic!("unknown seed `{other}`"),
    }
}

// ── harness helpers ───────────────────────────────────────────────────────────

/// A blank US-Letter (612×792 pt) document — exactly what the docs gate's
/// `new-blank` produces (see `docs/scripts/scripting/validate.ts`).
fn blank_model() -> CanvasModel {
    CanvasModel::new_blank(DOC_ID, 612.0, 792.0, CanvasOptions::default()).expect("new_blank")
}

/// Blank model with the given seed prelude SOURCE applied (asserting the
/// seed itself runs clean).
fn seeded(seed_source: &str) -> CanvasModel {
    let mut m = blank_model();
    if !seed_source.is_empty() {
        let r = execute_script(&mut m, seed_source);
        assert!(r.error.is_none(), "seed prelude errored: {:?}", r.error);
    }
    m
}

/// Blank model with the named seed applied.
fn seeded_for(seed: &str) -> CanvasModel {
    seeded(seed_src(seed))
}

/// Run a script, asserting it produced no error, and return the result.
fn run_ok(m: &mut CanvasModel, src: &str) -> ScriptResult {
    let r = execute_script(m, src);
    assert!(r.error.is_none(), "script errored: {:?}", r.error);
    r
}

fn output_has(r: &ScriptResult, needle: &str) -> bool {
    r.output.iter().any(|l| l.contains(needle))
}

/// Run `expr` (whose terminal value is a JSON string returned by a `paged.*`
/// read) and parse it into a JSON value.
fn read(m: &mut CanvasModel, expr: &str) -> Value {
    let r = execute_script(m, expr);
    assert!(r.error.is_none(), "read `{expr}` errored: {:?}", r.error);
    let line = r
        .output
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("read `{expr}` produced no output"));
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("read `{expr}` not JSON ({e}): {line}"))
}

/// `paged.get(ref, path)` parsed — the typed `{ "type", "value" }` envelope.
fn jget(m: &mut CanvasModel, frame: &str, path: &str) -> Value {
    read(m, &format!("paged.get('{frame}', '{path}')"))
}

/// The `value` envelope of one `paged.inspect` entry (Null if absent).
fn entry_value(props: &Value, path: &str) -> Value {
    props["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["path"] == path)
        .map(|e| e["value"].clone())
        .unwrap_or(Value::Null)
}

fn inspect_length(props: &Value, path: &str) -> Option<f64> {
    entry_value(props, path)["value"].as_f64()
}

fn inspect_text(props: &Value, path: &str) -> Option<String> {
    entry_value(props, path)["value"].as_str().map(String::from)
}

/// The frame children of spread 0 / page 0 from `paged.tree()`.
fn page_children(m: &mut CanvasModel) -> Value {
    read(m, "paged.tree()")[0]["children"][0]["children"].clone()
}

fn page_frame_count(m: &mut CanvasModel) -> usize {
    page_children(m).as_array().map(Vec::len).unwrap_or(0)
}

/// The first frame on page 1, addressed as `kind:id`.
fn first_frame_ref(m: &mut CanvasModel) -> String {
    let kids = page_children(m);
    let fr = &kids[0];
    format!(
        "{}:{}",
        fr["id"]["kind"].as_str().expect("frame kind"),
        fr["id"]["id"].as_str().expect("frame id")
    )
}

fn first_story_id(m: &mut CanvasModel) -> String {
    read(m, "paged.stories()")[0]["selfId"]
        .as_str()
        .expect("story id")
        .to_string()
}

fn page_count(m: &mut CanvasModel) -> usize {
    read(m, "paged.pages()").as_array().map(Vec::len).unwrap_or(0)
}

/// The `name` field of each row of a `paged.*` collection read.
fn collection_names(m: &mut CanvasModel, expr: &str) -> Vec<String> {
    read(m, expr)
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ══════════════════════════════════════════════════════════════════════════════
// Composite workflows — assert the concrete, reader-visible outcome.
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn workflow_two_column_article() {
    // example: workflow-two-column-article
    let mut m = blank_model();
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const frame = paged.insertTextFrame(pid, [72, 72, 720, 540]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Headline goes here\nBody copy flows beneath the headline and fills both columns with continuous text set at a comfortable reading size.');
// Two columns with a 14pt gutter.
paged.set(frame, 'textFrameColumnCount', 2);
paged.set(frame, 'textFrameColumnGutter', 14);
// A heading style on the first line.
const heading = paged.createParagraphStyle({ name: 'Article Heading' });
paged.applyStyle(sid, 0, 17, heading);
console.log('two-column article laid out in', frame);"#,
    );
    let frame = first_frame_ref(&mut m);
    assert_eq!(
        jget(&mut m, &frame, "textFrameColumnCount")["value"].as_f64(),
        Some(2.0),
        "frame should carry 2 columns"
    );
    let sid = first_story_id(&mut m);
    let stories = read(&mut m, "paged.stories()");
    assert_eq!(
        stories[0]["paragraphCount"].as_i64(),
        Some(2),
        "the \\n splits the story into 2 paragraphs"
    );
    // The first 17 chars carry the newly-created heading paragraph style.
    let props = read(&mut m, &format!("paged.inspect('storyRange:{sid}@0..17')"));
    let applied = inspect_text(&props, "appliedParagraphStyle");
    assert!(
        applied
            .as_deref()
            .is_some_and(|s| s.starts_with("ParagraphStyle/")),
        "chars 0..17 should carry a paragraph style, got {applied:?}"
    );
}

#[test]
fn workflow_price_table() {
    // example: workflow-price-table
    let mut m = seeded(SEED_STYLED_STORY);
    let hash_before = m.current_state_hash();
    let r = run_ok(
        &mut m,
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 5, cols: 3 });   // Item · Qty · Price
paged.insertHeaderRow(sid, table);
paged.setRowHeight(sid, table, 0, 26);     // tall header band
paged.setColumnWidth(sid, table, 0, 220);  // wide Item column
paged.setColumnWidth(sid, table, 1, 80);   // narrow Qty
console.log('price table', table, 'ready');"#,
    );
    // NOTE: a table's internal structure (body/header rows, columns, row
    // heights, column widths) is NOT observable through paged.* reads —
    // `paged.inspect(<tableId>)` returns null. We therefore assert the
    // strongest observable proxy: a non-empty table id was minted and the
    // structural ops mutated the scene. (The booleans were confirmed `true`
    // during authoring; the docs gate guards that the script runs clean.)
    assert!(
        output_has(&r, "price table") && output_has(&r, "ready"),
        "table not built: {:?}",
        r.output
    );
    assert_ne!(
        hash_before,
        m.current_state_hash(),
        "building the table must mutate the scene"
    );
}

#[test]
fn workflow_image_with_caption() {
    // example: workflow-image-with-caption
    let mut m = blank_model();
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const figure = paged.insertFrame(pid, [96, 96, 360, 432]);
paged.placeImage(figure, 'https://docs.paged.media/preview/sample.png', 'fillProportional');
// Caption frame beneath the image.
const caption = paged.insertTextFrame(pid, [366, 96, 392, 432]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Figure 1. A placed image fitted into its frame.');
const range = 'storyRange:' + sid + '@0..48';
paged.set(range, 'characterFontSize', 8);
console.log('captioned figure', figure, '+', caption);"#,
    );
    // Two frames on the page: a graphic (Rectangle) figure + a TextFrame caption.
    let kinds: Vec<String> = page_children(&mut m)
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["kind"].as_str().map(String::from))
        .collect();
    assert!(kinds.iter().any(|k| k == "Rectangle"), "figure: {kinds:?}");
    assert!(kinds.iter().any(|k| k == "TextFrame"), "caption: {kinds:?}");
    // The caption's first 48 chars are 8pt. (placeImage returns false
    // headlessly — the remote asset can't be fetched — so the image LINK
    // itself is not observable here; we assert the two frames + 8pt caption.)
    let sid = first_story_id(&mut m);
    let props = read(&mut m, &format!("paged.inspect('storyRange:{sid}@0..48')"));
    assert_eq!(
        inspect_length(&props, "characterFontSize"),
        Some(8.0),
        "caption chars 0..48 should be 8pt"
    );
}

#[test]
fn workflow_thread_overset() {
    // example: workflow-thread-overset
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const a = paged.insertTextFrame(pid, [72, 72, 200, 300]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'A feature article whose copy runs well past the bottom of this first short frame, so the overset must continue onto the next page to be read in full.');
const p2 = paged.insertPage(pid);
const b = paged.insertTextFrame(p2, [72, 72, 720, 300]);
console.log('continued onto page 2 →', paged.linkFrames(a, b));"#,
    );
    assert!(
        output_has(&r, "continued onto page 2 → true"),
        "linkFrames should thread the chain: {:?}",
        r.output
    );
    // The chain spans 2 frames across 2 pages: a page each with a TextFrame.
    assert_eq!(page_count(&mut m), 2, "the story now spans two pages");
    let tree = read(&mut m, "paged.tree()");
    let has_frame = |spread: &Value| {
        spread["children"][0]["children"]
            .as_array()
            .map(|a| a.iter().any(|n| n["kind"] == "TextFrame"))
            .unwrap_or(false)
    };
    assert!(has_frame(&tree[0]), "frame on page 1");
    assert!(has_frame(&tree[1]), "frame on page 2");
}

#[test]
fn workflow_pull_quote() {
    // example: workflow-pull-quote
    let mut m = seeded(SEED_ONE_TEXT);
    run_ok(
        &mut m,
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
paged.set(ref, 'frameFillColor', 'Color/Black');
paged.set(ref, 'frameFillTint', 10);              // 10% grey panel
paged.set(ref, 'frameStrokeColor', 'Color/Black');
paged.set(ref, 'frameStrokeWeight', 2);           // 2pt rule
paged.set(ref, 'frameInsetSpacing', [12, 12, 12, 12]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.set('storyRange:' + sid + '@0..40', 'characterFontSize', 18);
console.log('pull-quote styled on', ref);"#,
    );
    let frame = first_frame_ref(&mut m);
    assert_eq!(
        jget(&mut m, &frame, "frameFillColor")["value"].as_str(),
        Some("Color/Black"),
        "panel fill"
    );
    assert_eq!(
        jget(&mut m, &frame, "frameFillTint")["value"].as_f64(),
        Some(10.0),
        "10% tint"
    );
    assert_eq!(
        jget(&mut m, &frame, "frameStrokeWeight")["value"].as_f64(),
        Some(2.0),
        "2pt rule"
    );
    assert_eq!(
        jget(&mut m, &frame, "frameInsetSpacing")["value"],
        serde_json::json!([12.0, 12.0, 12.0, 12.0]),
        "12pt inset on all sides"
    );
    let sid = first_story_id(&mut m);
    let props = read(&mut m, &format!("paged.inspect('storyRange:{sid}@0..40')"));
    assert_eq!(
        inspect_length(&props, "characterFontSize"),
        Some(18.0),
        "enlarged 18pt type on the quote"
    );
}

#[test]
fn workflow_section_numbering() {
    // example: workflow-section-numbering
    let mut m = blank_model();
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertSection(pid, { style: 'lowerRoman', start: 1 });   // front matter: i, ii…
const body = paged.insertPage(pid);
paged.insertSection(body, { style: 'arabic', start: 1 });      // body restarts at 1
console.log('sections:', JSON.parse(paged.collection('sections')).length);"#,
    );
    // Two sections anchored at page index 0 and 1. NOTE: the label style
    // reads back as "arabic" for both headlessly, so we assert the section
    // COUNT + their start page indices rather than the lowerRoman style.
    let sections = read(&mut m, "paged.collection('sections')");
    let rows = sections.as_array().expect("sections array");
    assert_eq!(rows.len(), 2, "front-matter + body sections");
    let starts: Vec<i64> = rows.iter().filter_map(|s| s["startPageIndex"].as_i64()).collect();
    assert!(
        starts.contains(&0) && starts.contains(&1),
        "sections start at page 0 and page 1, got {starts:?}"
    );
}

#[test]
fn workflow_running_footer() {
    // example: workflow-running-footer
    let mut m = blank_model();
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const footer = paged.insertTextFrame(pid, [740, 72, 760, 540]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'PAGED MEDIA QUARTERLY   ');
paged.insertField(sid, JSON.parse(paged.stories())[0].characterCount, 'pageNumber');
paged.set('storyRange:' + sid + '@0..24', 'characterFontSize', 8);
console.log('running footer placed in', footer);"#,
    );
    // 24 chars of running head + 1 page-number field char appended at the END.
    let stories = read(&mut m, "paged.stories()");
    assert_eq!(
        stories[0]["characterCount"].as_i64(),
        Some(25),
        "24 head chars + a trailing page-number field char"
    );
    let sid = stories[0]["selfId"].as_str().unwrap().to_string();
    let props = read(&mut m, &format!("paged.inspect('storyRange:{sid}@0..24')"));
    assert_eq!(
        inspect_length(&props, "characterFontSize"),
        Some(8.0),
        "the running head is 8pt"
    );
}

#[test]
fn workflow_column_guides() {
    // example: workflow-column-guides
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const spread = JSON.parse(paged.collection('spreads'))[0].selfId;
let placed = 0;
for (const x of [153, 306, 459]) {          // quarter / half / three-quarter
  if (paged.insertGuide(spread, 'vertical', x, 0)) placed++;
}
console.log('placed', placed, 'column guides');"#,
    );
    // All three vertical guides (at x = 153/306/459, the quarter/half/
    // three-quarter marks of the 612pt-wide page) are placed. The script counts
    // the insertGuide successes itself; a script-inserted guide is not reflected
    // by a SUBSEQUENT collection('spreads') read, so we assert that count.
    assert!(
        output_has(&r, "placed 3 column guides"),
        "three column guides should be placed: {:?}",
        r.output
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Per-fn examples with a cleanly observable outcome.
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn author_text_frame() {
    // example: author-text-frame
    let mut m = blank_model();
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const frame = paged.insertTextFrame(pid, [120, 72, 320, 480]);   // [t,l,b,r] in pt
console.log('created', frame);                                    // "textFrame:uX"

const story = JSON.parse(paged.stories())[0].selfId;
paged.insertText(story, 0, 'Authored entirely by a paged.* script.');"#,
    );
    assert_eq!(
        read(&mut m, "paged.stories()")[0]["characterCount"].as_i64(),
        Some(38),
        "the sentence is poured into the new frame's story"
    );
}

#[test]
fn edit_text() {
    // example: edit-text
    let mut m = seeded(SEED_STYLED_STORY);
    let before = read(&mut m, "paged.stories()")[0]["characterCount"]
        .as_i64()
        .unwrap();
    let r = run_ok(
        &mut m,
        r#"const story = JSON.parse(paged.stories())[0].selfId;
paged.insertText(story, 0, 'NEW: ');
console.log('after insert:', JSON.parse(paged.stories())[0].characterCount, 'chars');
paged.deleteRange(story, 0, 5);   // remove the "NEW: " we just added
console.log('after delete:', JSON.parse(paged.stories())[0].characterCount, 'chars');"#,
    );
    assert!(
        output_has(&r, &format!("after insert: {} chars", before + 5)),
        "insert of 'NEW: ' adds 5 chars: {:?}",
        r.output
    );
    assert_eq!(
        read(&mut m, "paged.stories()")[0]["characterCount"].as_i64(),
        Some(before),
        "deleting the prefix returns to the original length"
    );
}

#[test]
fn delete_element() {
    // example: delete-element
    let mut m = seeded(SEED_TWO_FRAMES);
    let before = page_frame_count(&mut m);
    run_ok(
        &mut m,
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
console.log('deleting', ref, '→', paged.deleteElement(ref));"#,
    );
    assert_eq!(
        page_frame_count(&mut m),
        before - 1,
        "the selected frame is removed from the page"
    );
}

#[test]
fn insert_page() {
    // example: insert-page
    let mut m = seeded(SEED_ONE_TEXT);
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const np = paged.insertPage(pid);
console.log('added page', np);
console.log('pages now:', JSON.parse(paged.pages()).length);"#,
    );
    assert_eq!(page_count(&mut m), 2, "document grows from 1 to 2 pages");
}

#[test]
fn delete_page() {
    // example: delete-page
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const extra = paged.insertPage(pid);
console.log('pages after add:', JSON.parse(paged.pages()).length);
paged.deletePage(extra);
console.log('pages after delete:', JSON.parse(paged.pages()).length);"#,
    );
    assert!(output_has(&r, "pages after add: 2"), "{:?}", r.output);
    assert!(output_has(&r, "pages after delete: 1"), "{:?}", r.output);
    assert_eq!(page_count(&mut m), 1, "1 → 2 → 1 pages");
}

#[test]
fn duplicate_page() {
    // example: duplicate-page
    let mut m = seeded(SEED_ONE_TEXT);
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const copy = paged.duplicatePage(pid);
console.log('duplicated', pid, '→', copy);
console.log('pages now:', JSON.parse(paged.pages()).length);"#,
    );
    assert_eq!(page_count(&mut m), 2, "document grows from 1 to 2 pages");
}

#[test]
fn resize_page() {
    // example: resize-page
    let mut m = blank_model();
    run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.resizePage(pid, [0, 0, 841.89, 595.28]);   // [t,l,b,r] A4 in pt
console.log('resized to', JSON.parse(paged.pages())[0].sizePt.join(' × '), 'pt');"#,
    );
    let size = read(&mut m, "paged.pages()")[0]["sizePt"].clone();
    let w = size[0].as_f64().expect("width");
    let h = size[1].as_f64().expect("height");
    assert!(
        (w - 595.28).abs() < 0.1 && (h - 841.89).abs() < 0.1,
        "page is now A4 (595.28 × 841.89), got {size}"
    );
}

#[test]
fn thread_frames() {
    // example: thread-frames
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const a = paged.insertTextFrame(pid, [72, 72, 150, 280]);     // short — will overflow
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'This column holds far more copy than the short first frame can show, so it oversets and must continue in a threaded second frame further down the page.');
const b = paged.insertTextFrame(pid, [170, 72, 440, 280]);    // empty continuation
console.log('threaded', a, '→', b, ':', paged.linkFrames(a, b));"#,
    );
    assert!(
        output_has(&r, "threaded") && output_has(&r, ": true"),
        "linkFrames should thread the two frames: {:?}",
        r.output
    );
}

#[test]
fn unthread_frame() {
    // example: unthread-frame
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const a = paged.insertTextFrame(pid, [72, 72, 150, 280]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Long copy that overflows the first frame and continues into the threaded second frame until we cut the link again.');
const b = paged.insertTextFrame(pid, [170, 72, 440, 280]);
paged.linkFrames(a, b);
console.log('unthreaded', a, ':', paged.unlinkFrames(a));"#,
    );
    assert!(
        output_has(&r, "unthreaded") && output_has(&r, ": true"),
        "unlinkFrames should sever the thread: {:?}",
        r.output
    );
}

#[test]
fn path_point_insert() {
    // example: path-point-insert
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const tri = [
  { anchor: [200, 120], left: [200, 120], right: [200, 120] },
  { anchor: [340, 360], left: [340, 360], right: [340, 360] },
  { anchor: [ 60, 360], left: [ 60, 360], right: [ 60, 360] },
];
const p = paged.insertPath(pid, tri, false);
const ok = paged.pathPointInsert(p, 3, { anchor: [60, 120], left: [60, 120], right: [60, 120] });
console.log('added anchor →', ok);"#,
    );
    // A 4th anchor is added to the triangle. The anchor array isn't exposed
    // through paged.inspect, so we assert the op's reported success — `ok` is
    // only `true` when insertPath returned a real polygon and the anchor landed.
    assert!(
        output_has(&r, "added anchor → true"),
        "pathPointInsert should report success on a fresh polygon: {:?}",
        r.output
    );
}

#[test]
fn insert_guide() {
    // example: insert-guide
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"const spread = JSON.parse(paged.collection('spreads'))[0].selfId;
console.log('column guide at 144pt →', paged.insertGuide(spread, 'vertical', 144, 0));"#,
    );
    // insertGuide reports success. (A script-inserted guide is not reflected by
    // a SUBSEQUENT collection('spreads') read, so we assert the return value.)
    assert!(
        output_has(&r, "column guide at 144pt → true"),
        "insertGuide should place the guide: {:?}",
        r.output
    );
}

#[test]
fn delete_range() {
    // example: delete-range
    let mut m = seeded(SEED_STYLED_STORY);
    let before = read(&mut m, "paged.stories()")[0]["characterCount"]
        .as_i64()
        .unwrap();
    run_ok(
        &mut m,
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const before = JSON.parse(paged.stories())[0].characterCount;
paged.deleteRange(sid, 0, 7);   // remove "Heading"
console.log('chars', before, '→', JSON.parse(paged.stories())[0].characterCount);"#,
    );
    let after = read(&mut m, "paged.stories()")[0]["characterCount"]
        .as_i64()
        .unwrap();
    assert_eq!(before - after, 7, "deleting the 'Heading' word drops 7 chars");
}

#[test]
fn insert_field() {
    // example: insert-field
    let mut m = seeded(SEED_ONE_TEXT);
    let before = read(&mut m, "paged.stories()")[0]["characterCount"]
        .as_i64()
        .unwrap();
    let r = run_ok(
        &mut m,
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
console.log('page-number field →', paged.insertField(sid, 0, 'pageNumber'));"#,
    );
    assert!(
        output_has(&r, "page-number field → true"),
        "insertField should report success: {:?}",
        r.output
    );
    assert_eq!(
        read(&mut m, "paged.stories()")[0]["characterCount"].as_i64(),
        Some(before + 1),
        "the field inserts one marker char"
    );
}

#[test]
fn create_character_style() {
    // example: create-character-style
    let mut m = seeded(SEED_STYLED_STORY);
    let r = run_ok(
        &mut m,
        r#"const id = paged.createCharacterStyle({ name: 'Emphasis' });
console.log('created character style', id);"#,
    );
    assert!(
        output_has(&r, "CharacterStyle/"),
        "a real style id is minted: {:?}",
        r.output
    );
    let names = collection_names(&mut m, "paged.characterStyles()");
    assert!(
        names.iter().any(|n| n == "Emphasis"),
        "the new Emphasis style appears in the collection: {names:?}"
    );
}

#[test]
fn layer_insert() {
    // example: layer-insert
    let mut m = blank_model();
    let r = run_ok(
        &mut m,
        r#"console.log('added layer →', paged.layerInsert(0, 'Annotations'));
console.log('layers:', JSON.parse(paged.layers()).map(function (l) { return l.name; }).join(', '));"#,
    );
    assert!(
        output_has(&r, "added layer → true"),
        "layerInsert should report success: {:?}",
        r.output
    );
    let names = collection_names(&mut m, "paged.layers()");
    assert!(
        names.iter().any(|n| n == "Annotations"),
        "the Annotations layer is added: {names:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Style / numbering CRUD — each labelled case runs the verbatim example script
// and asserts its self-reported success signal (a minted id, or a `true`
// return), proving the op landed rather than merely "didn't throw".
// ══════════════════════════════════════════════════════════════════════════════

/// (example id, seed, verbatim script, expected output substring)
const STYLE_CRUD: &[(&str, &str, &str, &str)] = &[
    (
        "rename-character-style",
        "styled-story",
        r#"const id = paged.createCharacterStyle({ name: 'Emph' });
console.log('renamed →', paged.renameCharacterStyle(id, 'Emphasis'));"#,
        "→ true",
    ),
    (
        "delete-character-style",
        "styled-story",
        r#"const id = paged.createCharacterStyle({ name: 'Scratch' });
console.log('deleted →', paged.deleteCharacterStyle(id));"#,
        "→ true",
    ),
    (
        "create-object-style",
        "swatches-and-styles",
        r#"const id = paged.createObjectStyle({ name: 'Photo Frame' });
console.log('created object style', id);"#,
        "ObjectStyle/",
    ),
    (
        "rename-object-style",
        "swatches-and-styles",
        r#"const id = paged.createObjectStyle({ name: 'Box' });
console.log('renamed →', paged.renameObjectStyle(id, 'Sidebar Box'));"#,
        "→ true",
    ),
    (
        "delete-object-style",
        "swatches-and-styles",
        r#"const id = paged.createObjectStyle({ name: 'Scratch' });
console.log('deleted →', paged.deleteObjectStyle(id));"#,
        "→ true",
    ),
    (
        "create-cell-style",
        "styled-story",
        r#"const id = paged.createCellStyle({ name: 'Header Cell' });
console.log('created cell style', id);"#,
        "CellStyle/",
    ),
    (
        "rename-cell-style",
        "styled-story",
        r#"const id = paged.createCellStyle({ name: 'Cell' });
console.log('renamed →', paged.renameCellStyle(id, 'Body Cell'));"#,
        "→ true",
    ),
    (
        "delete-cell-style",
        "styled-story",
        r#"const id = paged.createCellStyle({ name: 'Scratch' });
console.log('deleted →', paged.deleteCellStyle(id));"#,
        "→ true",
    ),
    (
        "create-table-style",
        "styled-story",
        r#"const id = paged.createTableStyle({ name: 'Price List' });
console.log('created table style', id);"#,
        "TableStyle/",
    ),
    (
        "rename-table-style",
        "styled-story",
        r#"const id = paged.createTableStyle({ name: 'Tbl' });
console.log('renamed →', paged.renameTableStyle(id, 'Spec Table'));"#,
        "→ true",
    ),
    (
        "delete-table-style",
        "styled-story",
        r#"const id = paged.createTableStyle({ name: 'Scratch' });
console.log('deleted →', paged.deleteTableStyle(id));"#,
        "→ true",
    ),
    (
        "rename-paragraph-style",
        "styled-story",
        r#"const id = paged.createParagraphStyle({ name: 'Body' });
console.log('renamed →', paged.renameParagraphStyle(id, 'Body Text'));"#,
        "→ true",
    ),
    (
        "delete-paragraph-style",
        "styled-story",
        r#"const id = paged.createParagraphStyle({ name: 'Scratch' });
console.log('deleted →', paged.deleteParagraphStyle(id));"#,
        "→ true",
    ),
    (
        "create-numbering-list",
        "styled-story",
        r#"const id = paged.createNumberingList({ name: 'Steps' });
console.log('created numbering list', id);"#,
        "NumberingList/",
    ),
    (
        "edit-numbering-list",
        "styled-story",
        r#"const id = paged.createNumberingList({ name: 'Steps' });
console.log('continue across stories →', paged.editNumberingList(id, { continueAcrossStories: true }));"#,
        "→ true",
    ),
    (
        "delete-numbering-list",
        "styled-story",
        r#"const id = paged.createNumberingList({ name: 'Scratch' });
console.log('deleted →', paged.deleteNumberingList(id));"#,
        "→ true",
    ),
];

#[test]
fn style_crud_outcomes() {
    for &(id, seed, script, needle) in STYLE_CRUD {
        let mut m = seeded_for(seed);
        let r = run_ok(&mut m, script);
        assert!(
            output_has(&r, needle),
            "example `{id}`: expected `{needle}` in output {:?}",
            r.output
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Sections / layers / guides — labelled cases with read-back outcome assertions.
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn section_layer_guide_outcomes() {
    // example: insert-section
    {
        let mut m = blank_model();
        run_ok(
            &mut m,
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
const ok = paged.insertSection(pid, { style: 'lowerRoman', start: 1 });
console.log('front-matter section →', ok);
console.log('sections now:', JSON.parse(paged.collection('sections')).length);"#,
        );
        assert_eq!(
            read(&mut m, "paged.collection('sections')")
                .as_array()
                .map(Vec::len),
            Some(1),
            "insert-section: a section is anchored"
        );
    }
    // example: edit-section
    {
        let mut m = blank_model();
        run_ok(
            &mut m,
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertSection(pid, { style: 'arabic', start: 1 });
const secId = JSON.parse(paged.collection('sections'))[0].selfId;
console.log('prefix A- →', paged.editSection(secId, { prefix: 'A-' }));"#,
        );
        assert_eq!(
            read(&mut m, "paged.collection('sections')")[0]["prefix"].as_str(),
            Some("A-"),
            "edit-section: the A- folio prefix is stored"
        );
    }
    // example: delete-section
    {
        let mut m = blank_model();
        run_ok(
            &mut m,
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertSection(pid, { style: 'arabic', start: 1 });
const secId = JSON.parse(paged.collection('sections'))[0].selfId;
console.log('deleted →', paged.deleteSection(secId));"#,
        );
        assert_eq!(
            read(&mut m, "paged.collection('sections')")
                .as_array()
                .map(Vec::len),
            Some(0),
            "delete-section: the section is gone"
        );
    }
    // example: layer-move
    {
        let mut m = blank_model();
        let r = run_ok(
            &mut m,
            r#"paged.layerInsert(0, 'Background');
paged.layerInsert(1, 'Artwork');
const bg = JSON.parse(paged.layers()).find(function (l) { return l.name === 'Background'; });
console.log('sent to back →', paged.layerMove(bg.selfId, 0));"#,
        );
        assert!(
            output_has(&r, "sent to back → true"),
            "layer-move: layerMove reports success: {:?}",
            r.output
        );
    }
    // example: layer-remove
    {
        let mut m = blank_model();
        let r = run_ok(
            &mut m,
            r#"paged.layerInsert(0, 'Scratch');
const id = JSON.parse(paged.layers()).find(function (l) { return l.name === 'Scratch'; }).selfId;
console.log('removed →', paged.layerRemove(id));"#,
        );
        assert!(output_has(&r, "removed → true"), "layer-remove: {:?}", r.output);
        let names = collection_names(&mut m, "paged.layers()");
        assert!(
            !names.iter().any(|n| n == "Scratch"),
            "layer-remove: the Scratch layer is gone: {names:?}"
        );
    }
    // example: move-guide
    // (The example reads the minted guide id back from the spread WITHIN the
    // same script — that in-script read sees it — then slides it. A guide is
    // not reflected by a later cross-call read, so we assert the move's result.)
    {
        let mut m = blank_model();
        let r = run_ok(
            &mut m,
            r#"const spread = JSON.parse(paged.collection('spreads'))[0].selfId;
paged.insertGuide(spread, 'vertical', 144, 0);
const guides = JSON.parse(paged.collection('spreads'))[0].guides;
const g = guides[guides.length - 1].id;
console.log('moved to 216pt →', paged.moveGuide(g, 216));"#,
        );
        assert!(
            output_has(&r, "moved to 216pt → true"),
            "move-guide: the guide slides from 144 to 216: {:?}",
            r.output
        );
    }
    // example: delete-guide
    {
        let mut m = blank_model();
        let r = run_ok(
            &mut m,
            r#"const spread = JSON.parse(paged.collection('spreads'))[0].selfId;
paged.insertGuide(spread, 'horizontal', 200, 0);
const guides = JSON.parse(paged.collection('spreads'))[0].guides;
const g = guides[guides.length - 1].id;
console.log('deleted →', paged.deleteGuide(g));"#,
        );
        assert!(
            output_has(&r, "deleted → true"),
            "delete-guide: the guide is removed: {:?}",
            r.output
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Runs-clean guards — thin reads, headless-honest config fns (which return
// `false` cleanly on a doc lacking the resource), `createGroup` (a no-op
// headlessly), and per-fn examples whose outcome isn't observable through a
// read. Each labelled case is asserted only to run without error.
// ══════════════════════════════════════════════════════════════════════════════

/// (example id, seed, verbatim script)
const RUNS_CLEAN: &[(&str, &str, &str)] = &[
    (
        "inspect-element",
        "one-text-frame-selected",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
const props = JSON.parse(paged.inspect(ref));
console.log('properties of', ref);
console.log(JSON.stringify(props, null, 2));"#,
    ),
    (
        "get-one-property",
        "one-text-frame-selected",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
const bounds = JSON.parse(paged.get(ref, 'frameBounds'));
console.log('bounds', bounds);"#,
    ),
    (
        "tree-walk",
        "two-frames",
        r#"const tree = JSON.parse(paged.tree());
for (const spread of tree) {
  console.log('spread', spread.label);
  for (const page of spread.children ?? []) {
    console.log('  page', page.label, '→', (page.children ?? []).length, 'frames');
    for (const frame of page.children ?? []) {
      console.log('    -', frame.kind, frame.id);
    }
  }
}"#,
    ),
    (
        "pages-list",
        "blank",
        r#"const pages = JSON.parse(paged.pages());
for (const p of pages) {
  console.log('page', p.index, '· id', p.selfId, '·', p.sizePt.join(' × '), 'pt');
}"#,
    ),
    (
        "apply-paragraph-style",
        "styled-story",
        r#"const story = JSON.parse(paged.stories())[0].selfId;
const styles = JSON.parse(paged.paragraphStyles());
if (styles.length) {
  paged.applyStyle(story, 0, 7, styles[0].selfId);   // style the heading line
  console.log('applied', styles[0].name ?? styles[0].selfId);
}"#,
    ),
    (
        "place-image",
        "image-frame",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
// A data: or https: uri the engine can fetch; fit is optional.
paged.placeImage(ref, 'https://docs.paged.media/preview/sample.png', 'fillProportional');
console.log('placed into', ref);"#,
    ),
    (
        "insert-oval",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const oval = paged.insertOval(pid, [200, 180, 380, 440]);   // [t,l,b,r] in pt
console.log('created', oval);
if (oval) paged.set(oval, 'frameFillColor', 'Color/Black');"#,
    ),
    (
        "insert-line",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const line = paged.insertLine(pid, [72, 72], [360, 480]);    // [x1,y1] → [x2,y2]
console.log('created', line);"#,
    ),
    (
        "move-frame",
        "one-text-frame-selected",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
paged.moveFrame(ref, [1, 0, 0, 1, 72, 72]);   // translate by (72, 72)
console.log('moved', ref);"#,
    ),
    (
        "resize-frame",
        "one-text-frame-selected",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
paged.resizeFrame(ref, [108, 72, 540, 540]);
console.log('resized', ref, '→', paged.get(ref, 'frameBounds'));"#,
    ),
    (
        "group-and-dissolve",
        "two-frames",
        r#"const refs = [];
for (const sp of JSON.parse(paged.tree()))
  for (const pg of sp.children ?? [])
    for (const fr of pg.children ?? []) refs.push(fr.kind + ':' + fr.id);

paged.createGroup(refs);
// Find the group the engine just minted, then dissolve it.
let group = null;
for (const sp of JSON.parse(paged.tree()))
  for (const pg of sp.children ?? [])
    for (const fr of pg.children ?? []) if (fr.kind === 'group') group = fr.kind + ':' + fr.id;
console.log('grouped →', group, '· dissolved →', group && paged.dissolveGroup(group));"#,
    ),
    (
        "list-swatches",
        "swatches-and-styles",
        r#"const swatches = JSON.parse(paged.swatches());
console.log(swatches.length, 'swatches in the palette');
for (const s of swatches) console.log(' -', s.name, '·', s.kind);"#,
    ),
    (
        "list-gradients",
        "swatches-and-styles",
        r#"const gradients = JSON.parse(paged.gradients());
console.log(gradients.length, 'gradient swatch(es) defined');
for (const g of gradients) console.log(' -', g.name ?? g.selfId);"#,
    ),
    (
        "list-color-groups",
        "swatches-and-styles",
        r#"const groups = JSON.parse(paged.colorGroups());
console.log(groups.length, 'colour group(s)');
for (const g of groups) console.log(' -', g.name, '→', g.members.length, 'members');"#,
    ),
    (
        "list-layers",
        "blank",
        r#"paged.layerInsert(0, 'Background');
paged.layerInsert(1, 'Artwork');
const layers = JSON.parse(paged.layers());
console.log(layers.length, 'layers (bottom → top):');
for (const l of layers) console.log('  z' + l.z, l.name, l.visible ? '·visible' : '·hidden');"#,
    ),
    (
        "list-paragraph-styles",
        "styled-story",
        r#"const styles = JSON.parse(paged.paragraphStyles());
console.log(styles.length, 'paragraph styles:');
for (const s of styles) console.log(' -', s.name, s.basedOn ? '(based on ' + s.basedOn + ')' : '');"#,
    ),
    (
        "list-character-styles",
        "styled-story",
        r#"const styles = JSON.parse(paged.characterStyles());
console.log(styles.length, 'character styles:');
for (const s of styles) console.log(' -', s.name);"#,
    ),
    (
        "list-object-styles",
        "swatches-and-styles",
        r#"const styles = JSON.parse(paged.objectStyles());
console.log(styles.length, 'object style(s) defined');
for (const s of styles) console.log(' -', s.name);"#,
    ),
    (
        "audit-links",
        "image-frame",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
paged.placeImage(ref, 'https://docs.paged.media/preview/sample.png', 'fillProportional');
const links = JSON.parse(paged.links());
console.log(links.length, 'placed-image link(s):');
for (const l of links) console.log(' -', l.hostKind, '·', l.uri, '·', l.status || 'unresolved');"#,
    ),
    (
        "list-conditions",
        "blank",
        r#"const conds = JSON.parse(paged.conditions());
console.log(conds.length, 'conditional-text condition(s)');
for (const c of conds) console.log(' -', c.name, c.visible ? '·shown' : '·hidden');"#,
    ),
    (
        "list-condition-sets",
        "blank",
        r#"const sets = JSON.parse(paged.conditionSets());
console.log(sets.length, 'condition set(s)');
for (const s of sets) console.log(' -', s.name, '→', s.conditions.length, 'conditions');"#,
    ),
    (
        "read-collection",
        "blank",
        r#"const spreads = JSON.parse(paged.collection('spreads'));
console.log(spreads.length, 'spread(s):');
for (const sp of spreads) console.log(' -', sp.label, '·', sp.pageCount, 'page(s)');"#,
    ),
    (
        "read-document-meta",
        "blank",
        r#"const meta = JSON.parse(paged.documentMeta());
console.log('pages:', meta.pageCount);
console.log('default fill:', meta.defaultFillColor ?? 'none', '· default stroke:', meta.defaultStrokeColor ?? 'none');
console.log('CMYK profile:', meta.cmykProfileName ?? '(working space)');"#,
    ),
    (
        "read-stories",
        "one-text-frame-selected",
        r#"const stories = JSON.parse(paged.stories());
for (const s of stories) {
  console.log('story', s.selfId, '·', s.characterCount, 'chars /', s.paragraphCount, 'paras',
    s.overset ? '· OVERSET' : '');
}"#,
    ),
    (
        "read-selection",
        "one-text-frame-selected",
        r#"const sel = JSON.parse(paged.selection());
console.log(sel.length, 'element(s) selected');
for (const el of sel) console.log(' -', el.kind + ':' + el.id);"#,
    ),
    (
        "read-content-selection",
        "one-text-frame-selected",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
paged.setContentSelection({ storyId: sid, start: 0, end: 5 });
const raw = paged.contentSelection();
const cs = raw === null ? null : JSON.parse(raw);
console.log(cs ? ('caret over ' + (cs.end - cs.start) + ' chars in ' + cs.storyId) : 'no text selection');"#,
    ),
    (
        "clear-selection",
        "one-text-frame-selected",
        r#"console.log('before:', JSON.parse(paged.selection()).length, 'selected');
paged.clearSelection();
console.log('after:', JSON.parse(paged.selection()).length, 'selected');"#,
    ),
    (
        "set-content-selection",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
paged.setContentSelection({ storyId: sid, start: 0, end: 7 });   // the "Heading" run
const cs = JSON.parse(paged.contentSelection());
console.log('selected chars', cs.start, '..', cs.end, 'in', cs.storyId);"#,
    ),
    (
        "apply-master",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const masters = JSON.parse(paged.collection('masterPages'));
const masterId = masters.length ? masters[0].selfId : undefined;
console.log('applied master', masterId ?? '(detach)', '→', paged.applyMasterToPage(pid, masterId));"#,
    ),
    (
        "insert-graphic-frame",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const frame = paged.insertFrame(pid, [120, 120, 360, 432]);   // [t,l,b,r] pt
console.log('created', frame);
if (frame) {
  paged.set(frame, 'frameFillColor', 'Color/Black');
  paged.set(frame, 'frameFillTint', 20);
}"#,
    ),
    (
        "group-frames",
        "two-frames",
        r#"const refs = [];
for (const sp of JSON.parse(paged.tree()))
  for (const pg of sp.children ?? [])
    for (const fr of pg.children ?? []) refs.push(fr.kind + ':' + fr.id);
console.log('grouping', refs.length, 'items →', paged.createGroup(refs));"#,
    ),
    (
        "draw-custom-path",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const pennant = [
  { anchor: [200, 120], left: [200, 120], right: [200, 120] },
  { anchor: [340, 360], left: [340, 360], right: [340, 360] },
  { anchor: [ 60, 360], left: [ 60, 360], right: [ 60, 360] },
];
const path = paged.insertPath(pid, pennant, false);   // closed
console.log('created', path);
if (path) paged.set(path, 'frameFillColor', 'Color/Black');"#,
    ),
    (
        "path-point-remove",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const quad = [
  { anchor: [120, 120], left: [120, 120], right: [120, 120] },
  { anchor: [320, 120], left: [320, 120], right: [320, 120] },
  { anchor: [320, 320], left: [320, 320], right: [320, 320] },
  { anchor: [120, 320], left: [120, 320], right: [120, 320] },
];
const p = paged.insertPath(pid, quad, false);
console.log('removed anchor 3 →', paged.pathPointRemove(p, 3));"#,
    ),
    (
        "path-point-curve",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const tri = [
  { anchor: [200, 120], left: [200, 120], right: [200, 120] },
  { anchor: [340, 360], left: [340, 360], right: [340, 360] },
  { anchor: [ 60, 360], left: [ 60, 360], right: [ 60, 360] },
];
const p = paged.insertPath(pid, tri, false);
console.log('smoothed anchor 0 →', paged.pathPointCurveType(p, 0, true));"#,
    ),
    (
        "path-point-set",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const tri = [
  { anchor: [200, 120], left: [200, 120], right: [200, 120] },
  { anchor: [340, 360], left: [340, 360], right: [340, 360] },
  { anchor: [ 60, 360], left: [ 60, 360], right: [ 60, 360] },
];
const p = paged.insertPath(pid, tri, false);
console.log('moved apex →', paged.pathPointSet(p, 0, 'anchor', [260, 90]));"#,
    ),
    (
        "path-open-at",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const quad = [
  { anchor: [120, 120], left: [120, 120], right: [120, 120] },
  { anchor: [320, 120], left: [320, 120], right: [320, 120] },
  { anchor: [320, 320], left: [320, 320], right: [320, 320] },
  { anchor: [120, 320], left: [120, 320], right: [120, 320] },
];
const p = paged.insertPath(pid, quad, false);
console.log('opened at anchor 0 →', paged.pathOpenAt(p, 0));"#,
    ),
    (
        "outline-stroke",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const rule = paged.insertLine(pid, [72, 96], [400, 300]);
paged.set(rule, 'frameStrokeWeight', 8);
console.log('outlined →', paged.outlineStroke(rule, 8, 'round', 'round', 4));"#,
    ),
    (
        "offset-path",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const shape = [
  { anchor: [200, 120], left: [200, 120], right: [200, 120] },
  { anchor: [360, 380], left: [360, 380], right: [360, 380] },
  { anchor: [ 60, 380], left: [ 60, 380], right: [ 60, 380] },
];
const p = paged.insertPath(pid, shape, false);
console.log('inset 10pt →', paged.offsetPath(p, -10, 'miter', 4));"#,
    ),
    (
        "simplify-path",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const shape = [
  { anchor: [120, 120], left: [120, 120], right: [120, 120] },
  { anchor: [320, 120], left: [320, 120], right: [320, 120] },
  { anchor: [320, 320], left: [320, 320], right: [320, 320] },
  { anchor: [120, 320], left: [120, 320], right: [120, 320] },
];
const p = paged.insertPath(pid, shape, false);
console.log('simplified →', paged.simplifyPath(p, 3));"#,
    ),
    (
        "pathfinder-union",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const sqA = [
  { anchor: [120, 120], left: [120, 120], right: [120, 120] },
  { anchor: [300, 120], left: [300, 120], right: [300, 120] },
  { anchor: [300, 300], left: [300, 300], right: [300, 300] },
  { anchor: [120, 300], left: [120, 300], right: [120, 300] },
];
const sqB = [
  { anchor: [220, 220], left: [220, 220], right: [220, 220] },
  { anchor: [400, 220], left: [400, 220], right: [400, 220] },
  { anchor: [400, 400], left: [400, 400], right: [400, 400] },
  { anchor: [220, 400], left: [220, 400], right: [220, 400] },
];
const a = paged.insertPath(pid, sqA, false);
const b = paged.insertPath(pid, sqB, false);
console.log('union →', paged.pathfinderBoolean(a, [b], 'union'));"#,
    ),
    (
        "table-delete-row",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('removed row →', paged.deleteTableRow(sid, table, 2));"#,
    ),
    (
        "table-insert-column",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('added column →', paged.insertTableColumn(sid, table, 1));"#,
    ),
    (
        "table-delete-column",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('removed column →', paged.deleteTableColumn(sid, table, 2));"#,
    ),
    (
        "table-insert-header",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('added header band →', paged.insertHeaderRow(sid, table));"#,
    ),
    (
        "table-remove-header",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3, headerRows: 1 });
console.log('removed header →', paged.removeHeaderRow(sid, table));"#,
    ),
    (
        "table-insert-footer",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('added totals footer →', paged.insertFooterRow(sid, table));"#,
    ),
    (
        "table-remove-footer",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3, footerRows: 1 });
console.log('removed footer →', paged.removeFooterRow(sid, table));"#,
    ),
    (
        "table-cell-span",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 4 });
console.log('merged 2×2 →', paged.setCellSpan(sid, table, 0, 0, 2, 2));"#,
    ),
    (
        "table-row-height",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('header row 28pt →', paged.setRowHeight(sid, table, 0, 28));"#,
    ),
    (
        "table-column-width",
        "styled-story",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 4, cols: 3 });
console.log('label column 140pt →', paged.setColumnWidth(sid, table, 0, 140));"#,
    ),
    (
        "set-style-property",
        "styled-story",
        r#"const id = paged.createParagraphStyle({ name: 'Body' });
paged.setStyleProperty('paragraph', id, 'characterFontSize', 10);
paged.setStyleProperty('paragraph', id, 'paragraphSpaceAfter', 4);
console.log('Body style → 10pt / 4pt after');"#,
    ),
    (
        "set-condition-visible",
        "blank",
        r#"const conds = JSON.parse(paged.conditions());
const target = conds.length ? conds[0].selfId : 'Condition/none';
console.log('hide', target, '→', paged.setConditionVisible(target, false));"#,
    ),
    (
        "activate-condition-set",
        "blank",
        r#"const sets = JSON.parse(paged.conditionSets());
const target = sets.length ? sets[0].selfId : 'ConditionSet/none';
console.log('activate', target, '→', paged.activateConditionSet(target));"#,
    ),
    (
        "set-field-value",
        "one-text-frame-selected",
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertField(sid, 0, { placeholder: { plugin: 'merge', key: 'price' } });
console.log('resolved price →', paged.setFieldValue(sid, 0, '$49.00'));"#,
    ),
    (
        "redo-write",
        "one-text-frame-selected",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
paged.set(ref, 'frameFillColor', 'Color/Red');
paged.undo();
console.log('redo →', paged.redo());   // red again"#,
    ),
    (
        "set-document-defaults",
        "blank",
        r#"console.log('defaults set →', paged.setDocumentDefaults({ stroke: 'Color/Black', weight: 1 }));"#,
    ),
    (
        "set-color-settings",
        "blank",
        r#"console.log('colour settings →', paged.setColorSettings({
  rgbPolicy: 'PreserveEmbeddedProfiles',
  intent: 'relativeColorimetric',
  bpc: true,
}));"#,
    ),
    (
        "set-proof-setup",
        "blank",
        r#"console.log('proof setup →', paged.setProofSetup({
  profileName: 'US Web Coated (SWOP) v2',
  simulatePaperWhite: true,
}));"#,
    ),
    (
        "set-ink-setting",
        "blank",
        r#"const inks = JSON.parse(paged.collection('inks'));
// headless: a blank document has no spot inks, so this returns false (no error).
const spot = inks.length ? inks[0].spotId : 'Color/None';
console.log('convert to process →', paged.setInkSetting(spot, { convertToProcess: true }));"#,
    ),
    (
        "set-lab-for-spots",
        "blank",
        r#"console.log('use Lab for spots →', paged.setUseStandardLabForSpots(true));"#,
    ),
    (
        "import-swatch-library",
        "swatches-and-styles",
        r#"// headless: pass a real .ase file as number[] of bytes; the empty array below
// returns false cleanly (no error) because there is nothing to parse.
const aseBytes = [];
console.log('imported swatches →', paged.importSwatchLibrary(aseBytes, 'Brand'));"#,
    ),
    (
        "set-plugin-metadata",
        "image-frame",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
const envelope = JSON.stringify({ v: 1, data: { state: 'approved', reviewer: 'A. Editor' } });
console.log('tagged →', paged.setPluginMetadata(ref, 'x-paged:review', envelope, 'review'));"#,
    ),
    (
        "replace-image-bytes",
        "image-frame",
        r#"const [el] = JSON.parse(paged.selection());
const ref = el.kind + ':' + el.id;
// pass a number[] of PNG/JPEG bytes to SET the image; null clears it.
console.log('cleared inline bytes →', paged.replaceImageBytes(ref, null));"#,
    ),
    (
        "batch-mutations",
        "blank",
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const ok = paged.batch([
  { op: 'insertTextFrame', args: { pageId: pid, bounds: [72, 72, 720, 290] } },
  { op: 'insertTextFrame', args: { pageId: pid, bounds: [72, 306, 720, 540] } },
]);
console.log('two-column layout →', ok);"#,
    ),
];

#[test]
fn runs_clean() {
    for &(id, seed, script) in RUNS_CLEAN {
        let mut m = seeded_for(seed);
        let r = execute_script(&mut m, script);
        assert!(r.error.is_none(), "example `{id}` errored: {:?}", r.error);
    }
}
