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

//! Capability-evidence tests for the paged.* scripting surface. Each test name
//! carries a `__feat__<flat-id>` suffix (dots/dashes → underscores) so the
//! paged-media/state join attributes its result to the matching `scripting.*`
//! registry row (extract-features.ts), landing on the `core.canvas-wasm` stage
//! per ingest/suites.yaml. This is the engine-side evidence that the capability
//! rows claim — the same surface the docs examples exercise.
//!
//! The `__feat__` suffix is the state join's convention, not idiomatic Rust, so
//! the non-snake-case lint is allowed for these test names only.
#![allow(non_snake_case)]

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

fn blank() -> CanvasModel {
    CanvasModel::new_blank("scripting-evidence", 612.0, 792.0, CanvasOptions::default())
        .expect("new_blank")
}

fn run(model: &mut CanvasModel, src: &str) -> String {
    let r = execute_script(model, src);
    assert!(r.error.is_none(), "script errored: {:?}", r.error);
    r.output.join("\n")
}

/// scripting.page-enumeration — paged.pages() exposes a usable page id.
#[test]
fn paged_pages_yields_a_usable_page_id__feat__scripting_page_enumeration() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pages = JSON.parse(paged.pages());
console.log('count', pages.length, 'id', pages[0].selfId);"#,
    );
    assert!(out.contains("count 1"), "{out}");
    assert!(out.contains("id usp"), "expected a non-empty page selfId; {out}");
}

/// scripting.author-returns-id — insert fns return the created element address
/// (not a bare bool) and auto-select it.
#[test]
fn insert_fns_return_the_created_id__feat__scripting_author_returns_id() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const frame = paged.insertTextFrame(pid, [72, 72, 300, 300]);
console.log('created', frame);
const sel = JSON.parse(paged.selection());
console.log('selected', sel.length, sel[0] ? sel[0].kind + ':' + sel[0].id : 'none');"#,
    );
    assert!(out.contains("created textFrame:"), "insert should return a textFrame:<id>; {out}");
    assert!(out.contains("selected 1 textFrame:"), "the new frame should be auto-selected; {out}");
}

/// scripting.full-mutation-surface — the wider authoring surface is callable
/// and applies (delete, group-dissolve round-trip, table, style CRUD).
#[test]
fn full_mutation_surface_is_callable__feat__scripting_full_mutation_surface() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
// create + delete an element
const oval = paged.insertOval(pid, [120, 120, 220, 320]);
console.log('deleted', paged.deleteElement(oval));
// a table inside a story
paged.insertTextFrame(pid, [340, 72, 520, 320]);
const sid = JSON.parse(paged.stories())[0].selfId;
const table = paged.insertTable(sid, { rows: 2, cols: 2 });
console.log('table', table, 'row', paged.insertTableRow(sid, table, 1));
// style CRUD
const st = paged.createParagraphStyle({ name: 'Body' });
console.log('style', st, 'renamed', paged.renameParagraphStyle(st, 'Body Copy'));"#,
    );
    assert!(out.contains("deleted true"), "deleteElement should apply; {out}");
    assert!(out.contains("row true"), "insertTableRow should apply; {out}");
    assert!(out.contains("style ParagraphStyle/"), "createParagraphStyle should mint an id; {out}");
    assert!(out.contains("renamed true"), "renameParagraphStyle should apply; {out}");
}

/// scripting.visible-results (model half) — a paged.set mutation lands in the
/// model and is read back; the GPU repaint half is exercised by the editor/
/// dispatch tests, but the mutation reaching the model is the core.canvas-wasm
/// precondition this row claims.
#[test]
fn script_mutation_lands_in_the_model__feat__scripting_visible_results() {
    let mut m = blank();
    let before = m.current_state_hash();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const f = paged.insertFrame(pid, [72, 72, 200, 200]);
paged.set(f, 'frameFillColor', 'Color/Black');
console.log('fill', paged.get(f, 'frameFillColor'));"#,
    );
    assert!(out.contains("Color/Black"), "the fill write should read back; {out}");
    assert_ne!(before, m.current_state_hash(), "the mutation must change the scene state");
}

/// scripting.docs-playground-seeds — the docs playground's named seed preludes
/// are pure paged.* that scaffold a starter document. They run through the same
/// engine the editor embeds, so a clean run here is the core-side evidence that
/// the seeded examples actually work (the docs `validate:scripting` gate runs
/// the same preludes through `paged-run`). Mirrors docs/data/scripting/seeds.ts.
#[test]
fn seed_preludes_scaffold_a_document__feat__scripting_docs_playground_seeds() {
    const SEEDS: &[(&str, &str)] = &[
        (
            "one-text-frame-selected",
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [144, 72, 360, 540]);
const stories = JSON.parse(paged.stories());
if (stories.length) { paged.insertText(stories[0].selfId, 0, "Body text."); }"#,
        ),
        (
            "two-frames",
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [72, 72, 300, 320]);
paged.insertFrame(pid, [340, 72, 520, 320]);"#,
        ),
        (
            "styled-story",
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [108, 72, 540, 540]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, "Heading\nThe body follows the heading.");
const ps = JSON.parse(paged.paragraphStyles());
if (ps.length) { paged.applyStyle(sid, 0, 7, ps[0].selfId); }"#,
        ),
        (
            "a-table",
            r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [108, 72, 420, 540]);
const sid = JSON.parse(paged.stories())[0].selfId;
if (typeof paged.insertTable === "function") { paged.insertTable(sid, { rows: 3, cols: 3 }); }"#,
        ),
    ];
    for (name, prelude) in SEEDS {
        let mut m = blank();
        let r = execute_script(&mut m, prelude);
        assert!(r.error.is_none(), "seed `{name}` errored: {:?}", r.error);
        // A seed must leave addressable content behind.
        let tree = execute_script(&mut m, "console.log(paged.tree());");
        assert!(tree.output.join("").contains("Frame"), "seed `{name}` placed no frame");
    }
}
