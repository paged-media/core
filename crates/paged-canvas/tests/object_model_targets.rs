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

//! v71 — what a panel needs from `Set` to drop its old dedicated paths:
//!
//! - a swatch's colour written as ONE value (`swatch.color`): a space
//!   change with its components, model and tint kept, one undo step;
//! - a table and a cell addressed by their own ids (`table:<id>`,
//!   `cell:<table>/<r>,<c>`): the engine finds the owning story, so no
//!   caller has to carry it.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_mutate::{SwatchColorSpec, Value};
use paged_wire::Address;

fn load(sample: paged_gen::Sample) -> CanvasModel {
    let bytes = paged_gen::write_idml(&sample).unwrap();
    CanvasModel::load("om-targets", &bytes, CanvasOptions::default()).expect("load")
}

#[test]
fn a_swatch_colour_changes_space_in_one_write_and_keeps_model_and_tint() {
    let mut m = load(paged_gen::samples::swatches::build());
    let swatch = Address::parse("swatch:Color/InkHalf").unwrap();
    let before = m.get_property_at(&swatch, "color").unwrap();
    let Value::SwatchColor(ref was) = before else {
        panic!("{before:?}")
    };
    assert_eq!(was.space, "LAB");
    assert_eq!(was.tint, Some(50.0));

    m.apply_mutation(&Mutation::Set {
        address: swatch.clone(),
        path: "color".into(),
        value: Value::SwatchColor(SwatchColorSpec {
            space: "CMYK".into(),
            value: vec![0.0, 100.0, 0.0, 0.0],
            model: None,
            tint: None,
        }),
    })
    .expect("one write");
    let Value::SwatchColor(now) = m.get_property_at(&swatch, "color").unwrap() else {
        unreachable!()
    };
    assert_eq!(now.space, "CMYK");
    assert_eq!(now.value, vec![0.0, 100.0, 0.0, 0.0]);
    assert_eq!(now.model, was.model, "the model is kept");
    assert_eq!(now.tint, Some(50.0), "the tint is kept");

    // A colour whose components do not fit its space is refused whole.
    assert!(m
        .apply_mutation(&Mutation::Set {
            address: swatch.clone(),
            path: "color".into(),
            value: Value::SwatchColor(SwatchColorSpec {
                space: "RGB".into(),
                value: vec![1.0, 2.0, 3.0, 4.0],
                model: None,
                tint: None,
            }),
        })
        .is_err());

    m.undo().expect("one undo step");
    assert_eq!(m.get_property_at(&swatch, "color").unwrap(), before);
}

#[test]
fn the_colour_path_takes_its_json_form_from_every_surface() {
    // The schema's JSON door (`paged set`, `paged.set`, a binding).
    let row = paged_introspect::virtual_paths::schema_for("swatch", None, "color").unwrap();
    let v = paged_introspect::schema::from_json(
        &row,
        &serde_json::json!({ "space": "RGB", "value": [255, 0, 0] }),
    )
    .unwrap();
    assert!(matches!(v, Value::SwatchColor(ref c) if c.space == "RGB"));
}

#[test]
fn table_and_cell_addresses_set_without_their_story() {
    let mut m = load(paged_gen::samples::tables::build());
    let cell = m.query("cell").unwrap().into_iter().next().expect("a cell");
    let Address::Cell { table_id, .. } = &cell else {
        unreachable!()
    };
    let table = Address::Table {
        id: table_id.clone(),
    };
    // the read twins answer on the bare ids
    assert!(m.get_property_at(&table, "appliedTableStyle").is_ok());
    let before = m
        .get_property_at(&cell, "cellVerticalJustification")
        .unwrap();
    let next = if before == Value::Text("BottomAlign".into()) {
        "TopAlign"
    } else {
        "BottomAlign"
    };
    m.apply_mutation(&Mutation::Set {
        address: cell.clone(),
        path: "cellVerticalJustification".into(),
        value: Value::Enum(next.into()),
    })
    .expect("set a cell by cell:<table>/<r>,<c>");
    assert_eq!(
        m.get_property_at(&cell, "cellVerticalJustification")
            .unwrap(),
        Value::Text(next.into())
    );
    m.apply_mutation(&Mutation::Set {
        address: table.clone(),
        path: "rows[0].height".into(),
        value: Value::Length(Some(31.0)),
    })
    .expect("set a row height by table:<id>");
    assert_eq!(
        m.get_property_at(&table, "rows[0].height").unwrap(),
        Value::Length(Some(31.0))
    );
    // and over the wire's string form
    let reply = m.read_property(&cell.to_string(), "cellVerticalJustification");
    assert_eq!(reply.unwrap(), Value::Text(next.into()));
}

