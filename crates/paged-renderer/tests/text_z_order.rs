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

//! W2 — a story's glyphs paint at their text frame's z position.
//!
//! IDML has no z attribute: stacking order IS document order inside
//! `<Spread>`. A panel written after a text frame therefore covers it,
//! and InDesign hides the text (measured on the annual's page 59, where
//! a display headline under a slate panel is invisible in InDesign and
//! visible on our canvas). The emitter cannot run inside the page walk
//! — it needs the whole frame chain, which spans pages — so the glyphs
//! are relocated into the frame's recorded slot afterwards.

use std::io::Write;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn inter() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

/// A page with a text frame and a filled panel over it. `panel_first`
/// writes the panel BEFORE the frame (so it is behind); `layer` gives
/// the panel an `ItemLayer` to exercise the layer-z sort.
fn build(panel_first: bool, panel_attrs: &str, frame_attrs: &str, layers: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  {layers}
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
</Document>"#
        )
        .as_bytes(),
    )
    .unwrap();
    let frame = format!(
        r#"<TextFrame Self="frameA" ParentStory="a" GeometricBounds="100 60 160 400"{frame_attrs}><Properties/></TextFrame>"#
    );
    let panel = format!(
        r#"<Rectangle Self="panel" GeometricBounds="90 50 170 410" FillColor="Color/Black"{panel_attrs}><Properties/></Rectangle>"#
    );
    let items = if panel_first {
        format!("{panel}\n    {frame}")
    } else {
        format!("{frame}\n    {panel}")
    };
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 500"/>
    {items}
  </Spread>
</idPkg:Spread>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zip.start_file("Stories/Story_a.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24"><Content>Umbra</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

/// `(lowest glyph command index, highest glyph command index, the
/// panel's FillPath index)`.
fn indices(bytes: &[u8], text_at_frame_z: bool) -> (usize, usize, usize) {
    let font = inter();
    let doc = idml_import::import_idml_doc(bytes).expect("open IDML");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        text_at_frame_z,
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let page = &built.pages[0];
    let runs = page.list.glyph_runs.as_ref().expect("glyph runs collected");
    assert!(!runs.entries.is_empty(), "the story produced no glyphs");
    let glyphs: std::collections::BTreeSet<usize> = runs
        .entries
        .iter()
        .map(|e| e.command_index as usize)
        .collect();
    // Every glyph entry must still address a paint command — the remap
    // is what keeps the PDF exporter's `glyph_by_cmd` honest.
    for &i in &glyphs {
        assert!(
            matches!(
                page.list.commands.get(i),
                Some(DisplayCommand::FillPath { .. } | DisplayCommand::StrokePath { .. })
            ),
            "glyph entry {i} does not address a paint command: {:?}",
            page.list.commands.get(i)
        );
    }
    // The panel is the only filled box: the one FillPath that is not a glyph.
    let panel = page
        .list
        .commands
        .iter()
        .enumerate()
        .filter(|(i, c)| matches!(c, DisplayCommand::FillPath { .. }) && !glyphs.contains(i))
        .map(|(i, _)| i)
        .next()
        .expect("the panel painted a fill");
    (
        *glyphs.iter().next().expect("a glyph"),
        *glyphs.iter().next_back().expect("a glyph"),
        panel,
    )
}

#[test]
fn a_panel_above_a_text_frame_covers_its_text() {
    let bytes = build(false, "", "", "");
    let (lo, hi, panel) = indices(&bytes, true);
    assert!(
        hi < panel,
        "every glyph must paint before the panel that covers it: glyphs {lo}..={hi}, panel {panel}"
    );
}

#[test]
fn the_legacy_flag_keeps_text_on_top() {
    // The pre-W2 stream, pinned: with the flag off the glyphs are
    // appended after the whole page walk, so they paint over the panel.
    let bytes = build(false, "", "", "");
    let (lo, _, panel) = indices(&bytes, false);
    assert!(
        lo > panel,
        "with text_at_frame_z off the glyphs stay on top: glyphs from {lo}, panel {panel}"
    );
}

#[test]
fn a_panel_below_a_text_frame_still_leaves_the_text_on_top() {
    // The other direction: the panel is written FIRST, so the text is
    // above it in z and must stay above it in the command stream.
    let bytes = build(true, "", "", "");
    let (lo, _, panel) = indices(&bytes, true);
    assert!(
        lo > panel,
        "a panel behind the frame must paint first: glyphs from {lo}, panel {panel}"
    );
}

#[test]
fn an_upper_layer_lifts_the_panel_over_the_text() {
    // Layer z beats document order (Q-10): the panel is written first
    // but sits on the higher layer, so it covers the text.
    let layers = r#"<Layer Self="lo" Name="Back" Visible="true"/>
  <Layer Self="hi" Name="Front" Visible="true"/>"#;
    let bytes = build(true, r#" ItemLayer="hi""#, r#" ItemLayer="lo""#, layers);
    let (_, hi, panel) = indices(&bytes, true);
    assert!(
        hi < panel,
        "a panel on the upper layer covers the text whatever the XML order: glyphs ..={hi}, panel {panel}"
    );
}

#[test]
fn text_inside_a_transparent_group_composites_inside_the_group() {
    // A `<Group Opacity="50">` over the frame and the panel. Before W2
    // the glyphs were appended after the group's EndBlendGroup, so the
    // group's opacity never reached them; now they land inside the
    // bracket, which is where InDesign composites them.
    let bytes = {
        let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 500"/>
    <Group Self="grp">
      <Properties/>
      <TransparencySetting><BlendingSetting Opacity="50"/></TransparencySetting>
      <Rectangle Self="behind" GeometricBounds="90 50 170 410" FillColor="Color/Black"><Properties/></Rectangle>
      <TextFrame Self="frameA" ParentStory="a" GeometricBounds="100 60 160 400"><Properties/></TextFrame>
    </Group>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_a.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="24"><Content>Umbra</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap().into_inner()
    };
    let font = inter();
    let doc = idml_import::import_idml_doc(&bytes).expect("open IDML");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let page = &built.pages[0];
    let begin = page
        .list
        .commands
        .iter()
        .position(|c| matches!(c, DisplayCommand::BeginBlendGroup { .. }))
        .expect("the group brackets its members");
    let end = page
        .list
        .commands
        .iter()
        .rposition(|c| matches!(c, DisplayCommand::EndBlendGroup(_)))
        .expect("the group closes");
    let runs = page.list.glyph_runs.as_ref().expect("glyph runs");
    assert!(!runs.entries.is_empty(), "the story produced no glyphs");
    for e in &runs.entries {
        let i = e.command_index as usize;
        assert!(
            i > begin && i < end,
            "glyph command {i} must sit inside the group bracket {begin}..{end}"
        );
    }
}
