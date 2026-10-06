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

//! v71 — a CUSTOM text variable can be created and an instance of it
//! inserted into a story, so a data binding can drive a live value
//! through a definition InDesign keeps (a placeholder field is paged's
//! own and InDesign drops it). The value is then switched with one
//! `Set` on `textVariable:<id>` `textVariableContents`, and every
//! instance re-resolves from the definition.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_mutate::operation::FieldKind;
use paged_mutate::Value;
use paged_wire::Address;

fn model() -> CanvasModel {
    let bytes = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    CanvasModel::load("text-var", &bytes, CanvasOptions::default()).expect("load")
}

fn story_id(m: &CanvasModel) -> String {
    m.scene().stories[0].self_id.clone()
}

fn first_run_var(m: &CanvasModel, story: &str) -> (Option<String>, String) {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .unwrap();
    let r = &s.story.paragraphs[0].runs[0];
    (r.text_variable.clone(), r.text.clone())
}

const ID: &str = "dTextVariablenPrice";

#[test]
fn a_custom_text_variable_is_created_inserted_switched_and_undone() {
    let mut m = model();
    let vars_before = m.scene().designmap.text_variables.len();
    m.apply_mutation(&Mutation::CreateTextVariable {
        name: "Price".into(),
        contents: Some("9.99".into()),
        text_before: None,
        text_after: None,
    })
    .expect("create");
    let addr = Address::parse(&format!("textVariable:{ID}")).unwrap();
    assert_eq!(
        m.get_property_at(&addr, "textVariableContents").unwrap(),
        Value::Text("9.99".into())
    );
    assert_eq!(
        m.get_property_at(&addr, "textVariableType").unwrap(),
        Value::Text("CustomTextType".into())
    );
    assert_eq!(
        m.query("textVariable[name=\"Price\"]").unwrap(),
        vec![addr.clone()]
    );
    // A second variable of the same name is refused, not shadowed.
    assert!(m
        .apply_mutation(&Mutation::CreateTextVariable {
            name: "Price".into(),
            contents: None,
            text_before: None,
            text_after: None,
        })
        .is_err());

    let story = story_id(&m);
    m.apply_mutation(&Mutation::InsertField {
        story_id: story.clone(),
        offset: 0,
        field: FieldKind::TextVariable {
            variable_id: ID.into(),
        },
        content_offset: None,
    })
    .expect("insert instance");
    assert_eq!(
        first_run_var(&m, &story),
        (Some(ID.to_string()), "9.99".to_string())
    );

    // The live switch: one Set on the definition.
    m.apply_mutation(&Mutation::Set {
        address: addr.clone(),
        path: "textVariableContents".into(),
        value: Value::Text("12.50".into()),
    })
    .expect("switch");
    assert_eq!(
        m.get_property_at(&addr, "textVariableContents").unwrap(),
        Value::Text("12.50".into())
    );
    // The instance's baked ResultText follows, so an IDML save carries
    // the current value too.
    assert_eq!(first_run_var(&m, &story).1, "12.50");

    // Undo: the switch, the instance, the definition — one step each.
    m.undo().expect("undo switch");
    assert_eq!(first_run_var(&m, &story).1, "9.99");
    m.undo().expect("undo insert");
    assert_ne!(first_run_var(&m, &story).0.as_deref(), Some(ID));
    m.undo().expect("undo create");
    assert_eq!(m.scene().designmap.text_variables.len(), vars_before);
    m.redo().expect("redo create");
    assert_eq!(m.scene().designmap.text_variables.len(), vars_before + 1);
}

#[test]
fn an_instance_of_an_unknown_variable_is_refused() {
    let mut m = model();
    let story = story_id(&m);
    assert!(m
        .apply_mutation(&Mutation::InsertField {
            story_id: story,
            offset: 0,
            field: FieldKind::TextVariable {
                variable_id: "dTextVariablenNope".into(),
            },
            content_offset: None,
        })
        .is_err());
}

#[test]
fn a_variable_in_use_is_not_deleted() {
    let mut m = model();
    m.apply_mutation(&Mutation::CreateTextVariable {
        name: "Price".into(),
        contents: Some("1".into()),
        text_before: None,
        text_after: None,
    })
    .unwrap();
    let story = story_id(&m);
    m.apply_mutation(&Mutation::InsertField {
        story_id: story,
        offset: 0,
        field: FieldKind::TextVariable {
            variable_id: ID.into(),
        },
        content_offset: None,
    })
    .unwrap();
    assert!(m
        .apply_mutation(&Mutation::DeleteTextVariable {
            variable_id: ID.into()
        })
        .is_err());
}
