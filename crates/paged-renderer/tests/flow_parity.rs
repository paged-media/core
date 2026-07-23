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

//! Migration slice **S3b (pre-flight)** — parity harness.
//!
//! Proves the content-agnostic flow protocol (`paged_flow::run_flow` over a
//! `TextFlow`) **reproduces the live `StoryEmitter`'s per-frame line
//! assignment** on a real threaded IDML document. This is the equivalence
//! evidence that de-risks the eventual render-path flip (S3b proper), which
//! must additionally pass the fidelity gate. Here the render path is
//! untouched — this only *measures* that the protocol models the emitter.
//!
//! The fixture is deliberately in the regime where `flow_chain` geometry
//! equals the emitter's overflow reference: **no text insets** (so content-box
//! height == full bounds height, the emitter's `frame_height_64`) and **no
//! footnotes** (so no per-frame reservation). The leading and first-baseline
//! are read back from the emitter's own output, so the protocol runs on the
//! emitter's real metrics rather than assumed ones.

use std::io::Write;
use std::path::PathBuf;

use paged_flow::{run_flow, Overset};
use paged_renderer::{pipeline, BytesResolver, LineLayout, PipelineOptions, TextFlow};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Two frames threaded into one story (`frameA` -> `frameB`), no insets,
/// both anchored at the page top (content-top = 0 in page-local pt) so a
/// line's `baseline_y_pt` is directly its region-relative baseline. Heights
/// are chosen so the story fits (no overset) with the split falling well
/// clear of any frame edge.
fn build_threaded_idml(text: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
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
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();

    // frameA (head) 45pt tall -> frameB 200pt tall. GeometricBounds is
    // `top left bottom right`; both top-anchored (top = 0), side by side.
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 700"/>
    <TextFrame Self="frameA" ParentStory="u10" NextTextFrame="frameB" GeometricBounds="0 0 45 250" StrokeWeight="0"/>
    <TextFrame Self="frameB" ParentStory="u10" GeometricBounds="0 300 200 550" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>{text}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
    );
    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(story.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

// ~7 lines at 250pt / 12pt — overflows frameA (~3 lines) into frameB, fits.
const TEXT: &str = "The quick brown fox jumps over the lazy dog. \
Sphinx of black quartz, judge my vow. Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump.";

#[test]
fn protocol_reproduces_emitter_frame_assignment() {
    let bytes = build_threaded_idml(TEXT);
    let doc = idml_import::import_idml_doc(&bytes).unwrap();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();

    // The fixture must fit (no dropped content) so `story_layout` holds
    // *every* line — the protocol's total must reconcile against it.
    assert!(
        built.diagnostics.overset_story_ids().is_empty(),
        "fixture must fit within the chain (no overset)"
    );

    // The emitter's ground truth: body lines in document order, tagged with
    // the frame they landed in.
    let layout = built.story_layout("u10");
    assert!(layout.len() >= 4, "need enough lines to span both frames");

    // The composition's region-chain (S1). Region ids are the frame Self ids,
    // in chain order.
    let chain = doc.flow_chain("u10");
    assert_eq!(chain.regions.len(), 2);

    // Per-region line counts the emitter produced.
    let emitter_counts: Vec<usize> = chain
        .regions
        .iter()
        .map(|r| {
            layout
                .iter()
                .filter(|l| l.frame_id.as_deref() == Some(r.id.as_str()))
                .count()
        })
        .collect();
    assert!(
        emitter_counts.iter().all(|&c| c > 0),
        "each frame should host some lines; got {emitter_counts:?}"
    );

    // Read the emitter's real leading + first-baseline back from frameA's
    // lines, so the protocol runs on the emitter's own metrics (frameA is
    // page-top-anchored, so baseline_y_pt is already region-relative).
    let mut a_lines: Vec<&&LineLayout> = layout
        .iter()
        .filter(|l| l.frame_id.as_deref() == Some("frameA"))
        .collect();
    a_lines.sort_by(|x, y| x.baseline_y_pt.partial_cmp(&y.baseline_y_pt).unwrap());
    assert!(
        a_lines.len() >= 2,
        "need >=2 lines in frameA to derive leading"
    );
    let first_baseline_pt = a_lines[0].baseline_y_pt;
    let leading_pt = a_lines[1].baseline_y_pt - a_lines[0].baseline_y_pt;

    // Guard against a marginal fixture: the split line's baseline must clear
    // frameA's edge by a comfortable margin, else float/rounding could flip a
    // line between the emitter (1/64-int) and the protocol (f32).
    let region_a_h = chain.regions[0].geometry.height_pt;
    let last_a_baseline = a_lines.last().unwrap().baseline_y_pt;
    assert!(
        region_a_h - last_a_baseline >= 2.0 && (last_a_baseline + leading_pt) - region_a_h >= 2.0,
        "fixture is marginal near frameA's edge (h={region_a_h}, last={last_a_baseline}, L={leading_pt})"
    );

    // Run the protocol over the emitter's metrics and the composition chain.
    let flow = TextFlow::uniform(layout.len(), leading_pt, first_baseline_pt);
    let run = run_flow(&flow, &chain);

    let protocol_counts: Vec<usize> = run.placements.iter().map(|(_, f)| f.len()).collect();
    assert_eq!(
        protocol_counts, emitter_counts,
        "the flow protocol must reproduce the emitter's per-frame line assignment"
    );
    assert_eq!(
        run.overset,
        Overset::Fits,
        "the fixture fits, so must the protocol"
    );
}
