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

//! Threading inside a batch, read the way the EDITOR reads it.
//!
//! `batch_composition.rs` pins that chained `LinkFrames` compose inside
//! one batch — but it reads only the scene's `parent_story`. The
//! showcase harness reads three other things and saw them disagree,
//! three times, on the wasm built from this branch:
//!
//! 1. the `hitTest` message at a frame's centre (the story the editor
//!    edits when you click the frame);
//! 2. the BUILT document — which frames the poured story's lines land
//!    in (what the page paints);
//! 3. both of those AFTER the deferred rebuild a mixed batch settles
//!    once, at its end.
//!
//! These tests drive the harness's exact three-batch shape (mint with
//! handles → link with real ids → pour + style) plus the mixed-batch
//! two-frame shape, and assert all three reads for every frame in the
//! chain. Each batched shape has a one-mutation-at-a-time control so a
//! failure is attributable to batching and nothing else.

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::{
    channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions, HitFilter, PageId,
};

const PAGE: &str = "p1";

/// `batch_composition.rs`'s fixture, parametric over how many stories
/// it carries (ids `s0`, `s1`, … — the parser names a story from its
/// file, `Stories/Story_<id>.xml`): `s0` is the one `tf1` shows, every
/// further one is an unplaced story that only exists to occupy an id.
fn idml_with_stories(count: usize) -> Vec<u8> {
    let story_ids: Vec<String> = (0..count).map(|i| format!("s{i}")).collect();
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        // Two layers so the mixed-batch shape can move a frame between
        // them with `itemLayer`, exactly as the showcase harness does.
        let mut designmap = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<Layer Self="layer-body" Name="Body" Visible="true" Locked="false"/>
<Layer Self="layer-2" Name="Two" Visible="true" Locked="false"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
"#,
        );
        for id in &story_ids {
            designmap.push_str(&format!(
                r#"<idPkg:Story src="Stories/Story_{id}.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
"#
            ));
        }
        designmap.push_str("</Document>");
        zip.write_all(designmap.as_bytes()).unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="{}" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
                story_ids[0]
            )
            .as_bytes(),
        )
        .unwrap();
        for id in &story_ids {
            zip.start_file(format!("Stories/Story_{id}.xml"), opts)
                .unwrap();
            zip.write_all(
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="{id}">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#
                )
                .as_bytes(),
            )
            .unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn small_idml() -> Vec<u8> {
    idml_with_stories(1)
}

fn read_inter() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

