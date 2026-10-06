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

//! The paged.web bake contract: a whole web frame materialised as native
//! content in ONE `batch` — one undo step, every created id returned.
//!
//! Nothing here is new engine surface. The bake still issued, per text run,
//! `insertTextFrame`, two `collection("stories")` reads to find the minted
//! story by diffing, `insertText` and two `setElementProperty`s (four undo
//! steps per run), because it predates the C-15 handles (v57): a
//! `bindCreated` child names what the previous child minted and a later
//! `storyId` / `story_id` position addresses the text frame's minted story.
//! This file pins that the bake's exact wire shape — swatches, a rectangle,
//! a path and text frames with content, size, fill and face — applies as one
//! step, reports every mint with its handle and story, and undoes and redoes
//! as a unit. It is written in the JSON the bundle sends, not in Rust
//! literals, so the test fails if the wire spelling the bundle relies on
//! drifts.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};

fn doc_bytes() -> Vec<u8> {
    paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap()
}

fn first_page_id(m: &CanvasModel) -> String {
    m.built().pages[0].id.0.clone()
}

/// One web frame's bake: 2 swatches, 1 rect, 1 path, 2 text runs.
fn bake_batch(page: &str) -> Mutation {
    let run = |handle: &str, top: f32, text: &str, size: f32, swatch: &str, style: &str| {
        serde_json::json!([
            { "op": "insertTextFrame", "args": { "pageId": page, "bounds": [top, 40.0, top + size * 1.4, 300.0] } },
            { "op": "bindCreated", "args": { "handle": handle } },
            { "op": "insertText", "args": { "storyId": format!("$h:{handle}"), "offset": 0, "text": text } },
            { "op": "setElementProperty", "args": {
                "elementId": { "kind": "storyRange", "id": { "story_id": format!("$h:{handle}"), "start": 0, "end": text.chars().count() } },
                "path": "characterFontSize", "value": { "type": "length", "value": size } } },
            { "op": "setElementProperty", "args": {
                "elementId": { "kind": "storyRange", "id": { "story_id": format!("$h:{handle}"), "start": 0, "end": text.chars().count() } },
                "path": "characterFillColor", "value": { "type": "colorRef", "value": swatch } } },
            { "op": "setElementProperty", "args": {
                "elementId": { "kind": "storyRange", "id": { "story_id": format!("$h:{handle}"), "start": 0, "end": text.chars().count() } },
                "path": "characterFontFamily", "value": { "type": "text", "value": "Inter" } } },
            { "op": "setElementProperty", "args": {
                "elementId": { "kind": "storyRange", "id": { "story_id": format!("$h:{handle}"), "start": 0, "end": text.chars().count() } },
                "path": "characterFontStyle", "value": { "type": "text", "value": style } } }
        ])
    };
    let mut ops = vec![
        serde_json::json!({ "op": "createSwatch", "args": { "spec": {
            "selfId": "Color/web-c0", "name": "web-c0", "space": "RGB", "value": [12.0, 34.0, 200.0], "model": "Process" } } }),
        serde_json::json!({ "op": "createSwatch", "args": { "spec": {
            "selfId": "Color/web-c1", "name": "web-c1", "space": "RGB", "value": [240.0, 240.0, 230.0], "model": "Process" } } }),
        serde_json::json!({ "op": "insertFrame", "args": { "pageId": page, "bounds": [20.0, 20.0, 120.0, 320.0] } }),
        serde_json::json!({ "op": "bindCreated", "args": { "handle": "bg" } }),
        serde_json::json!({ "op": "setElementProperty", "args": {
            "elementId": { "kind": "rectangle", "id": "$h:bg" },
            "path": "frameFillColor", "value": { "type": "colorRef", "value": "Color/web-c1" } } }),
        serde_json::json!({ "op": "insertPath", "args": { "pageId": page, "open": false, "anchors": [
            { "anchor": [20.0, 130.0], "left": [20.0, 130.0], "right": [20.0, 130.0] },
            { "anchor": [80.0, 130.0], "left": [80.0, 130.0], "right": [80.0, 130.0] },
            { "anchor": [50.0, 170.0], "left": [50.0, 170.0], "right": [50.0, 170.0] } ] } }),
        serde_json::json!({ "op": "bindCreated", "args": { "handle": "shape" } }),
        serde_json::json!({ "op": "setElementProperty", "args": {
            "elementId": { "kind": "polygon", "id": "$h:shape" },
            "path": "frameFillColor", "value": { "type": "colorRef", "value": "Color/web-c0" } } }),
    ];
    for v in [
        run("t0", 30.0, "Heading", 24.0, "Color/web-c0", "Bold"),
        run("t1", 70.0, "Body copy", 11.0, "Color/web-c0", "Italic"),
    ] {
        ops.extend(v.as_array().unwrap().iter().cloned());
    }
    serde_json::from_value(serde_json::json!({ "op": "batch", "args": { "ops": ops } }))
        .expect("the bake's JSON decodes as a Mutation")
}

fn run_of<'a>(m: &'a CanvasModel, story_id: &str) -> Option<&'a paged_model::CharacterRun> {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story_id)?
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .find(|r| !r.text.is_empty())
}

