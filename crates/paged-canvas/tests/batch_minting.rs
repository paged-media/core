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

//! Id minting across KINDS that share one namespace.
//!
//! `batch_threading.rs` pins that stories minted together are distinct.
//! This file pins the same for the OTHER translation-time mints — the
//! hyperlink trio, tables and anchored frames — which all draw their
//! number from the page-item minter (`u<hex>`) but land where a
//! page-item scan never looks: the designmap and the story paragraphs.
//!
//! A paged.doc lowering measured on a real document sent one text step
//! as a mixed batch (`insertText` + two `insertHyperlink`s) and then
//! `insertTable` on its own; afterwards `designmap.hyperlinks` held
//! `Hyperlink/ueef094` TWICE and the table's `Self` was `ueef094` too.
//! The minter had re-scanned the page items before every mint, seen no
//! hyperlink and no table there, and handed out the same successor
//! three times.
//!
//! Every shape here runs on a document whose ids are SPARSE (a page
//! item and a hyperlink numbered past their count — what any document
//! authored through the wire looks like once anything was deleted,
//! rolled back or minted by another lane), and asserts three things:
//! every mint of the batch is distinct, every mint is distinct across
//! kinds, and no mint lands on an id the document already had.

use std::collections::BTreeSet;
use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, PageId};
use paged_mutate::operation::GuideOrientationSpec;

const PAGE: &str = "p1";
const STORY: &str = "s0";
const TEXT: &str = "Hello world";

