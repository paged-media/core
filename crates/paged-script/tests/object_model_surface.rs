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

//! ADR 132 through the script door: `paged.set` / `paged.get` on target
//! objects (`{ kind: "doc" }`, `{ kind: "layer", layerId }`, a style
//! definition, a page) reach `Mutation::Set`, and the value a script holds
//! is coerced by the path's schema — a unit string on a length, a closed
//! enum refusing a stranger, a range holding.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

fn run(model: &mut CanvasModel, src: &str) -> Vec<String> {
    let r = execute_script(model, src);
    assert!(
        r.error.is_none(),
        "script failed: {:?}\n{:?}",
        r.error,
        r.output
    );
    r.output
}

fn blank() -> CanvasModel {
    CanvasModel::new_blank("om", 612.0, 792.0, CanvasOptions::default()).expect("new_blank")
}

#[test]
fn target_objects_set_and_read_through_the_schema() {
    let mut model = blank();
    let out = run(
        &mut model,
        r#"
const ok = (label, v) => console.log(label, v);
const doc = { kind: 'doc' };
ok('bleed', paged.set(doc, 'documentBleedTop', '3mm'));
ok('bleedRead', paged.get(doc, 'documentBleedTop'));
ok('defaultsWeight', paged.set(doc, 'defaultStrokeWeight', 2));

const pid = JSON.parse(paged.pages())[0].selfId;
const page = { kind: 'page', pageId: pid };
ok('margin', paged.set(page, 'pageMarginTop', 36));
ok('columns', paged.set(page, 'pageColumnCount', 3));
ok('columnsRead', paged.get(page, 'pageColumnCount'));
ok('badColumns', paged.set(page, 'pageColumnCount', 0));

paged.layerInsert(0, 'Art');
const layer = JSON.parse(paged.layers()).find(l => l.name === 'Art');
const L = { kind: 'layer', layerId: layer.selfId };
ok('layerVisible', paged.set(L, 'layerVisible', false));
ok('layerVisibleRead', paged.get(L, 'layerVisible'));
ok('layerName', paged.set(L, 'layerName', 'Artwork'));

const os = paged.createObjectStyle({ name: 'Card' });
const style = { kind: 'style', family: 'object', styleId: os };
ok('styleFill', paged.set(style, 'frameFillColor', 'Color/Black'));
ok('styleRadius', paged.set(style, 'objectStyleCornerRadius', '2mm'));
ok('styleRadiusRead', paged.get(style, 'objectStyleCornerRadius'));
ok('styleName', paged.set(style, 'name', 'Card 2'));

const sw = paged.createSwatch({ space: 'CMYK', value: [0, 100, 100, 0], name: 'Red' });
const S = { kind: 'swatch', swatchId: sw };
ok('tint', paged.set(S, 'tint', 40));
ok('tintRead', paged.get(S, 'tint'));
ok('badTint', paged.set(S, 'tint', 140));
ok('value', paged.set(S, 'value', [10, 20, 30, 40]));

const tf = paged.insertTextFrame(pid, [72, 72, 300, 300]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Hello');
const range = 'storyRange:' + sid + '@0..5';
ok('enumOk', paged.set(range, 'paragraphJustification', 'CenterAlign'));
ok('enumBad', paged.set(range, 'paragraphJustification', 'Middle'));
ok('fontSizeUnit', paged.set(range, 'characterFontSize', '0.25in'));
ok('fontSizeRead', paged.get(range, 'characterFontSize'));
ok('labelSet', paged.set(tf, 'labels[x-paged:media.paged.probe]', '{"v":1,"data":{"a":1}}'));
ok('labelRead', paged.get(tf, 'labels[x-paged:media.paged.probe]'));
ok('unknown', paged.set(doc, 'noSuchPath', 1));
"#,
    );
    let line = |label: &str| {
        out.iter()
            .find_map(|l| {
                l.strip_prefix(&format!("[log] {label} "))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| panic!("no {label} in {out:?}"))
    };
    for label in [
        "bleed",
        "defaultsWeight",
        "margin",
        "columns",
        "layerVisible",
        "layerName",
        "styleFill",
        "styleRadius",
        "styleName",
        "tint",
        "value",
        "enumOk",
        "fontSizeUnit",
        "labelSet",
    ] {
        assert_eq!(line(label), "true", "{label}");
    }
    for label in ["badColumns", "badTint", "enumBad", "unknown"] {
        assert_eq!(line(label), "false", "{label} must be refused");
    }
    assert!(
        line("bleedRead").contains("8.50393"),
        "{}",
        line("bleedRead")
    );
    assert_eq!(line("columnsRead"), r#"{"type":"length","value":3.0}"#);
    assert_eq!(line("layerVisibleRead"), r#"{"type":"bool","value":false}"#);
    assert!(
        line("styleRadiusRead").contains("5.66929"),
        "{}",
        line("styleRadiusRead")
    );
    assert_eq!(line("tintRead"), r#"{"type":"length","value":40.0}"#);
    assert!(
        line("fontSizeRead").contains("18"),
        "{}",
        line("fontSizeRead")
    );
    assert!(line("labelRead").contains("media.paged") || line("labelRead").contains("\\\"a\\\""));
}

#[test]
fn a_set_is_one_undo_step_from_a_script() {
    let mut model = blank();
    let out = run(
        &mut model,
        r#"
const sw = paged.createSwatch({ space: 'CMYK', value: [0, 0, 0, 100], name: 'Ink' });
const S = { kind: 'swatch', swatchId: sw };
paged.set(S, 'tint', 30);
paged.set(S, 'name', 'Ink 2');
paged.undo();
console.log('name', paged.get(S, 'name'));
console.log('tint', paged.get(S, 'tint'));
"#,
    );
    assert!(
        out.iter()
            .any(|l| l == r#"[log] name {"type":"text","value":"Ink"}"#),
        "{out:?}"
    );
    assert!(
        out.iter()
            .any(|l| l == r#"[log] tint {"type":"length","value":30.0}"#),
        "{out:?}"
    );
}

/// v71 polish — `null` clears a nullable row through the script door, as a
/// bare `null` and as a typed `{ type, value: null }`, for every
/// text-carried row kind: text (a label), enum (a paragraph's
/// justification), ref (a page's applied master). A required row still
/// refuses it.
#[test]
fn null_clears_a_nullable_text_enum_and_ref_row() {
    let mut model = blank();
    let out = run(
        &mut model,
        r#"
const ok = (label, v) => console.log(label, v);
const pid = JSON.parse(paged.pages())[0].selfId;
const page = 'page:' + pid;
const tf = paged.insertTextFrame(pid, [72, 72, 300, 300]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Hello');
const range = 'storyRange:' + sid + '@0..5';
const label = 'labels[x-paged:media.paged.probe]';
paged.set(tf, label, '{"v":1,"data":{"a":1}}');
ok('text', paged.set(tf, label, null));
ok('textRead', paged.get(tf, label));
paged.set(tf, label, '{"v":1,"data":{"a":1}}');
ok('textTyped', paged.set(tf, label, { type: 'text', value: null }));
paged.set(range, 'paragraphJustification', 'CenterAlign');
ok('enum', paged.set(range, 'paragraphJustification', null));
paged.set(range, 'paragraphJustification', 'CenterAlign');
ok('enumTyped', paged.set(range, 'paragraphJustification', { type: 'enum', value: null }));
ok('ref', paged.set(page, 'appliedMaster', null));
ok('refRead', paged.get(page, 'appliedMaster'));
ok('refTyped', paged.set(page, 'appliedMaster', { type: 'ref', value: null }));
ok('required', paged.set('doc', 'activeConditionSet', null));
"#,
    );
    let line = |label: &str| {
        out.iter()
            .find_map(|l| {
                l.strip_prefix(&format!("[log] {label} "))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| panic!("no {label} in {out:?}"))
    };
    for label in ["text", "textTyped", "enum", "enumTyped", "ref", "refTyped"] {
        assert_eq!(line(label), "true", "{label}: null must clear");
    }
    assert_eq!(line("required"), "false", "a required row refuses null");
    for label in ["textRead", "refRead"] {
        let read = line(label);
        assert!(
            read == "null" || read.contains(r#""value":"""#) || read.contains(r#""value":null"#),
            "{label}: {read}"
        );
    }
}
