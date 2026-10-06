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

//! The native-producer constructors keep InDesign's defaults. A frame that
//! came back invisible (a derived `Default` would give `visible: false`) is
//! the trap `TextFrame::new` exists to avoid.

use paged_model::{ArrowheadType, Bounds, GraphicLine, Oval, Page, Polygon, Rectangle, TextFrame};

fn bounds() -> Bounds {
    Bounds {
        top: 72.0,
        left: 72.0,
        bottom: 720.0,
        right: 540.0,
    }
}

#[test]
fn a_new_text_frame_is_visible_and_plain() {
    let f = TextFrame::new("f1", Some("s1".into()), bounds());
    assert_eq!(f.self_id.as_deref(), Some("f1"));
    assert_eq!(f.parent_story.as_deref(), Some("s1"));
    assert!(f.visible, "a new frame must be visible");
    assert!(!f.locked && !f.nonprinting);
    assert!(f.fill_color.is_none() && f.stroke_color.is_none());
    assert!(f.first_baseline_offset.is_none() && f.inset_spacing.is_none());
    assert!(f.next_text_frame.is_none());
}

#[test]
fn a_new_page_has_no_master_and_shows_master_items() {
    let p = Page::new("p1", bounds());
    assert_eq!(p.self_id.as_deref(), Some("p1"));
    assert!(p.applied_master.is_none() && p.item_transform.is_none());
    assert!(p.override_list.is_empty());
    assert_ne!(p.show_master_items, Some(false));
}

/// The four non-text page items build the same way. What matters is the
/// handful of defaults a derived `Default` would get WRONG — a visible
/// item, an arrowhead scale of 100 % — and that a producer can name only
/// the fields it sets and take the rest from here, which is the shape
/// that keeps compiling when the model gains a field.
#[test]
fn the_other_page_items_are_visible_and_plain() {
    let r = Rectangle::new("r1", bounds());
    assert_eq!(r.self_id.as_deref(), Some("r1"));
    assert!(r.visible && !r.locked && !r.nonprinting);
    assert!(r.fill_color.is_none() && r.stroke_color.is_none() && r.end_cap.is_none());
    assert!(!r.has_image_element && r.image_link.is_none());

    let o = Oval::new("o1", bounds());
    assert_eq!(o.self_id.as_deref(), Some("o1"));
    assert!(o.visible && !o.locked && o.effects.is_none());

    let l = GraphicLine::new("l1", bounds());
    assert_eq!(l.self_id.as_deref(), Some("l1"));
    assert!(l.visible && l.anchors.is_empty());
    assert_eq!(
        (l.start_arrow, l.end_arrow),
        (ArrowheadType::None, ArrowheadType::None),
        "a new line carries no arrowheads"
    );
    assert_eq!(
        (l.start_arrow_scale, l.end_arrow_scale),
        (100.0, 100.0),
        "an arrowhead scale defaults to 100 %, not to 0"
    );

    let p = Polygon::new("p1", bounds());
    assert_eq!(p.self_id.as_deref(), Some("p1"));
    assert!(p.visible && p.anchors.is_empty() && p.subpath_starts.is_empty());
    assert!(p.effects.is_none() && p.opacity.is_none() && p.blend_mode.is_none());
}

#[test]
fn a_producer_names_what_it_sets_and_takes_the_rest() {
    // The functional-update form a reader in another repository uses.
    let p = Polygon {
        self_id: None,
        fill_color: Some("Color/Black".into()),
        stroke_weight: Some(2.0),
        ..Polygon::new("", bounds())
    };
    assert_eq!(p.self_id, None);
    assert_eq!(p.fill_color.as_deref(), Some("Color/Black"));
    assert_eq!(p.stroke_weight, Some(2.0));
    assert!(p.visible, "the rest still comes from the constructor");
    assert_eq!(p.bounds.right, 540.0);
}