/// A real font, so the pour lays out LINES and the built document can
/// say which frame each one landed in.
fn load_bytes(bytes: &[u8]) -> CanvasModel {
    CanvasModel::load(
        "doc1",
        bytes,
        CanvasOptions {
            fonts: vec![read_inter()],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

fn load() -> CanvasModel {
    load_bytes(&small_idml())
}

/// A document whose wire-minted story ids are SPARSE: three stories
/// named `Story/u3`, `Story/u4`, `Story/u5` — numbered past their
/// count. That is what a document authored through the wire looks like
/// as soon as any story was deleted, rolled back or minted by another
/// path: `stories.len()` falls behind the numbers already taken.
///
/// Planted through `scene_mut` because the parser names a story from
/// its file and can never produce the minter's `Story/u<n>` spelling.
fn load_sparse() -> CanvasModel {
    let mut model = load_bytes(&idml_with_stories(3));
    {
        let scene = model.scene_mut();
        for (i, story) in scene.stories.iter_mut().enumerate() {
            story.self_id = format!("Story/u{}", i + 3);
        }
        for frame in &mut scene.spreads[0].spread.text_frames {
            if frame.parent_story.as_deref() == Some("s0") {
                frame.parent_story = Some("Story/u3".into());
            }
        }
    }
    model.rebuild_after_mutation().expect("rebuild");
    let ids: Vec<&str> = model
        .scene()
        .stories
        .iter()
        .map(|s| s.self_id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["Story/u3", "Story/u4", "Story/u5"],
        "the sparse fixture"
    );
    model
}

/// 340 × 24 pt frames stacked down the page, clear of the fixture's own
/// `tf1` (100..400 × 100..400) so a hit at a centre can only mean one
/// frame. `(top, left, bottom, right)`, page-local — the wire's order.
fn frame_bounds(i: usize) -> (f32, f32, f32, f32) {
    let top = 420.0 + i as f32 * 60.0;
    (top, 50.0, top + 24.0, 390.0)
}

/// The `docPoint` the harness sends: the frame's centre, `(x, y)`.
fn centre(b: (f32, f32, f32, f32)) -> (f32, f32) {
    ((b.1 + b.3) / 2.0, (b.0 + b.2) / 2.0)
}

fn insert_frame(i: usize) -> Mutation {
    Mutation::InsertTextFrame {
        page_id: PageId(PAGE.into()),
        bounds: frame_bounds(i),
    }
}

fn bind(handle: &str) -> Mutation {
    Mutation::BindCreated {
        handle: handle.into(),
    }
}

/// ~900 characters — enough to run past four 24 pt frames, so every
/// frame in the chain carries lines and the tail is overset.
fn long_text() -> String {
    let sentence = "The annual pours one story through four linked frames on its flagship spread. ";
    let mut s = String::new();
    while s.chars().count() < 900 {
        s.push_str(sentence);
    }
    s
}

fn link(from: &str, to: &str) -> Mutation {
    Mutation::LinkFrames {
        from: from.into(),
        to: to.into(),
    }
}

fn pour(story: &str) -> Mutation {
    Mutation::InsertText {
        story_id: story.into(),
        offset: 0,
        text: long_text(),
        cell: None,
    }
}

fn style(story: &str) -> Mutation {
    Mutation::ApplyStyle {
        story_id: story.into(),
        start: 0,
        end: long_text().chars().count() as u32,
        style: "ParagraphStyle/Body".into(),
        scope: paged_mutate::operation::StyleScope::Paragraph,
        cell: None,
    }
}

fn move_to_layer(frame: &str, layer: &str) -> Mutation {
    Mutation::SetElementProperty {
        element_id: ElementId::TextFrame(frame.into()),
        path: paged_mutate::PropertyPath::ItemLayer,
        value: paged_mutate::Value::Text(layer.into()),
    }
}

/// The frames of a chain, in chain order, with the story each was BORN
/// with (`insertTextFrame` mints one per frame).
struct Chain {
    frames: Vec<String>,
    born_stories: Vec<String>,
}

impl Chain {
    fn head_story(&self) -> &str {
        &self.born_stories[0]
    }
}

fn scene_story_of(model: &CanvasModel, frame: &str) -> Option<String> {
    model.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame))
        .and_then(|f| f.parent_story.clone())
}

fn scene_layer_of(model: &CanvasModel, frame: &str) -> Option<String> {
    model.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame))
        .and_then(|f| f.item_layer.clone())
}

/// Mint `n` frames the harness's way: one batch, every insert followed
/// by a `bindCreated`, ids read back off the reply's `minted` list.
fn mint_in_one_batch(model: &mut CanvasModel, n: usize) -> Chain {
    let mut ops = Vec::with_capacity(n * 2);
    for i in 0..n {
        ops.push(insert_frame(i));
        ops.push(bind(&format!("f{i}")));
    }
    let out = model
        .apply_mutation(&Mutation::Batch { ops })
        .expect("mint batch");
    assert_eq!(out.minted.len(), n, "one mint per frame: {:?}", out.minted);
    let frames: Vec<String> = out
        .minted
        .iter()
        .map(|m| match &m.element {
            ElementId::TextFrame(id) => id.clone(),
            other => panic!("expected a text frame, got {other:?}"),
        })
        .collect();
    let born_stories: Vec<String> = out
        .minted
        .iter()
        .map(|m| {
            m.story_id
                .clone()
                .expect("a minted text frame carries its story")
        })
        .collect();
    // The reply and the scene agree about what was born.
    for (f, s) in frames.iter().zip(&born_stories) {
        assert_eq!(scene_story_of(model, f).as_deref(), Some(s.as_str()));
    }
    // And every frame was born with a story of its OWN: `insertTextFrame`
    // mints one per frame, and a chain whose members already share a
    // story before any link ran is not threaded, it is confused — every
    // pour into "B's story" lands in A's, and the threading oracle reads
    // green before the links are sent.
    let distinct: std::collections::BTreeSet<&str> =
        born_stories.iter().map(String::as_str).collect();
    assert_eq!(
        distinct.len(),
        n,
        "{n} frames minted in one batch must be born on {n} DISTINCT stories; got {born_stories:?}",
    );
    eprintln!("minted in one batch: frames {frames:?} born on {born_stories:?}");
    Chain {
        frames,
        born_stories,
    }
}

