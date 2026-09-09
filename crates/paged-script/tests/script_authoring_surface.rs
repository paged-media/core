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

//! The thirty-one fns that closed the script surface, driven for real.
//!
//! `script_surface.rs` proves the bridge NAMES every wire op. Naming is
//! not reaching: a fn that mints the right variant from the wrong
//! arguments returns `false` forever and the roster still reads full.
//! Two of these were written wrong first and only this file said so —
//! `reorderElement`'s target vocabulary is `front`/`back`/`forward`/
//! `backward`, not the `bringToFront` spelling the first draft
//! documented, and a gradient stop is `{stopColor, locationPct}`, not
//! `{stopColor, location}`.
//!
//! So each test below asserts an OUTCOME — the swatch is in
//! `paged.swatches()`, the layer flags come back flipped in
//! `paged.layers()`, the story survives its detach — rather than that
//! the call returned true.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

fn blank() -> CanvasModel {
    CanvasModel::new_blank("authoring-surface", 612.0, 792.0, CanvasOptions::default())
        .expect("new_blank")
}

fn run(model: &mut CanvasModel, src: &str) -> String {
    let r = execute_script(model, src);
    assert!(r.error.is_none(), "script errored: {:?}", r.error);
    r.output.join("\n")
}

/// The nine colour-resource CRUD fns. Before these, a script could paint
/// with a swatch and not make one.
#[test]
fn colour_resources_are_created_edited_and_deleted_by_name() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const sw = paged.createSwatch({ space: 'CMYK', value: [0, 1, 1, 0], name: 'Vermilion' });
console.log('swatch', sw);
console.log('named', JSON.parse(paged.swatches()).some(s => s.name === 'Vermilion'));
console.log('edited', paged.editSwatch(sw, { space: 'CMYK', value: [0, 0.5, 1, 0], name: 'Amber' }));
console.log('renamed', JSON.parse(paged.swatches()).some(s => s.name === 'Amber'));

const gr = paged.createGradient({ kind: 'Linear', name: 'Dawn', stops: [
  { stopColor: 'Color/Black', locationPct: 0 },
  { stopColor: sw, locationPct: 100 } ] });
console.log('gradient', gr, 'count', JSON.parse(paged.gradients()).length);
console.log('gradEdited', paged.editGradient(gr, { kind: 'Radial', name: 'Dusk', stops: [
  { stopColor: 'Color/Black', locationPct: 0 },
  { stopColor: sw, locationPct: 100 } ] }));

const cg = paged.createColorGroup({ name: 'Inks', members: [sw] });
console.log('group', cg, 'groups', JSON.parse(paged.colorGroups()).length);
console.log('groupEdited', paged.editColorGroup(cg, { name: 'Spot inks', members: [sw] }));

console.log('deletes', paged.deleteColorGroup(cg), paged.deleteGradient(gr), paged.deleteSwatch(sw));
console.log('gone', JSON.parse(paged.swatches()).some(s => s.name === 'Amber'),
            JSON.parse(paged.gradients()).length, JSON.parse(paged.colorGroups()).length);"#,
    );
    assert!(
        out.contains("swatch Color/"),
        "createSwatch mints an id; {out}"
    );
    assert!(
        out.contains("named true"),
        "the swatch reaches the collection; {out}"
    );
    assert!(out.contains("edited true"), "{out}");
    assert!(
        out.contains("renamed true"),
        "the edit reaches the collection; {out}"
    );
    assert!(out.contains("gradient Gradient/"), "{out}");
    assert!(out.contains("count 1"), "{out}");
    assert!(out.contains("gradEdited true"), "{out}");
    assert!(out.contains("group ColorGroup/"), "{out}");
    assert!(out.contains("groups 1"), "{out}");
    assert!(out.contains("groupEdited true"), "{out}");
    assert!(out.contains("deletes true true true"), "{out}");
    assert!(
        out.contains("gone false 0 0"),
        "the three deletes empty their collections; {out}"
    );
}

