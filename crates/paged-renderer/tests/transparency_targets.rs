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

//! R-10 / R-11 — how the display list composites a page item's
//! transparency targets, a group's isolate and knockout, and a shadow
//! under the document's light, as InDesign 20.0.1 does (the
//! `transparency-targets` and `effects-attrs` fixtures).

use std::io::Write;

use paged_compose::{BlendMode, DisplayCommand, DisplayList};
use paged_renderer::{pipeline, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn inter() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

/// A one-page package holding `items`, a story `a` and `preferences`.
fn package(items: &str, preferences: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut file = |name: &str, body: &str, opts| {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    file(
        "mimetype",
        "application/vnd.adobe.indesign-idml-package",
        stored,
    );
    file(
        "designmap.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Graphic src="Resources/Graphic.xml"/>
  <idPkg:Preferences src="Resources/Preferences.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
</Document>"#,
        deflated,
    );
    file(
        "Resources/Graphic.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Black" Model="Process" Space="CMYK" ColorValue="0 0 0 100" Name="Black"/>
</idPkg:Graphic>"#,
        deflated,
    );
    file(
        "Resources/Preferences.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Preferences xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">{preferences}</idPkg:Preferences>"#
        ),
        deflated,
    );
    file(
        "Spreads/Spread_sp1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 500"/>
    {items}
  </Spread>
</idPkg:Spread>"#
        ),
        deflated,
    );
    file(
        "Stories/Story_a.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24" FillColor="Color/Black"><Content>Ag</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        deflated,
    );
    zip.finish().unwrap().into_inner()
}

fn page_list(bytes: &[u8]) -> DisplayList {
    let font = inter();
    let doc = idml_import::import_idml_doc(bytes).expect("open IDML");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    let mut built = pipeline::build_document(&doc, &opts).expect("build");
    built.pages.swap_remove(0).list
}

/// `(begin, end, blend, opacity, isolated, knockout)` for every
/// `BeginBlendGroup`, `end` being its matching `EndBlendGroup`.
fn groups(list: &DisplayList) -> Vec<(usize, usize, BlendMode, f32, bool, bool)> {
    let mut open: Vec<(usize, BlendMode, f32, bool, bool)> = Vec::new();
    let mut out = Vec::new();
    for (i, c) in list.commands.iter().enumerate() {
        match c {
            DisplayCommand::BeginBlendGroup {
                blend_mode,
                opacity,
                isolated,
                knockout,
                ..
            } => open.push((i, *blend_mode, *opacity, *isolated, *knockout)),
            DisplayCommand::EndBlendGroup(_) => {
                if let Some((b, m, o, iso, ko)) = open.pop() {
                    out.push((b, i, m, o, iso, ko));
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|g| g.0);
    out
}

const RECT: &str = r#"GeometricBounds="100 100 200 300" FillColor="Color/Black" StrokeColor="Color/Black" StrokeWeight="8""#;

/// InDesign composites a text frame's text inside the frame's object
/// group (case 1: the "Ag" over a fill at 50 % shows light green, not
/// darkened by a second fade). One group holds the fill and every glyph.
#[test]
fn a_text_frame_and_its_text_fade_as_one_object() {
    let list = page_list(&package(
        &format!(
            r#"<TextFrame Self="tf" ParentStory="a" {RECT}><Properties/><TransparencySetting><BlendingSetting Opacity="50"/></TransparencySetting></TextFrame>"#
        ),
        "",
    ));
    let halves: Vec<_> = groups(&list)
        .into_iter()
        .filter(|g| (g.3 - 0.5).abs() < 1e-3)
        .collect();
    assert_eq!(
        halves.len(),
        1,
        "one object group, not one for the body and one for the text"
    );
    let (begin, end, ..) = halves[0];
    let runs = list.glyph_runs.as_ref().expect("glyph runs");
    assert!(!runs.entries.is_empty());
    for e in &runs.entries {
        let i = e.command_index as usize;
        assert!(
            begin < i && i < end,
            "glyph {i} outside the group {begin}..{end}"
        );
    }
}

/// Each target composites in its own group, nested in the object's: the
/// fill at 40 % Multiply, the stroke at 30 % Normal, the text at 60 %
/// Screen (cases 4–12).
#[test]
fn every_target_has_its_own_group() {
    let list = page_list(&package(
        &format!(
            r#"<TextFrame Self="tf" ParentStory="a" {RECT}><Properties/><StrokeTransparencySetting><BlendingSetting Opacity="30"/></StrokeTransparencySetting><FillTransparencySetting><BlendingSetting BlendMode="Multiply" Opacity="40"/></FillTransparencySetting><ContentTransparencySetting><BlendingSetting BlendMode="Screen" Opacity="60"/></ContentTransparencySetting></TextFrame>"#
        ),
        "",
    ));
    let gs = groups(&list);
    let find = |m: BlendMode, o: f32| {
        gs.iter()
            .find(|g| g.2 == m && (g.3 - o).abs() < 1e-3)
            .copied()
            .unwrap_or_else(|| panic!("no {m:?} {o} group in {gs:?}"))
    };
    let fill = find(BlendMode::Multiply, 0.4);
    let stroke = find(BlendMode::Normal, 0.3);
    let text = find(BlendMode::Screen, 0.6);
    assert!(fill.1 < stroke.0, "the fill paints before the stroke");
    let paints = |g: (usize, usize, BlendMode, f32, bool, bool)| {
        list.commands[g.0 + 1..g.1]
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    DisplayCommand::FillPath { .. } | DisplayCommand::StrokePath { .. }
                )
            })
            .count()
    };
    assert!(paints(fill) >= 1 && paints(stroke) >= 1, "{gs:?}");
    let runs = list.glyph_runs.as_ref().expect("glyph runs");
    for e in &runs.entries {
        let i = e.command_index as usize;
        assert!(
            text.0 < i && i < text.1,
            "glyph {i} outside the text's group"
        );
    }
}