/// Every story in the scene with its character count — what a pour
/// landed in, and what it did not.
fn scene_stories(model: &CanvasModel) -> Vec<(String, usize)> {
    model
        .scene()
        .stories
        .iter()
        .map(|s| {
            let chars: usize = s
                .story
                .paragraphs
                .iter()
                .flat_map(|p| p.runs.iter())
                .map(|r| r.text.chars().count())
                .sum();
            (s.self_id.clone(), chars)
        })
        .collect()
}

/// Every laid-out line in the built document as `(frame, story)`.
fn built_lines(model: &CanvasModel) -> Vec<(Option<String>, String)> {
    model
        .built()
        .pages
        .iter()
        .flat_map(|p| p.story_layout.iter())
        .map(|l| (l.frame_id.clone(), l.story_id.clone()))
        .collect()
}

/// The control's minting: one `insertTextFrame` per mutation.
fn mint_one_at_a_time(model: &mut CanvasModel, n: usize) -> Chain {
    let mut frames = Vec::new();
    let mut born_stories = Vec::new();
    for i in 0..n {
        let out = model.apply_mutation(&insert_frame(i)).expect("frame");
        let id = match out.created_id {
            Some(ElementId::TextFrame(id)) => id,
            other => panic!("expected a text frame, got {other:?}"),
        };
        born_stories.push(scene_story_of(model, &id).expect("born story"));
        frames.push(id);
    }
    Chain {
        frames,
        born_stories,
    }
}

/// (a) + (b): every frame of the chain carries the head's story in the
/// SCENE, and the hit test at its centre — the exact call the `hitTest`
/// message makes — answers that same story for THAT frame.
fn assert_threaded(model: &CanvasModel, chain: &Chain, stage: &str) {
    let head = chain.head_story();
    for (i, frame) in chain.frames.iter().enumerate() {
        assert_eq!(
            scene_story_of(model, frame).as_deref(),
            Some(head),
            "{stage}: frame {i} ({frame}) must carry the head's story in the scene \
             (born with {})",
            chain.born_stories[i],
        );
        let hit = model.hit_test_filtered(
            &PageId(PAGE.into()),
            centre(frame_bounds(i)),
            HitFilter::Text,
        );
        assert_eq!(
            hit.frame_id.as_deref(),
            Some(frame.as_str()),
            "{stage}: the hit at frame {i}'s centre must land on frame {frame}; got {hit:?}",
        );
        assert_eq!(
            hit.story_id.as_deref(),
            Some(head),
            "{stage}: the hit at frame {i} ({frame}) must answer the head's story \
             (born with {}); got {hit:?}",
            chain.born_stories[i],
        );
    }
}