/// v71 polish — `RequestGet` tells a MIXED value (a range whose runs
/// disagree) from a path the object does not have, and both from an
/// address that names nothing or does not parse: each refusal carries a
/// code, and mixed is an answer, not a refusal.
#[test]
fn a_read_tells_mixed_from_no_such_path() {
    use paged_canvas::ReadError;
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/generated/annual-base.idml"),
    )
    .unwrap();
    let mut m = CanvasModel::load("om-mixed", &bytes, CanvasOptions::default()).unwrap();
    let sid =
        m.scene()
            .stories
            .iter()
            .find(|s| {
                s.story.paragraphs.first().is_some_and(|p| {
                    p.runs.iter().map(|r| r.text.chars().count()).sum::<usize>() >= 6
                })
            })
            .map(|s| s.self_id.clone())
            .expect("a story with text");
    m.apply_mutation(&Mutation::Set {
        address: Address::parse(&format!("storyRange:{sid}@0..2")).unwrap(),
        path: "characterFontSize".into(),
        value: Value::Length(Some(31.0)),
    })
    .unwrap();
    let range = format!("storyRange:{sid}@0..6");

    let mixed = m.read_property(&range, "characterFontSize");
    assert_eq!(mixed, Err(ReadError::Mixed), "{mixed:?}");
    assert_eq!(ReadError::Mixed.code(), "mixed");

    let one = m.read_property(&format!("storyRange:{sid}@0..2"), "characterFontSize");
    assert_eq!(one, Ok(Value::Length(Some(31.0))));

    let unknown = m.read_property(&range, "noSuchPath").unwrap_err();
    assert_eq!(unknown.code(), "noSuchPath", "{unknown}");
    let nothing = m
        .read_property("frame:noSuchFrame", "frameFillColor")
        .unwrap_err();
    assert_eq!(nothing.code(), "notFound", "{nothing}");
    let bad = m.read_property("%%", "frameFillColor").unwrap_err();
    assert_eq!(bad.code(), "badAddress", "{bad}");
}

/// A blank document with one minted text frame whose story holds `text`.
fn minted_story(text: &str) -> (CanvasModel, String) {
    use paged_wire::{ElementId, PageId};
    let idml = paged_canvas::blank::blank_idml(612.0, 792.0);
    let mut m = CanvasModel::load("om-caret", &idml, CanvasOptions::default()).expect("load");
    let page = m.scene().spreads[0].spread.pages[0]
        .self_id
        .clone()
        .expect("page id");
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId(page),
            bounds: (36.0, 36.0, 300.0, 300.0),
        })
        .expect("frame");
    let Some(ElementId::TextFrame(frame)) = out.created_id else {
        panic!("expected a text frame, got {:?}", out.created_id);
    };
    let story = m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame.as_str()))
        .and_then(|f| f.parent_story.clone())
        .expect("minted story");
    if !text.is_empty() {
        m.apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: text.into(),
            cell: None,
        })
        .expect("text");
    }
    (m, story)
}

/// A caret (`storyRange:<story>@n..n`) is an address: the editor's
/// Character and Paragraph panels target one whenever nothing is
/// selected, including in a new, empty text frame. Paragraph paths read
/// and write the paragraph the caret stands in; a reversed range is
/// still refused.
#[test]
fn a_caret_is_an_address_for_the_paragraph_it_stands_in() {
    for (text, at) in [("", 0), ("Hello world", 3)] {
        let (mut m, sid) = minted_story(text);
        let caret = format!("storyRange:{sid}@{at}..{at}");
        assert!(Address::parse(&caret).is_ok(), "{caret} parses");
        m.apply_mutation(&Mutation::Set {
            address: Address::parse(&caret).unwrap(),
            path: "paragraphSpaceBefore".into(),
            value: Value::Length(Some(7.0)),
        })
        .unwrap_or_else(|e| panic!("{caret}: {e:?}"));
        assert_eq!(
            m.read_property(&caret, "paragraphSpaceBefore"),
            Ok(Value::Length(Some(7.0))),
            "{caret}"
        );
        // A character path reads the character before the caret, so a
        // caret in text answers what a typed character would carry.
        if !text.is_empty() {
            assert!(
                m.read_property(&caret, "characterFontSize").is_ok(),
                "{caret}: {:?}",
                m.read_property(&caret, "characterFontSize")
            );
        }
    }
    assert!(Address::parse("storyRange:Story/u1@6..0").is_err());
}