/// One page, one frame on story `s0` ("Hello world"), one rectangle.
/// The parser names a story from its file, so `s0` is never in the
/// minter's `u<hex>` spelling; the sparse ids are planted afterwards.
fn small_idml() -> Vec<u8> {
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
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<Layer Self="layer-body" Name="Body" Visible="true" Locked="false"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_s0.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="s0" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s0.xml", opts).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="s0">
<ParagraphStyleRange>
<CharacterStyleRange><Content>{TEXT}</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// The highest page-item number the sparse fixture carries (`u1f`).
const SPARSE_PAGE_ITEM: u64 = 0x1f;
/// The hyperlink the sparse fixture already holds (`Hyperlink/u20`) —
/// numbered one past the page item, so a minter that scans page items
/// only hands its very next successor to an id the document has.
const SPARSE_HYPERLINK: u64 = 0x20;

/// The fixture with SPARSE ids planted: the rectangle renamed to
/// `u1f` and a hyperlink trio at `u20` already registered. Planted
/// through `scene_mut` because the parser names page items and links
/// from the file, so only a wire-authored document reaches this shape.
fn load_sparse() -> CanvasModel {
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    {
        let scene = model.scene_mut();
        for r in &mut scene.spreads[0].spread.rectangles {
            if r.self_id.as_deref() == Some("r1") {
                r.self_id = Some(format!("u{SPARSE_PAGE_ITEM:x}"));
            }
        }
        let base = format!("u{SPARSE_HYPERLINK:x}");
        scene
            .designmap
            .hyperlink_destinations
            .push(paged_model::HyperlinkDestination {
                self_id: format!("HyperlinkURLDestination/{base}"),
                kind: paged_model::HyperlinkDestinationKind::Url("https://paged.media".into()),
            });
        scene.designmap.hyperlinks.push(paged_model::Hyperlink {
            self_id: format!("Hyperlink/{base}"),
            name: None,
            source: Some(format!("HyperlinkTextSource/{base}")),
            destination: Some(format!("HyperlinkURLDestination/{base}")),
        });
    }
    model.rebuild_after_mutation().expect("rebuild");
    let before = shared_ids(&model);
    assert!(
        before.contains(&format!("u{SPARSE_PAGE_ITEM:x}"))
            && before.contains(&format!("Hyperlink/u{SPARSE_HYPERLINK:x}")),
        "the sparse fixture: {before:?}"
    );
    model
}

/// Every id in the document that a translation-time mint could land
/// on, as the strings the model holds them in. Duplicates are KEPT so
/// a collision shows as a repeated entry.
fn shared_ids(model: &CanvasModel) -> Vec<String> {
    let scene = model.scene();
    let mut ids: Vec<String> = Vec::new();
    for parsed in &scene.spreads {
        let s = &parsed.spread;
        ids.extend(s.text_frames.iter().filter_map(|f| f.self_id.clone()));
        ids.extend(s.rectangles.iter().filter_map(|r| r.self_id.clone()));
        ids.extend(s.ovals.iter().filter_map(|o| o.self_id.clone()));
        ids.extend(s.graphic_lines.iter().filter_map(|l| l.self_id.clone()));
        ids.extend(s.polygons.iter().filter_map(|p| p.self_id.clone()));
        ids.extend(s.groups.iter().filter_map(|g| g.self_id.clone()));
    }
    for story in &scene.stories {
        ids.push(story.self_id.clone());
        for p in &story.story.paragraphs {
            ids.extend(p.table.iter().filter_map(|t| t.self_id.clone()));
            ids.extend(p.anchored_frames.iter().filter_map(|a| a.self_id.clone()));
        }
    }
    for h in &scene.designmap.hyperlinks {
        ids.push(h.self_id.clone());
        // The text source has no collection of its own (it is a run
        // tag), so the link's reference is its one home. The
        // destination's home is `hyperlink_destinations` below; listing
        // the reference too would count every valid link twice.
        ids.extend(h.source.clone());
    }
    ids.extend(
        scene
            .designmap
            .hyperlink_destinations
            .iter()
            .map(|d| d.self_id.clone()),
    );
    ids.extend(scene.designmap.sections.iter().map(|s| s.self_id.clone()));
    ids
}

/// The document holds no id twice — the invariant every shape below
/// must keep. Names the repeated ids when it does not.
fn assert_no_duplicates(ids: &[String], stage: &str) {
    let mut seen = BTreeSet::new();
    let dupes: Vec<&String> = ids.iter().filter(|id| !seen.insert(*id)).collect();
    assert!(
        dupes.is_empty(),
        "{stage}: the document holds these ids more than once: {dupes:?}\nall ids: {ids:?}"
    );
}

/// The bare `u<hex>` number an id was minted from: the whole id for a
/// page item / table / anchored frame, the suffix after the kind for
/// the hyperlink trio.
fn base_of(id: &str) -> &str {
    id.rsplit_once('/').map_or(id, |(_, rest)| rest)
}

fn number_of(id: &str) -> u64 {
    let hex = base_of(id)
        .strip_prefix('u')
        .unwrap_or_else(|| panic!("{id} is not a u<hex> id"));
    u64::from_str_radix(hex, 16).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// What the batch minted, in the kinds this file cares about.
#[derive(Debug)]
struct Minted {
    hyperlinks: Vec<String>,
    tables: Vec<String>,
    anchored: Vec<String>,
}

impl Minted {
    fn all(&self) -> Vec<&String> {
        self.hyperlinks
            .iter()
            .chain(&self.tables)
            .chain(&self.anchored)
            .collect()
    }
}

/// The ids present after but not before, per kind, in document order.
fn minted_since(before: &[String], model: &CanvasModel) -> Minted {
    let before: BTreeSet<&String> = before.iter().collect();
    let scene = model.scene();
    let hyperlinks = scene
        .designmap
        .hyperlinks
        .iter()
        .map(|h| h.self_id.clone())
        .filter(|id| !before.contains(id))
        .collect();
    let mut tables = Vec::new();
    let mut anchored = Vec::new();
    for story in &scene.stories {
        for p in &story.story.paragraphs {
            tables.extend(
                p.table
                    .iter()
                    .filter_map(|t| t.self_id.clone())
                    .filter(|id| !before.contains(id)),
            );
            anchored.extend(
                p.anchored_frames
                    .iter()
                    .filter_map(|a| a.self_id.clone())
                    .filter(|id| !before.contains(id)),
            );
        }
    }
    Minted {
        hyperlinks,
        tables,
        anchored,
    }
}

/// The three assertions every shape makes: distinct within the batch,
/// distinct ACROSS kinds (one number per mint, whatever it named), and
/// past every id the document had — never on `u1f` or `u20`.
fn assert_distinct_and_fresh(
    before: &[String],
    model: &CanvasModel,
    expect: (usize, usize, usize),
) {
    let after = shared_ids(model);
    assert_no_duplicates(&after, "after the batch");
    let minted = minted_since(before, model);
    assert_eq!(
        (
            minted.hyperlinks.len(),
            minted.tables.len(),
            minted.anchored.len()
        ),
        expect,
        "(hyperlinks, tables, anchored frames) minted: {minted:?}"
    );
    let bases: BTreeSet<&str> = minted.all().into_iter().map(|id| base_of(id)).collect();
    assert_eq!(
        bases.len(),
        minted.all().len(),
        "every mint must carry its own number, whatever kind it named: {minted:?}"
    );
    let floor = SPARSE_PAGE_ITEM.max(SPARSE_HYPERLINK);
    for id in minted.all() {
        assert!(
            number_of(id) > floor,
            "{id} was minted onto or below an id the document already had (u{floor:x}): \
             {minted:?}"
        );
    }
    eprintln!("minted: {minted:?}");
}

fn hyperlink(start: u32, end: u32) -> Mutation {
    Mutation::InsertHyperlink {
        story_id: STORY.into(),
        start,
        end,
        url: format!("https://paged.media/{start}-{end}"),
    }
}

fn table() -> Mutation {
    Mutation::InsertTable {
        story_id: STORY.into(),
        rows: 2,
        cols: 2,
        header_rows: 0,
        footer_rows: 0,
        column_widths: Vec::new(),
        row_heights: Vec::new(),
    }
}

fn anchored(offset: u32) -> Mutation {
    Mutation::InsertAnchoredFrame {
        story_id: STORY.into(),
        offset,
        width: 40.0,
        height: 40.0,
        image_uri: None,
    }
}

/// The text op that makes a batch MIXED (not translatable to one
/// `Operation::Batch`): the lane the DOCX lowering's text step takes.
fn append_text() -> Mutation {
    Mutation::InsertText {
        story_id: STORY.into(),
        offset: TEXT.len() as u32,
        text: " and more".into(),
        cell: None,
    }
}

// ── The finding ─────────────────────────────────────────────────────

/// Two hyperlinks and a table in ONE translatable batch: the offset is
/// threaded, so within the batch the mints differ — but the first one
/// must not land on the hyperlink the document already has.
#[test]
fn two_hyperlinks_and_a_table_in_one_batch_mint_distinct_fresh_ids() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![hyperlink(0, 5), hyperlink(6, 11), table()],
        })
        .expect("batch");
    assert_distinct_and_fresh(&before, &model, (2, 1, 0));
}

