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

//! Spread + Page builder. A `Spread` wraps one body page (Phase 0 is
//! single-page-per-spread). Page items live as direct children of the
//! `Spread` element, not the `Page` — that's IDML's convention.

use crate::builders::page_item::PageItem;
use crate::geometry::IDENTITY;
use crate::xml::{format_f32, XmlBuilder};

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub struct Spread {
    pub self_id: String,
    pub page_self_id: String,
    pub page_name: String,
    pub applied_master: String,
    pub page_width_pt: f32,
    pub page_height_pt: f32,
    pub page_items: Vec<PageItem>,
    /// Master-spread item `Self` ids this body page has overridden.
    /// Emitted as the `<Page OverrideList="…">` attribute (space-
    /// separated). The renderer skips stamping any master item whose
    /// id appears here, so the body's replacement frame isn't double-
    /// painted under the master placeholder. Empty ⇒ no attribute.
    pub override_list: Vec<String>,
    /// Optional `<MarginPreference>` child of the `<Page>` element
    /// (top, bottom, left, right in pt). `None` ⇒ no margins emitted
    /// (the page's content area is the full page rectangle). Parsed
    /// into `Spread::page_margins` keyed by the page's `Self` id.
    pub margins: Option<MarginPreference>,
}

/// `<MarginPreference>` payload: per-page margins in points. Defaults to
/// a single column with no gutter (the common case for the fixtures).
#[derive(Clone, Copy)]
pub struct MarginPreference {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

pub fn write_spread(s: &Spread) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Spread", &[PKG_NS, DOM_VERSION]);

    let identity = format!(
        "{} {} {} {} {} {}",
        format_f32(IDENTITY[0]),
        format_f32(IDENTITY[1]),
        format_f32(IDENTITY[2]),
        format_f32(IDENTITY[3]),
        format_f32(IDENTITY[4]),
        format_f32(IDENTITY[5]),
    );
    let bounds = format!(
        "0 0 {} {}",
        format_f32(s.page_height_pt),
        format_f32(s.page_width_pt),
    );
    b.start(
        "Spread",
        &[
            ("Self", s.self_id.as_str()),
            ("PageCount", "1"),
            ("BindingLocation", "0"),
            ("ShowMasterItems", "true"),
            ("AllowPageShuffle", "true"),
            ("ItemTransform", &identity),
        ],
    );
    // AppliedMaster must reference the bare `<MasterSpread Self="...">`
    // id, not the `MasterSpread/<id>` filename prefix the call sites
    // tend to compose. Real InDesign exports use bare ids (e.g.
    // `AppliedMaster="ub4"`) — match that.
    let applied_master = strip_type_prefix(&s.applied_master);
    let override_list = s.override_list.join(" ");
    let mut page_attrs: Vec<(&str, &str)> = vec![
        ("Self", s.page_self_id.as_str()),
        ("Name", s.page_name.as_str()),
        ("AppliedMaster", applied_master),
        ("ItemTransform", &identity),
        ("GeometricBounds", &bounds),
        ("MasterPageTransform", &identity),
    ];
    if !override_list.is_empty() {
        page_attrs.push(("OverrideList", override_list.as_str()));
    }
    // `<MarginPreference>` is a child of `<Page>` when present; the
    // parser keys it to the most-recently-pushed page, so emit it inside
    // the `<Page>` element (or, equivalently, right after — we nest it).
    if let Some(m) = s.margins {
        b.start("Page", &page_attrs);
        let (top, bottom, left, right) = (
            format_f32(m.top),
            format_f32(m.bottom),
            format_f32(m.left),
            format_f32(m.right),
        );
        b.empty(
            "MarginPreference",
            &[
                ("ColumnCount", "1"),
                ("ColumnGutter", "0"),
                ("Top", &top),
                ("Bottom", &bottom),
                ("Left", &left),
                ("Right", &right),
            ],
        );
        b.end("Page");
    } else {
        b.empty("Page", &page_attrs);
    }
    for item in &s.page_items {
        item.write(&mut b);
    }
    b.end("Spread");
    b.end("idPkg:Spread");
    b.into_bytes()
}

fn strip_type_prefix(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}