fn story_text(m: &CanvasModel, story_id: &str) -> Option<String> {
    let s = m.scene().stories.iter().find(|s| s.self_id == story_id)?;
    Some(
        s.story
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter())
            .map(|r| r.text.as_str())
            .collect(),
    )
}

#[test]
fn a_whole_web_bake_is_one_batch_one_undo_step_with_every_id_returned() {
    let mut m = CanvasModel::load("d", &doc_bytes(), CanvasOptions::default()).unwrap();
    let page = first_page_id(&m);
    let log_before = m.applied_log_len();
    let stories_before = m.scene().stories.len();

    let outcome = m
        .apply_mutation(&bake_batch(&page))
        .expect("the bake applies");

    // ONE undo step for the whole frame.
    assert_eq!(m.applied_log_len(), log_before + 1, "one undo step");

    // Every mint is reported, in order, with its handle — and the text
    // frames with the story they minted. No collection read needed.
    let handles: Vec<Option<&str>> = outcome.minted.iter().map(|e| e.handle.as_deref()).collect();
    assert_eq!(
        handles,
        vec![Some("bg"), Some("shape"), Some("t0"), Some("t1")],
        "minted: {:?}",
        outcome.minted
    );
    let story_of = |h: &str| {
        outcome
            .minted
            .iter()
            .find(|e| e.handle.as_deref() == Some(h))
            .and_then(|e| e.story_id.clone())
            .expect("a text frame's mint names its story")
    };
    let (s0, s1) = (story_of("t0"), story_of("t1"));
    assert_ne!(s0, s1);
    assert_eq!(m.scene().stories.len(), stories_before + 2);

    // Content and character attributes landed on the minted stories.
    assert_eq!(story_text(&m, &s0).as_deref(), Some("Heading"));
    assert_eq!(story_text(&m, &s1).as_deref(), Some("Body copy"));
    let r0 = run_of(&m, &s0).expect("run");
    assert_eq!(r0.point_size, Some(24.0));
    assert_eq!(r0.fill_color.as_deref(), Some("Color/web-c0"));
    assert_eq!(r0.font.as_deref(), Some("Inter"));
    assert_eq!(r0.font_style.as_deref(), Some("Bold"));
    let r1 = run_of(&m, &s1).expect("run");
    assert_eq!(r1.point_size, Some(11.0));
    assert_eq!(r1.font_style.as_deref(), Some("Italic"));

    // Undo removes the whole bake in one step …
    m.undo().expect("undo the bake");
    assert_eq!(m.applied_log_len(), log_before);
    assert_eq!(m.scene().stories.len(), stories_before);
    assert!(story_text(&m, &s0).is_none(), "the minted story is gone");

    // … and redo brings it all back.
    m.redo().expect("redo the bake");
    assert_eq!(m.applied_log_len(), log_before + 1);
    assert_eq!(m.scene().stories.len(), stories_before + 2);
    let texts: Vec<String> = m
        .scene()
        .stories
        .iter()
        .filter_map(|s| story_text(&m, &s.self_id))
        .collect();
    assert!(texts.iter().any(|t| t == "Heading") && texts.iter().any(|t| t == "Body copy"));
}

/// The all-translatable lane: a bake of shapes only (no text, so every
/// child translates into ONE `Operation::Batch`) must report its handles
/// exactly as the mixed lane does. The path handle used to come back
/// `None` on this lane.
#[test]
fn a_shapes_only_bake_reports_every_handle_on_the_translate_lane() {
    let mut m = CanvasModel::load("d", &doc_bytes(), CanvasOptions::default()).unwrap();
    let page = first_page_id(&m);
    let path = |left: f32| {
        serde_json::json!({ "op": "insertPath", "args": { "pageId": page, "open": false, "anchors": [
            { "anchor": [left, 130.0], "left": [left, 130.0], "right": [left, 130.0] },
            { "anchor": [left + 60.0, 130.0], "left": [left + 60.0, 130.0], "right": [left + 60.0, 130.0] },
            { "anchor": [left + 30.0, 170.0], "left": [left + 30.0, 170.0], "right": [left + 30.0, 170.0] } ] } })
    };
    let batch: Mutation =
        serde_json::from_value(serde_json::json!({ "op": "batch", "args": { "ops": [
        path(20.0),
        { "op": "bindCreated", "args": { "handle": "p0" } },
        path(120.0),
        { "op": "bindCreated", "args": { "handle": "p1" } },
        { "op": "setElementProperty", "args": {
            "elementId": { "kind": "polygon", "id": "$h:p0" },
            "path": "frameFillColor", "value": { "type": "colorRef", "value": "Color/Black" } } }
    ] } }))
        .expect("decodes");
    let outcome = m.apply_mutation(&batch).expect("applies");
    let handles: Vec<Option<&str>> = outcome.minted.iter().map(|e| e.handle.as_deref()).collect();
    assert_eq!(
        handles,
        vec![Some("p0"), Some("p1")],
        "minted: {:?}",
        outcome.minted
    );
}