/// (c): after the pour, the BUILT document places lines of the head's
/// story in every frame of the chain — what the page actually paints.
fn assert_poured_through(model: &CanvasModel, chain: &Chain, stage: &str) {
    let head = chain.head_story();
    let built = model.built();
    let mut summary = Vec::new();
    for (i, frame) in chain.frames.iter().enumerate() {
        let lines: Vec<_> = built
            .pages
            .iter()
            .flat_map(|p| p.story_layout.iter())
            .filter(|l| l.frame_id.as_deref() == Some(frame.as_str()))
            .collect();
        let stories: std::collections::BTreeSet<&str> =
            lines.iter().map(|l| l.story_id.as_str()).collect();
        summary.push(format!(
            "frame {i} ({frame}): {} lines from {stories:?}",
            lines.len()
        ));
        assert!(
            !lines.is_empty(),
            "{stage}: frame {i} ({frame}) carries no lines in the built document — the pour \
             never reached it; overset: {:?}; so far: {summary:?}; every built line \
             (frame, story): {:?}; scene stories (id, chars): {:?}",
            built.diagnostics.items,
            built_lines(model),
            scene_stories(model),
        );
        assert_eq!(
            stories.into_iter().collect::<Vec<_>>(),
            vec![head],
            "{stage}: every line in frame {i} ({frame}) must belong to the head's story",
        );
    }
}

// ── the annual's flagship shape: four frames, three links, one pour ────

/// The harness's EXACT three batches: mint with handles, link with the
/// real ids the reply named, pour + style the head story. Read like the
/// editor reads: scene, hit test, built document.
#[test]
fn four_frames_linked_in_one_batch_thread_for_the_hit_test_and_the_build() {
    let mut model = load();
    let chain = mint_in_one_batch(&mut model, 4);
    let [a, b, c, d] = [
        chain.frames[0].as_str(),
        chain.frames[1].as_str(),
        chain.frames[2].as_str(),
        chain.frames[3].as_str(),
    ];

    let builds_before = model.last_rebuild_stats().rebuilds;
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![link(a, b), link(b, c), link(c, d)],
        })
        .expect("three links in one batch");
    eprintln!(
        "link batch: {} build(s); scene stories now {:?}",
        model.last_rebuild_stats().rebuilds - builds_before,
        chain
            .frames
            .iter()
            .map(|f| scene_story_of(&model, f))
            .collect::<Vec<_>>()
    );
    assert_threaded(&model, &chain, "after the link batch");

    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![pour(chain.head_story()), style(chain.head_story())],
        })
        .expect("pour + style in one batch");
    assert_threaded(&model, &chain, "after the pour batch");
    assert_poured_through(&model, &chain, "after the pour batch");
}

/// The control: the same sequence, one mutation each. A failure above
/// that does not reproduce here is a batching defect.
#[test]
fn four_frames_linked_one_mutation_at_a_time_thread_for_the_hit_test_and_the_build() {
    let mut model = load();
    let chain = mint_one_at_a_time(&mut model, 4);
    for pair in chain.frames.windows(2) {
        model
            .apply_mutation(&link(&pair[0], &pair[1]))
            .expect("link");
    }
    assert_threaded(&model, &chain, "after the links");

    model
        .apply_mutation(&pour(chain.head_story()))
        .expect("pour");
    model
        .apply_mutation(&style(chain.head_story()))
        .expect("style");
    assert_threaded(&model, &chain, "after the pour");
    assert_poured_through(&model, &chain, "after the pour");
}

// ── the mixed-lane shape: link, re-layer, pour — in ONE batch ─────────

/// The manuscript chapter's shape: the link rides a batch that also
/// moves the target to another layer and pours text, so the batch
/// takes the MIXED lane (`apply_mixed_batch`) and its rebuild is
/// deferred to one settle at the end. The target must still carry the
/// head's story for the hit test, sit on its new layer, and paint the
/// overflow.
#[test]
fn link_relayer_and_pour_in_one_mixed_batch_thread_for_the_hit_test_and_the_build() {
    let mut model = load();
    let chain = mint_in_one_batch(&mut model, 2);
    let (a, b) = (chain.frames[0].as_str(), chain.frames[1].as_str());

    let builds_before = model.last_rebuild_stats().rebuilds;
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                link(a, b),
                move_to_layer(b, "layer-2"),
                pour(chain.head_story()),
            ],
        })
        .expect("link + relayer + pour in one batch");
    eprintln!(
        "mixed batch: {} build(s)",
        model.last_rebuild_stats().rebuilds - builds_before
    );

    assert_eq!(
        scene_layer_of(&model, b).as_deref(),
        Some("layer-2"),
        "the relayer child landed",
    );
    assert_threaded(&model, &chain, "after the mixed batch");
    assert_poured_through(&model, &chain, "after the mixed batch");
}

