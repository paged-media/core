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

//! W4.2 Full-Green core.mutation evidence — the W0.1 character metric
//! paths that shipped without their own routable test fn:
//! `CharacterLeading` (typography.leading), `CharacterTracking` +
//! `CharacterKerningMethod` (typography.tracking-kerning),
//! `CharacterHorizontalScale` + `CharacterSkew` (typography.scale-skew),
//! and `CharacterVerticalScale` (typography.vertical-scale).
//!
//! Each test pins the full mutation contract through the REAL model: a
//! `SetProperty` over a `StoryRange` writes the new value onto the
//! intersecting run (splitting it at the range boundary), the prior
//! value is restored byte-equal by the captured inverse, and the
//! neighbouring remainder run is untouched. Collision-free fn names so
//! the state test-map can route each onto its own core.mutation cell.

use std::path::PathBuf;

use paged_mutate::{apply, NodeId, Operation, PropertyPath, Value};
use paged_scene::Document;

fn fixture_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("text.idml");
    std::fs::read(path).expect("read text fixture")
}

/// The id of the first story whose first paragraph is a single run of
/// at least 20 characters — a clean target for a [0,10) sub-range
/// split. The text fixture's lead story is a 160-char single run.
fn long_single_run_story(doc: &Document) -> String {
    for s in &doc.stories {
        let Some(p) = s.story.paragraphs.first() else {
            continue;
        };
        if p.runs.len() == 1 && p.runs[0].text.chars().count() >= 20 {
            return s.self_id.clone();
        }
    }
    panic!("text fixture must carry a long single-run story");
}

/// Apply `path = value` over [0, 10) of the first long single-run
/// story, then assert the write via `check_set`, then undo and assert
/// the prior values are back via `check_undone`. The apply splits the
/// run at offset 10 (run 0 = [0,10) carries the new value, run 1 the
/// remainder); the inverse restores per-run property values, so we
/// assert on VALUES, not run count (the inverse does not re-merge runs
/// that now share identical formatting — matching the W0.1 contract).
fn round_trip<S, U>(path: PropertyPath, value: Value, check_set: S, check_undone: U)
where
    S: Fn(&Document, &str),
    U: Fn(&Document, &str),
{
    let bytes = fixture_bytes();
    let mut doc = paged_parse::import_idml_doc(&bytes).expect("open document");
    let story_id = long_single_run_story(&doc);

    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: story_id.clone(),
                start: 0,
                end: 10,
            },
            path,
            value,
        },
    )
    .expect("apply must succeed");

    check_set(&doc, &story_id);

    apply(&mut doc, &applied.inverse).expect("undo");
    check_undone(&doc, &story_id);
}

fn story<'a>(doc: &'a Document, sid: &str) -> &'a paged_model::Story {
    &doc.stories.iter().find(|s| s.self_id == sid).unwrap().story
}

#[test]
fn evid_character_leading_writes_and_inverse_restores() {
    round_trip(
        PropertyPath::CharacterLeading,
        Value::Length(Some(18.0)),
        |d, sid| {
            let s = story(d, sid);
            assert_eq!(s.paragraphs[0].runs[0].leading, Some(18.0));
            assert_ne!(
                s.paragraphs[0].runs[1].leading,
                Some(18.0),
                "the remainder run keeps its prior leading"
            );
        },
        |d, sid| {
            let s = story(d, sid);
            assert!(
                s.paragraphs[0].runs.iter().all(|r| r.leading.is_none()),
                "inverse restores the prior (absent) leading on every run"
            );
        },
    );
}

#[test]
fn evid_character_tracking_writes_and_inverse_restores() {
    round_trip(
        PropertyPath::CharacterTracking,
        Value::Length(Some(75.0)),
        |d, sid| {
            let s = story(d, sid);
            assert_eq!(s.paragraphs[0].runs[0].tracking, Some(75.0));
            assert_ne!(s.paragraphs[0].runs[1].tracking, Some(75.0));
        },
        |d, sid| {
            let s = story(d, sid);
            assert!(s.paragraphs[0].runs.iter().all(|r| r.tracking.is_none()));
        },
    );
}

#[test]
fn evid_character_kerning_method_writes_and_inverse_restores() {
    round_trip(
        PropertyPath::CharacterKerningMethod,
        Value::Text("Optical".to_string()),
        |d, sid| {
            let s = story(d, sid);
            assert_eq!(
                s.paragraphs[0].runs[0].kerning_method.as_deref(),
                Some("Optical")
            );
        },
        |d, sid| {
            let s = story(d, sid);
            assert!(s.paragraphs[0]
                .runs
                .iter()
                .all(|r| r.kerning_method.is_none()));
        },
    );
}

#[test]
fn evid_character_horizontal_scale_writes_and_inverse_restores() {
    round_trip(
        PropertyPath::CharacterHorizontalScale,
        Value::Length(Some(130.0)),
        |d, sid| {
            let s = story(d, sid);
            assert_eq!(s.paragraphs[0].runs[0].horizontal_scale, Some(130.0));
            assert_ne!(s.paragraphs[0].runs[1].horizontal_scale, Some(130.0));
        },
        |d, sid| {
            let s = story(d, sid);
            assert!(s.paragraphs[0]
                .runs
                .iter()
                .all(|r| r.horizontal_scale.is_none()));
        },
    );
}

#[test]
fn evid_character_skew_writes_and_inverse_restores() {
    round_trip(
        PropertyPath::CharacterSkew,
        Value::Length(Some(12.0)),
        |d, sid| {
            let s = story(d, sid);
            assert_eq!(s.paragraphs[0].runs[0].skew, Some(12.0));
            assert_ne!(s.paragraphs[0].runs[1].skew, Some(12.0));
        },
        |d, sid| {
            let s = story(d, sid);
            assert!(s.paragraphs[0].runs.iter().all(|r| r.skew.is_none()));
        },
    );
}

#[test]
fn evid_character_vertical_scale_writes_and_inverse_restores() {
    round_trip(
        PropertyPath::CharacterVerticalScale,
        Value::Length(Some(85.0)),
        |d, sid| {
            let s = story(d, sid);
            assert_eq!(s.paragraphs[0].runs[0].vertical_scale, Some(85.0));
            assert_ne!(s.paragraphs[0].runs[1].vertical_scale, Some(85.0));
        },
        |d, sid| {
            let s = story(d, sid);
            assert!(s.paragraphs[0]
                .runs
                .iter()
                .all(|r| r.vertical_scale.is_none()));
        },
    );
}