/// The four layer attribute setters, read back through `paged.layers()`
/// — the four flags a script could previously only flip by knowing the
/// wire spelling and going through `paged.batch`.
#[test]
fn layer_attributes_flip_and_read_back() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"paged.layerInsert(0, 'Art');
const id = JSON.parse(paged.layers())[0].selfId;
console.log('set', paged.layerSetVisible(id, false), paged.layerSetLocked(id, true),
            paged.layerSetPrintable(id, false), paged.layerSetName(id, 'Ink'));
const l = JSON.parse(paged.layers())[0];
console.log('read', l.name, l.visible, l.locked, l.printable);
console.log('back', paged.layerSetVisible(id, true));
console.log('again', JSON.parse(paged.layers())[0].visible);"#,
    );
    assert!(out.contains("set true true true true"), "{out}");
    assert!(
        out.contains("read Ink false true false"),
        "all four flags land on the layer; {out}"
    );
    assert!(out.contains("back true"), "{out}");
    assert!(
        out.contains("again true"),
        "the setter goes both ways; {out}"
    );
}

/// The six region verbs plus `pathfinderFaces`, and the read that makes
/// the last of them usable. A face is addressed by an id, so shipping
/// the verb without `planarRegions` would have shipped a hole.
#[test]
fn the_pathfinder_region_verbs_rewrite_the_artwork() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const pair = (t, l) => [paged.insertOval(pid, [t, l, t + 100, l + 100]),
                        paged.insertOval(pid, [t + 50, l + 50, t + 150, l + 150])];
let [a, b] = pair(100, 100);
console.log('divide', paged.pathfinderDivide([a, b]));
let [c, d] = pair(100, 300);
console.log('trim', paged.pathfinderTrim([c, d]));
let [e, f] = pair(300, 100);
console.log('merge', paged.pathfinderMerge([e, f]));
let [g, h] = pair(300, 300);
console.log('crop', paged.pathfinderCrop([g, h]));
let [i, j] = pair(500, 100);
console.log('outline', paged.pathfinderOutline([i, j]));
let [k, l] = pair(500, 300);
console.log('minusBack', paged.pathfinderMinusBack([k, l]));

let [p, q] = pair(650, 100);
const regions = JSON.parse(paged.planarRegions([p, q]));
console.log('faces', regions.found, regions.faces.length, regions.inputCount);
console.log('keep', paged.pathfinderFaces([p, q], [regions.faces[0].id], 'keep'));

// user input never throws: an empty selection and a mode that is not a
// mode are both `false`, not an exception.
console.log('empty', paged.pathfinderDivide([]), 'badMode',
            paged.pathfinderFaces([p, q], ['0#0'], 'sideways'));"#,
    );
    for verb in [
        "divide true",
        "trim true",
        "merge true",
        "crop true",
        "outline true",
        "minusBack true",
    ] {
        assert!(out.contains(verb), "{verb} should apply; {out}");
    }
    assert!(
        out.contains("faces true 3 2"),
        "two overlapping ovals make three faces; {out}"
    );
    assert!(out.contains("keep true"), "{out}");
    assert!(out.contains("empty false badMode false"), "{out}");
}

/// Opacity masks and text on a path — two capabilities that were
/// panel-only. The attach case also pins the engine's "one story, one
/// flow" rule: a story still in a frame refuses the path.
#[test]
fn masks_and_text_on_a_path_apply_from_a_script() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const A = (x, y) => ({ anchor: [x, y], left: [x, y], right: [x, y] });

const art = paged.insertOval(pid, [100, 100, 200, 200]);
const mask = paged.insertOval(pid, [120, 120, 220, 220]);
console.log('mask', paged.applyOpacityMask(art, mask, { invert: true }));
console.log('release', paged.releaseOpacityMask(art));

const frame = paged.insertTextFrame(pid, [300, 100, 400, 300]);
const story = JSON.parse(paged.stories())[0].selfId;
paged.insertText(story, 0, 'Hello world');
const path = paged.insertPath(pid, [A(100, 500), A(300, 500)], true, false);
console.log('busy', paged.attachTextToPath(path, story));
console.log('freed', paged.deleteElement(frame));
console.log('attach', paged.attachTextToPath(path, story, { pathTypeAlignment: 'CenterPathType' }));
console.log('detach', paged.detachTextFromPath(path));
console.log('storySurvives', JSON.parse(paged.stories()).length);"#,
    );
    assert!(out.contains("mask true"), "{out}");
    assert!(out.contains("release true"), "{out}");
    assert!(
        out.contains("busy false"),
        "a story already flowing into a frame refuses a path; {out}"
    );
    assert!(out.contains("attach true"), "{out}");
    assert!(out.contains("detach true"), "{out}");
    assert!(
        out.contains("storySurvives 1"),
        "detach unlinks and does not delete the text; {out}"
    );
}