// ── the same shapes on a document whose story ids are sparse ─────────

/// Four `insertTextFrame` children of ONE batch each mint a story of
/// their own — on a document whose story ids are sparse, which is every
/// real document.
///
/// The story id is minted at TRANSLATION time as
/// `Story/u<stories.len() + offset>`, skipping ids the document already
/// holds. But `stories.len()` is a count, not a ceiling, and in the
/// translatable lane every child translates before any applies: the
/// guard sees only the stories already in the document, so every
/// offset walks up to the SAME first free number. The first frame's
/// apply then creates that story and the other three ADOPT it
/// (`insert_node` reuses an existing `ParentStory`) — the annual's
/// flagship spread was born "threaded" before any link was sent.
#[test]
fn frames_minted_in_one_batch_on_a_sparse_id_document_are_born_on_distinct_stories() {
    let mut model = load_sparse();
    let stories_before = model.scene().stories.len();
    let chain = mint_in_one_batch(&mut model, 4);
    assert_eq!(
        model.scene().stories.len(),
        stories_before + 4,
        "four frames, four new stories; born: {:?}",
        chain.born_stories,
    );
}

/// The one-at-a-time control on the sparse document: each mint applies
/// before the next translates, so the guard sees its sibling.
#[test]
fn frames_minted_one_at_a_time_on_a_sparse_id_document_are_born_on_distinct_stories() {
    let mut model = load_sparse();
    let stories_before = model.scene().stories.len();
    let chain = mint_one_at_a_time(&mut model, 4);
    let distinct: std::collections::BTreeSet<&str> =
        chain.born_stories.iter().map(String::as_str).collect();
    assert_eq!(distinct.len(), 4, "born: {:?}", chain.born_stories);
    assert_eq!(model.scene().stories.len(), stories_before + 4);
}

/// The harness's three batches on the sparse document — the shape the
/// annual actually runs, since its story ids are nowhere near dense.
#[test]
fn four_frames_on_a_sparse_id_document_linked_in_one_batch_thread_for_the_hit_test_and_the_build() {
    let mut model = load_sparse();
    let chain = mint_in_one_batch(&mut model, 4);
    let [a, b, c, d] = [
        chain.frames[0].as_str(),
        chain.frames[1].as_str(),
        chain.frames[2].as_str(),
        chain.frames[3].as_str(),
    ];
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![link(a, b), link(b, c), link(c, d)],
        })
        .expect("three links in one batch");
    assert_threaded(&model, &chain, "after the link batch");
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![pour(chain.head_story()), style(chain.head_story())],
        })
        .expect("pour + style in one batch");
    assert_threaded(&model, &chain, "after the pour batch");
    assert_poured_through(&model, &chain, "after the pour batch");
}

/// The control for the mixed shape: the same three children, one
/// mutation each.
#[test]
fn link_relayer_and_pour_one_mutation_at_a_time_thread_for_the_hit_test_and_the_build() {
    let mut model = load();
    let chain = mint_one_at_a_time(&mut model, 2);
    let (a, b) = (chain.frames[0].as_str(), chain.frames[1].as_str());

    model.apply_mutation(&link(a, b)).expect("link");
    model
        .apply_mutation(&move_to_layer(b, "layer-2"))
        .expect("relayer");
    model
        .apply_mutation(&pour(chain.head_story()))
        .expect("pour");

    assert_eq!(scene_layer_of(&model, b).as_deref(), Some("layer-2"));
    assert_threaded(&model, &chain, "after the three mutations");
    assert_poured_through(&model, &chain, "after the three mutations");
}
