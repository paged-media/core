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

//! A script can read a story's text, and select text frames by the
//! paragraph styles their story applies.
//!
//! Before: `paged.inspect("story:…")` and `paged.get(<frame>, "text")`
//! answered `null` — no door read text at all — and
//! `textFrame[style="ParagraphStyle/Price"]` matched nothing, because
//! `[style=…]` only ever read a page item's OBJECT style. Now:
//! - `paged.get(<story | textFrame | storyRange>, "text")` is the text:
//!   a story's paragraphs joined by `\n` (the separator `insertText`
//!   splits on); a text frame answers its story's (the whole thread's);
//!   a range answers its characters.
//! - `paged.inspect("story:<id>")` is the story's snapshot, `text` first.
//! - `[style=…]` on a text frame or a story also matches the paragraph
//!   and character styles the story applies, by `Self` id or name.

use std::io::{Cursor, Write};

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

const PKG: &str = r#"xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging""#;

fn frame(id: &str, story: &str, y: f32) -> String {
    let pt = |x: f32, y: f32| {
        format!(
            r#"<PathPointType Anchor="{x} {y}" LeftDirection="{x} {y}" RightDirection="{x} {y}"/>"#
        )
    };
    format!(
        r#"<TextFrame Self="{id}" ParentStory="{story}" ItemTransform="1 0 0 1 0 0"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>{}{}{}{}</PathPointArray></GeometryPathType></PathGeometry></Properties></TextFrame>"#,
        pt(36.0, y),
        pt(36.0, y + 100.0),
        pt(300.0, y + 100.0),
        pt(300.0, y)
    )
}

fn story(id: &str, paragraphs: &[(&str, &str)]) -> String {
    let body: String = paragraphs
        .iter()
        .enumerate()
        .map(|(i, (style, text))| {
            let br = if i + 1 < paragraphs.len() { "<Br/>" } else { "" };
            format!(
                r#"<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/{style}"><CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"><Content>{text}</Content>{br}</CharacterStyleRange></ParagraphStyleRange>"#
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><idPkg:Story {PKG} DOMVersion="20.0"><Story Self="{id}">{body}</Story></idPkg:Story>"#
    )
}

/// A price list: one frame holding a name and a price, one a name only,
/// and a rectangle in object style "Box".
fn package() -> Vec<u8> {
    let designmap = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d" {PKG}>
<idPkg:Styles src="Resources/Styles.xml"/>
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
<idPkg:Story src="Stories/Story_st1.xml"/>
<idPkg:Story src="Stories/Story_st2.xml"/>
</Document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><idPkg:Styles {PKG} DOMVersion="20.0">
<RootCharacterStyleGroup><CharacterStyle Self="CharacterStyle/$ID/[No character style]" Name="$ID/[No character style]"/></RootCharacterStyleGroup>
<RootParagraphStyleGroup><ParagraphStyle Self="ParagraphStyle/$ID/[No paragraph style]" Name="$ID/[No paragraph style]"/><ParagraphStyle Self="ParagraphStyle/Name" Name="Name"/><ParagraphStyle Self="ParagraphStyle/Price" Name="Price"/></RootParagraphStyleGroup>
<RootObjectStyleGroup><ObjectStyle Self="ObjectStyle/$ID/[None]" Name="$ID/[None]"/><ObjectStyle Self="ObjectStyle/Box" Name="Box"/></RootObjectStyleGroup>
</idPkg:Styles>"#
    );
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><idPkg:Spread {PKG} DOMVersion="20.0">
<Spread Self="s1" PageCount="1" BindingLocation="0" ItemTransform="1 0 0 1 0 0">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
{}{}
<Rectangle Self="r1" AppliedObjectStyle="ObjectStyle/Box" GeometricBounds="400 36 500 300" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        frame("tf1", "st1", 36.0),
        frame("tf2", "st2", 200.0)
    );
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        for (name, body) in [
            ("designmap.xml", designmap),
            ("Resources/Styles.xml", styles),
            ("Spreads/Spread_s1.xml", spread),
            (
                "Stories/Story_st1.xml",
                story("st1", &[("Name", "Widget"), ("Price", "12.00")]),
            ),
            ("Stories/Story_st2.xml", story("st2", &[("Name", "Gadget")])),
        ] {
            zip.start_file(name, deflated).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn run(src: &str) -> Vec<String> {
    let mut model =
        CanvasModel::load("price-list", &package(), CanvasOptions::default()).expect("load");
    let r = execute_script(&mut model, src);
    assert!(r.error.is_none(), "script error: {:?}", r.error);
    r.output
}

#[test]
fn a_script_reads_a_story_s_text_through_get() {
    let out = run(r#"
        const text = (a) => { const v = paged.get(a, "text"); return v === null ? null : JSON.parse(v).value; };
        console.log(JSON.stringify(text("story:st1")));
        console.log(JSON.stringify(text("textFrame:tf1")));
        console.log(JSON.stringify(text("textFrame:tf2")));
        console.log(JSON.stringify(text("storyRange:st1@0..6")));
        console.log(JSON.stringify(text("storyRange:st1@6..11")));
    "#);
    assert_eq!(
        out,
        [
            r#"[log] "Widget\n12.00""#,
            r#"[log] "Widget\n12.00""#,
            r#"[log] "Gadget""#,
            r#"[log] "Widget""#,
            r#"[log] "12.00""#,
        ]
    );
}

#[test]
fn inspect_answers_a_story() {
    let out = run(r#"
        const s = paged.inspect("story:st1");
        console.log(s === null ? "null" : "snapshot");
        const snap = JSON.parse(s);
        console.log(snap.kind, snap.id.kind, snap.id.id);
        const text = snap.entries.find(e => e.path === "text");
        console.log(JSON.stringify(text.value));
        console.log(snap.entries.some(e => e.path === "flowGrow"));
        console.log(paged.inspect("story:nope"));
    "#);
    assert_eq!(
        out,
        [
            "[log] snapshot",
            "[log] Story story st1",
            r#"[log] {"type":"text","value":"Widget\n12.00"}"#,
            "[log] true",
            "[log] null",
        ]
    );
}

#[test]
fn a_style_selector_on_frames_matches_their_story_s_paragraph_styles() {
    let out = run(r#"
        console.log(paged.query('textFrame[style="ParagraphStyle/Price"]'));
        console.log(paged.query('textFrame[style=Price]'));
        console.log(paged.query('textFrame[style=Name]'));
        console.log(paged.query('textFrame[style!=Price]'));
        console.log(paged.query('story[style=Price]'));
        console.log(paged.query('frame[style=Box]'));
        console.log(paged.query('rectangle[style="ObjectStyle/Box"]'));
    "#);
    assert_eq!(
        out,
        [
            r#"[log] ["textFrame:tf1"]"#,
            r#"[log] ["textFrame:tf1"]"#,
            r#"[log] ["textFrame:tf1","textFrame:tf2"]"#,
            r#"[log] ["textFrame:tf2"]"#,
            r#"[log] ["story:st1"]"#,
            r#"[log] ["rectangle:r1"]"#,
            r#"[log] ["rectangle:r1"]"#,
        ]
    );
}