/// z-order, nesting and the two path-topology verbs.
#[test]
fn z_order_nesting_and_path_topology_apply_from_a_script() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
const A = (x, y) => ({ anchor: [x, y], left: [x, y], right: [x, y] });

const a = paged.insertOval(pid, [100, 100, 200, 200]);
paged.insertOval(pid, [150, 150, 250, 250]);
console.log('z', paged.reorderElement(a, 'front'), paged.reorderElement(a, 'back'),
            paged.reorderElement(a, 'forward'), paged.reorderElement(a, 'backward'),
            paged.reorderElement(a, { index: 1 }));
console.log('zBogus', paged.reorderElement(a, 'bringToFront'));

const box0 = paged.insertFrame(pid, [300, 300, 420, 420]);
const child = paged.insertOval(pid, [320, 320, 360, 360]);
console.log('nest', paged.pasteInto(box0, child), paged.releaseFrom(child));

const open0 = paged.insertPath(pid, [A(100, 600), A(200, 600), A(200, 650)], true, false);
console.log('close', paged.closePath(open0));
const p1 = paged.insertPath(pid, [A(300, 600), A(400, 600)], true, false);
const p2 = paged.insertPath(pid, [A(400, 600), A(500, 650)], true, false);
console.log('join', paged.joinPaths(p1, p2));"#,
    );
    assert!(out.contains("z true true true true true"), "{out}");
    assert!(
        out.contains("zBogus false"),
        "a target outside the ZOrderTarget vocabulary is false, not a throw; {out}"
    );
    assert!(out.contains("nest true true"), "{out}");
    assert!(out.contains("close true"), "{out}");
    assert!(out.contains("join true"), "{out}");
}

/// Anchored frames and hyperlinks — the two story-offset authoring ops
/// whose read side already existed while the write side did not.
///
/// Both assert where the result actually LANDS, which is not where the
/// first draft looked: an anchored frame is a child of the story's
/// paragraph, so it never appears in `paged.tree()` (spreads → pages →
/// frames) and does not move `characterCount`; and a hyperlink is read
/// back through `paged.collection("hyperlinks")`, while `paged.links()`
/// — which the fn's doc comment first pointed at — is the placed-asset
/// list and stays empty.
#[test]
fn anchored_frames_and_hyperlinks_are_authored_from_a_script() {
    let mut m = blank();
    let out = run(
        &mut m,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [72, 72, 400, 400]);
const story = JSON.parse(paged.stories())[0].selfId;
paged.insertText(story, 0, 'Read the manual for details');
console.log('link', paged.insertHyperlink(story, 9, 15, 'https://paged.media'));
const links = JSON.parse(paged.collection('hyperlinks'));
console.log('hyperlinks', links.length, links[0] && links[0].destination.indexOf('URLDestination') > 0);
console.log('placedAssets', JSON.parse(paged.links()).length);
console.log('anchor', paged.insertAnchoredFrame(story, 4, 40, 20));
console.log('chars', JSON.parse(paged.stories())[0].characterCount);"#,
    );
    assert!(out.contains("link true"), "{out}");
    assert!(
        out.contains("hyperlinks 1 true"),
        "the link registers a Hyperlink resolving to a URL destination; {out}"
    );
    assert!(
        out.contains("placedAssets 0"),
        "paged.links() is the placed-asset list and must stay empty; {out}"
    );
    assert!(out.contains("anchor true"), "{out}");
    assert!(
        out.contains("chars 27"),
        "the anchor marker is not a story character; {out}"
    );

    // The frame itself lands INSIDE the story, which is why no script
    // read shows it — assert on the model rather than believe the bool.
    let stories = serde_json::to_string(&m.scene().stories).expect("stories serialize");
    assert!(
        stories.contains("anchored"),
        "the anchored frame should be a child of the story's paragraph"
    );
}