/// A group's flags reach its bracket, and a knockout group brackets
/// each member so every member is one element (cases 41–48).
#[test]
fn a_groups_isolate_and_knockout_reach_its_bracket() {
    let member = |id: &str, x: u32| {
        format!(
            r#"<Rectangle Self="{id}" GeometricBounds="100 {x} 160 {}" FillColor="Color/Black"><Properties/></Rectangle>"#,
            x + 60
        )
    };
    let list = page_list(&package(
        &format!(
            r#"<Group Self="g" ItemTransform="1 0 0 1 0 0"><TransparencySetting><BlendingSetting IsolateBlending="true" KnockoutGroup="true"/></TransparencySetting>{}{}</Group>"#,
            member("m1", 100),
            member("m2", 130)
        ),
        "",
    ));
    let gs = groups(&list);
    let outer = gs
        .iter()
        .find(|g| g.4 && g.5)
        .unwrap_or_else(|| panic!("no isolated knockout group in {gs:?}"));
    let inner: Vec<_> = gs
        .iter()
        .filter(|g| g.0 > outer.0 && g.1 < outer.1)
        .collect();
    assert_eq!(inner.len(), 2, "each member is its own element: {gs:?}");
}

/// Under the document's light (45° here) a 7 / 7 shadow keeps its
/// distance and falls down and to the LEFT (`effects-attrs` case 14).
#[test]
fn a_shadow_under_the_global_light_turns_to_it() {
    let shadow = |global: bool| {
        let list = page_list(&package(
            &format!(
                r#"<Rectangle Self="r" GeometricBounds="100 100 200 300" FillColor="Color/Black"><Properties/><TransparencySetting><DropShadowSetting Mode="Drop" XOffset="7" YOffset="7" Size="4" Opacity="75" UseGlobalLight="{global}"/></TransparencySetting></Rectangle>"#
            ),
            r#"<TransparencyPreference GlobalLightAngle="45" GlobalLightAltitude="20"/>"#,
        ));
        list.commands
            .iter()
            .find_map(|c| match c {
                DisplayCommand::DropShadow { shadow, .. } => {
                    Some((shadow.offset_x, shadow.offset_y))
                }
                _ => None,
            })
            .expect("a shadow")
    };
    assert_eq!(shadow(false), (7.0, 7.0));
    let (x, y) = shadow(true);
    assert!(
        (x + 7.0).abs() < 1e-3 && (y - 7.0).abs() < 1e-3,
        "({x}, {y})"
    );
}
