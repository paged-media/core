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

//! What an InDesign user sees after a paged save: state the object model
//! writes must reach the IDML XML itself, not only a container part that
//! InDesign drops on its own save.
//!
//! - a document label (`setDocumentMetadata`) is a
//!   `<Document><Properties><Label><KeyValuePair>` in `designmap.xml`;
//! - a hidden page item (`element.visible = false`) is `Visible="false"`
//!   on its element in the spread.

use std::io::Read;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};

const KEY: &str = "x-paged:media.paged.data";
const VALUE: &str = r#"{"v":1,"data":{"parts":"a1"}}"#;

fn model() -> CanvasModel {
    let bytes = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    CanvasModel::load("om-export", &bytes, CanvasOptions::default()).expect("load")
}

fn entries(zip_bytes: &[u8], prefix: &str) -> Vec<(String, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).expect("zip");
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        if f.name().starts_with(prefix) {
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            out.push((f.name().to_string(), s));
        }
    }
    out
}

fn designmap(zip_bytes: &[u8]) -> String {
    entries(zip_bytes, "designmap.xml")
        .pop()
        .expect("designmap.xml")
        .1
}

#[test]
fn a_document_label_is_in_the_exported_designmap() {
    let mut m = model();
    m.apply_mutation(&Mutation::SetDocumentMetadata {
        key: KEY.into(),
        value: Some(VALUE.into()),
        caller: None,
    })
    .expect("set");
    for (what, bytes) in [
        ("idml", m.export_idml().expect("export idml")),
        (
            "paged",
            m.export_paged(paged_canvas::channel::PROTOCOL_VERSION.0)
                .expect("export paged"),
        ),
    ] {
        let dm = designmap(&bytes);
        assert!(
            dm.contains(KEY),
            "{what}: the document label is not in designmap.xml:\n{dm}"
        );
    }
}

/// KNOWN DEFECT, pinned (protocol 71): the IDML adapter (plugin-publish
/// `idml-export`, `rewrite.rs` page-item attribute patcher) has no
/// `Visible` arm, so the source attribute passes through and a hidden
/// item comes back visible in InDesign. The fix lives in plugin-publish
/// (thread `CommonAttrs.visible`/`locked` into the per-kind patchers as
/// `preserving_bool_patch(raw, visible, true)` and emit them on minted
/// items); when core re-pins onto it, this assertion goes red — flip it
/// to `Visible="false"` then.
#[test]
fn a_hidden_item_is_visible_false_in_the_exported_spread() {
    let mut m = model();
    let id = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find_map(|f| f.self_id.clone())
        .expect("a text frame");
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: paged_wire::ElementId::TextFrame(id.clone()),
        path: paged_mutate::PropertyPath::ElementVisible,
        value: paged_mutate::Value::Bool(false),
    })
    .expect("hide");
    let bytes = m.export_idml().expect("export idml");
    let spreads = entries(&bytes, "Spreads/");
    let (name, xml) = spreads
        .iter()
        .find(|(_, x)| x.contains(&format!("Self=\"{id}\"")))
        .expect("the frame's spread");
    let at = xml.find(&format!("Self=\"{id}\"")).unwrap();
    let open = xml[..at].rfind('<').unwrap();
    let tag_end = at + xml[at..].find('>').unwrap();
    let tag = &xml[open..=tag_end];
    assert!(
        tag.contains("Visible=\"true\""),
        "{name}: the adapter now writes a hidden frame's Visible — the pinned \
         defect is fixed upstream; assert Visible=\"false\" here instead: {tag}"
    );
}

/// KNOWN DEFECT, pinned (protocol 71): a text variable authored in
/// paged (`createTextVariable` + an `InsertField` text-variable instance)
/// lives in the model, but the IDML adapter (plugin-publish
/// `idml-export`) writes neither the `<TextVariable>` definition into
/// `designmap.xml` nor the `<TextVariableInstance AssociatedTextVariable
/// ResultText>` into the story — it only patches definitions and
/// instances the source package already had. When core re-pins onto the
/// adapter fix this goes red: assert both are written then.
#[test]
fn an_authored_text_variable_reaches_the_idml() {
    let mut m = model();
    m.apply_mutation(&Mutation::CreateTextVariable {
        name: "Price".into(),
        contents: Some("9.99".into()),
        text_before: None,
        text_after: None,
    })
    .expect("create");
    let story = m.scene().stories[0].self_id.clone();
    m.apply_mutation(&Mutation::InsertField {
        story_id: story,
        offset: 0,
        field: paged_mutate::operation::FieldKind::TextVariable {
            variable_id: "dTextVariablenPrice".into(),
        },
        content_offset: None,
    })
    .expect("insert");
    let bytes = m.export_idml().expect("export");
    let dm = designmap(&bytes);
    let stories = entries(&bytes, "Stories/");
    let definition = dm.contains("dTextVariablenPrice");
    let instance = stories
        .iter()
        .any(|(_, x)| x.contains("AssociatedTextVariable=\"dTextVariablenPrice\""));
    eprintln!("definition written: {definition}; instance written: {instance}");
    assert!(
        !definition && !instance,
        "the adapter now writes authored text variables (definition {definition}, instance \
         {instance}) — the pinned defect is fixed upstream; assert both are written"
    );
}
