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

//! v74 — tracked deletions accepted and rejected over the wire (D-01).
//! InDesign 20.0.1 (`track-changes-probe`): "Alpha beta INSERTED delta."
//! with "gamma " deleted reads "Alpha beta INSERTED gamma delta." once the
//! deletion is rejected; accepted, the text stays deleted. Each undoes in
//! one step.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};

const DEL: char = paged_model::TRACKED_DELETION_MARKER;

/// A story "Alpha beta INSERTED <deletion>delta." with "gamma " deleted.
fn model() -> (CanvasModel, String) {
    let idml = paged_gen::write_idml(&paged_gen::samples::notes::build()).expect("idml");
    let mut m = CanvasModel::load("doc", &idml, CanvasOptions::default()).expect("load");
    let story = m.scene().stories[0].self_id.clone();
    let s = &mut m.scene_mut().stories[0].story;
    s.paragraphs.truncate(1);
    let p = &mut s.paragraphs[0];
    p.runs.truncate(1);
    p.runs[0].text = format!("Alpha beta INSERTED {DEL}delta.");
    p.notes.clear();
    p.footnotes.clear();
    p.tracked_deletions = vec![paged_model::TrackedChange {
        change_type: "DeletedText".into(),
        user_name: Some("Reviewer".into()),
        deleted_text: Some("gamma ".into()),
        ..Default::default()
    }];
    (m, story)
}

fn text(m: &CanvasModel) -> String {
    m.scene().stories[0].story.paragraphs[0]
        .runs
        .iter()
        .map(|r| r.text.as_str())
        .collect()
}

#[test]
fn a_rejected_deletion_puts_its_text_back_and_undoes() {
    let (mut m, story) = model();
    let at = m.tracked_changes()[0].offset;
    assert_eq!(at, 20);
    m.apply_mutation(&Mutation::RejectTrackedChange {
        story_id: story.clone(),
        offset: at,
    })
    .expect("reject");
    assert_eq!(text(&m), "Alpha beta INSERTED gamma delta.");
    assert!(m.tracked_changes().is_empty());
    m.undo().expect("undo");
    assert_eq!(text(&m), format!("Alpha beta INSERTED {DEL}delta."));
    assert_eq!(m.tracked_changes()[0].text, "gamma ");
}

#[test]
fn an_accepted_deletion_stays_deleted_and_undoes() {
    let (mut m, story) = model();
    m.apply_mutation(&Mutation::AcceptTrackedChange {
        story_id: story.clone(),
        offset: 20,
    })
    .expect("accept");
    assert_eq!(text(&m), "Alpha beta INSERTED delta.");
    assert!(m.tracked_changes().is_empty());
    m.undo().expect("undo");
    assert_eq!(m.tracked_changes().len(), 1);
    assert_eq!(text(&m), format!("Alpha beta INSERTED {DEL}delta."));
}

#[test]
fn an_offset_with_no_deletion_is_refused() {
    let (mut m, story) = model();
    assert!(m
        .apply_mutation(&Mutation::AcceptTrackedChange {
            story_id: story,
            offset: 3,
        })
        .is_err());
}
