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

use paged_model::{Bounds, Page, TextFrame};

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
