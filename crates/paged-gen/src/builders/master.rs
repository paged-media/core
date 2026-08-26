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

//! Master-spread builder. One master per body page so a single
//! variant page can't pollute its neighbours via inherited master
//! items. Masters may carry their own page items (master frames /
//! rectangles) so body pages can inherit — and, via `OverrideList`,
//! suppress — them.

use crate::builders::page_item::PageItem;
use crate::geometry::IDENTITY;
use crate::xml::{format_f32, XmlBuilder};

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub struct Master {
    pub self_id: String,
    pub page_self_id: String,
    pub page_width_pt: f32,
    pub page_height_pt: f32,
    /// Master-spread page items (rectangles, frames, …). Stamped onto
    /// every body page that applies this master, unless suppressed by
    /// `ShowMasterItems="false"` or the body page's `OverrideList`.
    /// Empty ⇒ the classic empty master.
    pub page_items: Vec<PageItem>,
}

pub fn write_master(m: &Master) -> Vec<u8> {
    // The IDML component-name rule requires the file name's stem to
    // match `<Type>_<bare Self>` (spec §8.2). Real InDesign exports
    // strip any namespace-style prefix before emitting Self — so a
    // sample passing `"MasterSpread/u<hex>"` would produce a Self
    // that doesn't match the writer's filename. Strip here so the
    // call sites can keep their existing prefix conventions.
    let bare_self = strip_type_prefix(&m.self_id);
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:MasterSpread", &[PKG_NS, DOM_VERSION]);
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
        format_f32(m.page_height_pt),
        format_f32(m.page_width_pt),
    );
    b.start(
        "MasterSpread",
        &[
            ("Self", bare_self),
            ("Name", "$ID/None"),
            ("PageCount", "1"),
            // `ShowMasterItems` on the *master* spread is InDesign's
            // "let body pages display these master items" toggle. The
            // empty master left it `false`; with real master items we
            // want them inherited, so `true`. With no items this is a
            // harmless no-op for the existing empty-master call sites.
            ("ShowMasterItems", "true"),
            ("ItemTransform", &identity),
        ],
    );
    b.empty(
        "Page",
        &[
            ("Self", m.page_self_id.as_str()),
            ("Name", ""),
            ("AppliedMaster", "n"),
            ("ItemTransform", &identity),
            ("GeometricBounds", &bounds),
        ],
    );
    // Master page items live as direct children of the MasterSpread
    // element (same convention as body Spread items), not nested under
    // the Page. They carry their own `Self` ids so a body page's
    // `OverrideList` can name them for suppression.
    for item in &m.page_items {
        item.write(&mut b);
    }
    b.end("MasterSpread");
    b.end("idPkg:MasterSpread");
    b.into_bytes()
}

// ── Facing masters (verso + recto master pages in ONE spread) ───────
//
// Added ALONGSIDE the single-page `Master` so the ~30 existing
// `Master { .. }` literals stay untouched and every current sample's
// bytes stay identical. Same coordinate convention as
// `spread::FacingSpread`: spine at x = 0, verso master page mapped in
// via `ItemTransform="1 0 0 1 -W 0"`, recto identity, master items in
// SPREAD coords (verso furniture at negative x). The renderer routes
// each master item to a master page by centroid and stamps only the
// items belonging to the body page's same-ordinal master page — so a
// body verso (local page 0) inherits the verso furniture and a recto
// (local page 1) the recto furniture, independently.
//
// Unlike `Master`, this struct carries the InDesign `NamePrefix` /
// `BaseName` name split directly. `write_master` pins
// `Name="$ID/None"` because its callers' masters are anonymous
// throwaways, and `showcase_base` splices a name in after the fact
// (`write_named_master`) to avoid widening the shared struct; a NEW
// struct has no literals to protect, so the name is a first-class
// field here instead of a post-hoc rewrite.

/// A two-page (facing) master spread with independent per-side
/// furniture.
pub struct FacingMaster {
    pub self_id: String,
    /// InDesign's master-name split: the displayed `Name` is
    /// `"{name_prefix}-{base_name}"` (e.g. `B-Body`).
    pub name_prefix: String,
    pub base_name: String,
    pub verso_page_self_id: String,
    pub recto_page_self_id: String,
    pub page_width_pt: f32,
    pub page_height_pt: f32,
    /// Master items in SPREAD coords (verso furniture at negative x).
    /// Each carries its own `Self` id so a body page's `OverrideList`
    /// can suppress it per side.
    pub page_items: Vec<PageItem>,
}

pub fn write_facing_master(m: &FacingMaster) -> Vec<u8> {
    let bare_self = strip_type_prefix(&m.self_id);
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:MasterSpread", &[PKG_NS, DOM_VERSION]);
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
        format_f32(m.page_height_pt),
        format_f32(m.page_width_pt),
    );
    let name = format!("{}-{}", m.name_prefix, m.base_name);
    b.start(
        "MasterSpread",
        &[
            ("Self", bare_self),
            ("Name", &name),
            ("NamePrefix", &m.name_prefix),
            ("BaseName", &m.base_name),
            ("PageCount", "2"),
            ("ShowMasterItems", "true"),
            ("ItemTransform", &identity),
        ],
    );
    let verso_xform = format!("1 0 0 1 {} 0", format_f32(-m.page_width_pt));
    b.empty(
        "Page",
        &[
            ("Self", m.verso_page_self_id.as_str()),
            ("Name", ""),
            ("AppliedMaster", "n"),
            ("ItemTransform", &verso_xform),
            ("GeometricBounds", &bounds),
        ],
    );
    b.empty(
        "Page",
        &[
            ("Self", m.recto_page_self_id.as_str()),
            ("Name", ""),
            ("AppliedMaster", "n"),
            ("ItemTransform", &identity),
            ("GeometricBounds", &bounds),
        ],
    );
    for item in &m.page_items {
        item.write(&mut b);
    }
    b.end("MasterSpread");
    b.end("idPkg:MasterSpread");
    b.into_bytes()
}

/// Drop a leading `<Type>/` (e.g. `MasterSpread/u<hex>` →
/// `u<hex>`) so emitted `Self` attributes match the
/// `<Type>_<Self>.xml` filename convention.
fn strip_type_prefix(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}