/// The DOCX lowering's lane: a text op makes the batch MIXED, so every
/// child is applied on its own — each mint re-scans a document its
/// sibling has already changed, and must SEE that sibling's mint.
#[test]
fn two_hyperlinks_and_a_table_in_one_mixed_batch_mint_distinct_fresh_ids() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![append_text(), hyperlink(0, 5), hyperlink(6, 11), table()],
        })
        .expect("mixed batch");
    assert_distinct_and_fresh(&before, &model, (2, 1, 0));
}

/// The exact sequence paged.doc issues: the text step as one mixed
/// batch (`insertText` + the hyperlinks), then `insertTable` as its
/// own mutation (its cells need the minted id). The table's number
/// must differ from both hyperlinks'.
#[test]
fn the_docx_lowering_shape_mints_distinct_fresh_ids() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![append_text(), hyperlink(0, 5), hyperlink(6, 11)],
        })
        .expect("text step");
    model.apply_mutation(&table()).expect("table");
    assert_distinct_and_fresh(&before, &model, (2, 1, 0));
}

/// The control: one mutation at a time. The same three mints, with the
/// scene settled between each — every kind must be visible to the next.
#[test]
fn hyperlinks_a_table_and_an_anchored_frame_minted_one_at_a_time_are_distinct_and_fresh() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    model.apply_mutation(&hyperlink(0, 5)).expect("hyperlink 1");
    model
        .apply_mutation(&hyperlink(6, 11))
        .expect("hyperlink 2");
    model.apply_mutation(&table()).expect("table");
    model.apply_mutation(&anchored(0)).expect("anchored frame");
    assert_distinct_and_fresh(&before, &model, (2, 1, 1));
}

/// Anchored frames are the third kind on the shared number. Two in one
/// mixed batch: the applier rejects a duplicate (`DuplicateNodeId`),
/// which rolls the whole batch back — so this asserts the batch
/// APPLIES at all, then that the frames and the link differ.
#[test]
fn two_anchored_frames_and_a_hyperlink_in_one_mixed_batch_mint_distinct_fresh_ids() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![append_text(), anchored(0), anchored(6), hyperlink(0, 5)],
        })
        .expect("mixed batch");
    assert_distinct_and_fresh(&before, &model, (1, 0, 2));
}

/// A second batch after the first: the second batch's mints must start
/// past the first's, whatever kinds the first minted.
#[test]
fn a_second_batch_mints_past_the_first_batch_across_kinds() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![hyperlink(0, 5), table()],
        })
        .expect("first batch");
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![hyperlink(6, 11), table(), anchored(0)],
        })
        .expect("second batch");
    assert_distinct_and_fresh(&before, &model, (2, 2, 1));
}

// ── Sections and guides mint at APPLY time ──────────────────────────

/// Sections (`Section/u<n>`) and guides (`Guide/<spread>/<index>`) are
/// numbered by the applier, each against the collection it lands in,
/// with the scene settled between children — so two of each in one
/// batch are distinct by construction. Pinned so a move of either
/// mint to translation time has to keep it.
#[test]
fn two_sections_and_two_guides_in_one_batch_mint_distinct_ids() {
    let mut model = load_sparse();
    let before = shared_ids(&model);
    let section = |prefix: &str| Mutation::InsertSection {
        at_page: PageId(PAGE.into()),
        prefix: Some(prefix.into()),
        numbering_style: None,
        start_at: Some(1),
    };
    let guide = |position: f32| Mutation::InsertGuide {
        spread_id: "s1".into(),
        orientation: GuideOrientationSpec::Vertical,
        position,
        page_index: 0,
    };
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![section("A-"), section("B-"), guide(100.0), guide(200.0)],
        })
        .expect("batch");
    let after = shared_ids(&model);
    assert_no_duplicates(&after, "after the batch");
    let sections: Vec<&str> = model
        .scene()
        .designmap
        .sections
        .iter()
        .map(|s| s.self_id.as_str())
        .collect();
    assert_eq!(sections.len(), 2, "{sections:?}");
    assert_ne!(sections[0], sections[1], "{sections:?}");
    assert!(
        sections.iter().all(|s| !before.contains(&s.to_string())),
        "{sections:?} vs {before:?}"
    );
    let guides = &model.scene().spreads[0].spread.guides;
    assert_eq!(guides.len(), 2, "{guides:?}");
    let positions: BTreeSet<i64> = guides.iter().map(|g| g.location as i64).collect();
    assert_eq!(positions, BTreeSet::from([100, 200]), "{guides:?}");
}
