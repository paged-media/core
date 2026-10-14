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

//! The text wrap and anchored-object options an item takes from its object
//! style (ADR 143).

use paged_model::{
    AnchoredObjectSetting, AnchoredStated, ObjectStyleCategories, ObjectStyleDef, StyleSheet,
    TextWrap, TextWrapMode,
};

fn style_anchoring() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: Some("Anchored".into()),
        anchor_point: Some("TopRightAnchor".into()),
        anchor_y_offset: 6.0,
        horizontal_reference_point: Some("TextFrame".into()),
        horizontal_alignment: Some("RightAlign".into()),
        ..Default::default()
    }
}

fn sheet(enable_wrap: Option<bool>, enable_anchor: Option<bool>) -> StyleSheet {
    let mut sheet = StyleSheet::default();
    sheet.object_styles.insert(
        "ObjectStyle/S".into(),
        ObjectStyleDef {
            self_id: "ObjectStyle/S".into(),
            enable: ObjectStyleCategories {
                text_wrap_and_others: enable_wrap,
                anchored_object_options: enable_anchor,
                ..Default::default()
            },
            text_wrap: Some(TextWrap {
                mode: TextWrapMode::BoundingBoxTextWrap,
                offsets: [4.0, 4.0, 4.0, 12.0],
                ..TextWrap::NONE
            }),
            anchored: Some(style_anchoring()),
            ..Default::default()
        },
    );
    sheet
}

#[test]
fn an_item_that_states_nothing_takes_its_styles_options_with_the_category_on() {
    let on = sheet(Some(true), Some(true));
    let wrap = on
        .item_text_wrap(None, Some("ObjectStyle/S"))
        .expect("the style's wrap");
    assert_eq!(wrap.offsets, [4.0, 4.0, 4.0, 12.0]);
    let anchored = on
        .item_anchored(None, Some("ObjectStyle/S"))
        .expect("the style's anchoring");
    assert_eq!(anchored.anchor_y_offset, 6.0);
    // Off, or absent: nothing.
    for flag in [Some(false), None] {
        let off = sheet(flag, flag);
        assert!(off.item_text_wrap(None, Some("ObjectStyle/S")).is_none());
        assert!(off.item_anchored(None, Some("ObjectStyle/S")).is_none());
    }
}

#[test]
fn an_item_overrides_its_style_attribute_by_attribute() {
    // InDesign writes `<AnchoredObjectSetting AnchorXoffset="-20"/>`: the
    // item keeps the style's position, point, alignment and y offset.
    let own = AnchoredObjectSetting {
        anchor_x_offset: -20.0,
        stated: Some(AnchoredStated {
            anchor_x_offset: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let got = sheet(Some(true), Some(true))
        .item_anchored(Some(&own), Some("ObjectStyle/S"))
        .unwrap();
    assert_eq!(got.anchor_x_offset, -20.0);
    assert_eq!(got.anchor_y_offset, 6.0);
    assert_eq!(got.anchored_position.as_deref(), Some("Anchored"));
    assert_eq!(got.anchor_point.as_deref(), Some("TopRightAnchor"));
    // A setting with no record of what it stated is all its own.
    let legacy = AnchoredObjectSetting {
        anchor_x_offset: -20.0,
        ..Default::default()
    };
    let got = sheet(Some(true), Some(true))
        .item_anchored(Some(&legacy), Some("ObjectStyle/S"))
        .unwrap();
    assert_eq!(got.anchor_y_offset, 0.0);
    // An item's own wrap wins whole.
    let none = TextWrap::NONE;
    let wrap = sheet(Some(true), Some(true))
        .item_text_wrap(Some(none), Some("ObjectStyle/S"))
        .unwrap();
    assert_eq!(wrap.mode, TextWrapMode::None);
}
