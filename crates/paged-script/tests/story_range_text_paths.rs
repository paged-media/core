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

//! The probe the catalog's own "no recorded reason" asked for.
//!
//! Twenty-seven character and paragraph properties sat in the
//! unadvertised list under one sentence: "it has a working apply arm on
//! `NodeId::StoryRange` and was never advertised — promote it once a
//! probe proves the wire address." They are the properties a typesetter
//! reaches for first — the font family, the case, the underline, the
//! indents, the drop cap, the tab stops, the hyphenation, the bullets —
//! and `paged.set` answered `false` for every one of them.
//!
//! Nothing was missing but the NAME. The apply layer routes them through
//! the same match arm as `characterFontSize` and `paragraphSpaceBefore`,
//! which were advertised all along, and the Boa bridge's
//! `js_value_to_wire` already lists each of them by variant. Only
//! `lookup_path` did not know the string, so the call never got as far
//! as the code that would have worked.
//!
//! This is the proof, through the real door: every promoted path is set
//! with `paged.set` on a `storyRange:` address and read back with
//! `paged.get`.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

fn seeded() -> CanvasModel {
    let mut model = CanvasModel::new_blank("story-range", 612.0, 792.0, CanvasOptions::default())
        .expect("new_blank");
    let r = execute_script(
        &mut model,
        r#"const pid = JSON.parse(paged.pages())[0].selfId;
paged.insertTextFrame(pid, [72, 72, 400, 400]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Type on a page, and read it back again.');
console.log('story', sid);"#,
    );
    assert!(r.error.is_none(), "seed failed: {:?}", r.error);
    model
}

/// Set every promoted path, then read every one back. One script, so a
/// failure names the property rather than the harness.
#[test]
fn every_promoted_story_range_path_sets_and_reads_back() {
    let mut model = seeded();
    let r = execute_script(
        &mut model,
        r#"const sid = JSON.parse(paged.stories())[0].selfId;
const range = 'storyRange:' + sid + '@0..4';

// [name, value to write] — the value shapes the apply layer expects:
// a string for the text/enum paths, a number for the lengths, a
// boolean for the toggles, and the tagged `{type, value}` form for the
// two whole-struct paths.
const CASES = [
  ['characterFontFamily', 'Inter'],
  ['characterFontStyle', 'Bold'],
  ['characterKerningMethod', 'Optical'],
  ['characterCase', 'SmallCaps'],
  ['characterPosition', 'Superscript'],
  ['characterLanguage', 'en_GB'],
  ['characterOtfFeatures', 'liga'],
  ['characterBaselineShift', 2],
  ['characterHorizontalScale', 110],
  ['characterVerticalScale', 90],
  ['characterSkew', 12],
  ['characterUnderline', true],
  ['characterStrikethru', true],
  ['characterLigatures', false],
  ['paragraphLeftIndent', 18],
  ['paragraphRightIndent', 6],
  ['paragraphDropCapCharacters', 1],
  ['paragraphDropCapLines', 3],
  ['paragraphKeepWithNext', 2],
  ['paragraphHyphenation', false],
  ['paragraphKeepLinesTogether', true],
  ['paragraphListType', 'BulletList'],
  // A CHARACTER, not a code point: the apply layer takes the first
  // char of the string, so '8226' would store '8'.
  ['paragraphBulletCharacter', '\u2022'],
  ['paragraphNumberingFormat', '1, 2, 3, 4...'],
  ['paragraphRuleAbove', { type: 'paragraphRule', value: { on: true, weight: 1 } }],
  ['paragraphRuleBelow', { type: 'paragraphRule', value: { on: true, weight: 2 } }],
  ['paragraphTabStops', { type: 'tabStops', value: [{ position: 36, alignment: 'Left' }] }],
];

const rejected = [];
for (const [name, value] of CASES) {
  if (paged.set(range, name, value) !== true) rejected.push(name);
}
console.log('rejected', rejected.length, rejected.join(','));

// The read side, and not merely "something came back": `paged.set`
// returning true is the call succeeding, not the value landing. Every
// scalar is compared to what was written.
const wrong = [];
for (const [name, wrote] of CASES) {
  const raw = paged.get(range, name);
  if (raw === null || raw === undefined) { wrong.push(name + ':null'); continue; }
  const back = JSON.parse(raw);
  if (typeof wrote === 'object') continue;      // struct paths, checked below
  if (back.value !== wrote) wrong.push(name + ':' + JSON.stringify(back.value) + '!=' + JSON.stringify(wrote));
}
console.log('wrong', wrong.length, wrong.join(' '));

// The two whole-struct paths, by a field of the struct.
const tabs = JSON.parse(paged.get(range, 'paragraphTabStops'));
console.log('tabPosition', tabs.value.length === 1 ? tabs.value[0].position : 'none');"#,
    );
    let out = r.output.join("\n");
    assert!(r.error.is_none(), "script errored: {:?}\n{out}", r.error);
    assert!(
        out.contains("rejected 0 "),
        "paged.set refused a promoted path; {out}"
    );
    assert!(
        out.contains("wrong 0 "),
        "a promoted path accepted a write and did not keep it — `paged.set` \
         returning true is the call succeeding, not the value landing; {out}"
    );
    assert!(
        out.contains("tabPosition 36"),
        "the tab list should carry the stop it was given; {out}"
    );
}

/// The catalog is what makes these reachable, so assert the roster
/// itself — a path that quietly leaves `settablePaths` takes
/// `paged.set` with it, and the script above would still pass if the
/// name resolved through some other route.
#[test]
fn the_promoted_paths_are_in_the_published_catalog() {
    let catalog = paged_script::api_catalog();
    for name in [
        "characterFontFamily",
        "characterCase",
        "characterUnderline",
        "characterLanguage",
        "paragraphLeftIndent",
        "paragraphTabStops",
        "paragraphHyphenation",
        "paragraphBulletCharacter",
        // The seventh frame effect, whose six siblings were advertised
        // and which was not, for no recorded reason.
        "frameGradientFeather",
    ] {
        assert!(
            catalog.settable_paths.contains(&name),
            "{name} is applied, bridged and reachable — and not advertised"
        );
    }
    assert_eq!(
        catalog.settable_paths.len(),
        202,
        "the advertised roster changed; move the count with the decision"
    );
}
