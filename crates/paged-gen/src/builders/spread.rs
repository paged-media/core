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
    /// Optional `ItemTransform` on the `<Spread>` element (W1.9). `None`
    /// ⇒ the identity matrix (the legacy default — byte-identical to
    /// pre-W1.9 output). A rotation/scale here propagates onto every
    /// body item's emitted display-list transform, so a generated
    /// fixture can exercise the spread-rotation path the hand-built
    /// `spread_transform` test covers.
    pub item_transform: Option<[f32; 6]>,
}

/// `<MarginPreference>` payload: per-page margins in points + the
/// column grid that subdivides the resulting content area. Defaults
/// (via [`MarginPreference::symmetric`]) to a single column with no
/// gutter (the common case for the fixtures).
#[derive(Clone, Copy)]
pub struct MarginPreference {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
    /// `ColumnCount` — number of columns the margin box is divided
    /// into. 1 ⇒ a single column.
    pub column_count: u32,
    /// `ColumnGutter` — gutter width (pt) between adjacent columns.
    pub column_gutter: f32,
}

impl MarginPreference {
    /// A single-column margin box (no gutter) — the historical default
    /// that earlier fixtures relied on before columns were a knob.
    pub fn symmetric(top: f32, bottom: f32, left: f32, right: f32) -> Self {
        Self {
            top,
            bottom,
            left,
            right,
            column_count: 1,
            column_gutter: 0.0,
        }
    }
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
    // The `<Spread>` element's ItemTransform (W1.9). The `<Page>` /
    // MasterPageTransform stay identity — the spread transform alone
    // maps the body items into the rendered page, matching how
    // InDesign serialises a rotated/scaled spread.
    let spread_xform = s
        .item_transform
        .map(|m| {
            format!(
                "{} {} {} {} {} {}",
                format_f32(m[0]),
                format_f32(m[1]),
                format_f32(m[2]),
                format_f32(m[3]),
                format_f32(m[4]),
                format_f32(m[5]),
            )
        })
        .unwrap_or_else(|| identity.clone());
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
            ("ItemTransform", &spread_xform),
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
        let column_count = m.column_count.to_string();
        let column_gutter = format_f32(m.column_gutter);
        b.empty(
            "MarginPreference",
            &[
                ("ColumnCount", &column_count),
                ("ColumnGutter", &column_gutter),
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

// ── Facing spreads (verso + recto in ONE `<Spread>`) ────────────────
//
// Added ALONGSIDE the single-page `Spread` on purpose: every existing
// sample's emitted bytes stay identical, and the ~35 `Spread { .. }`
// literals never learn about a second page. A facing spread is IDML's
// native "reader spread" shape: `PageCount="2"`, two `<Page>` children
// (verso first, then recto), spread-local coordinates with the spine at
// x = 0 — the verso page maps in via `ItemTransform="1 0 0 1 -W 0"`,
// the recto via identity, and each page's `GeometricBounds` stays
// `0 0 H W` (bounds are page-INNER coords; the ItemTransform places
// them in the spread, spec §10.3.3). Page items are spread children
// positioned in SPREAD coords, so verso items live at negative x; the
// renderer routes each item to the page containing its centroid.

/// One side of a [`FacingSpread`]: the per-page knobs the single-page
/// [`Spread`] carries on itself (name, master, overrides, margins).
pub struct FacingPage {
    pub self_id: String,
    pub name: String,
    /// `AppliedMaster` ref (`MasterSpread/<id>` or bare — the type
    /// prefix is stripped, matching [`Spread::applied_master`]).
    pub applied_master: String,
    /// Master-item `Self` ids THIS side has overridden (per-page
    /// `OverrideList`, same semantics as [`Spread::override_list`]).
    pub override_list: Vec<String>,
    /// Per-page `<MarginPreference>` — the seat of mirrored
    /// inside/outside margins (verso and recto swap left/right).
    pub margins: Option<MarginPreference>,
}

/// A two-page reader spread: verso (left, even folio) + recto (right,
/// odd folio) sharing one spread coordinate system.
pub struct FacingSpread {
    pub self_id: String,
    pub page_width_pt: f32,
    pub page_height_pt: f32,
    /// Emitted first — IDML lists a reader spread's pages left to
    /// right, so the verso is `pages[0]` (`local_page_idx` 0 in the
    /// renderer, which is also what routes it to a facing master's
    /// FIRST master page).
    pub verso: FacingPage,
    pub recto: FacingPage,
    /// Page items in SPREAD coords (spine at x = 0; verso items at
    /// negative x). Routed to a page by centroid containment.
    pub page_items: Vec<PageItem>,
}

pub fn write_facing_spread(s: &FacingSpread) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Spread", &[PKG_NS, DOM_VERSION]);

    let identity = format_matrix_str(IDENTITY);
    b.start(
        "Spread",
        &[
            ("Self", s.self_id.as_str()),
            ("PageCount", "2"),
            // Interior reader spreads bind at location 1 (the spine
            // sits between the two pages); location 0 is the
            // single-page "everything right of the spine" shape.
            ("BindingLocation", "1"),
            ("ShowMasterItems", "true"),
            ("AllowPageShuffle", "true"),
            ("ItemTransform", &identity),
        ],
    );

    let verso_xform = format!("1 0 0 1 {} 0", crate::xml::format_f32(-s.page_width_pt));
    write_facing_page(&mut b, &s.verso, s, &verso_xform, &identity);
    write_facing_page(&mut b, &s.recto, s, &identity, &identity);

    for item in &s.page_items {
        item.write(&mut b);
    }
    b.end("Spread");
    b.end("idPkg:Spread");
    b.into_bytes()
}

fn write_facing_page(
    b: &mut XmlBuilder,
    p: &FacingPage,
    s: &FacingSpread,
    item_transform: &str,
    identity: &str,
) {
    let bounds = format!(
        "0 0 {} {}",
        format_f32(s.page_height_pt),
        format_f32(s.page_width_pt),
    );
    let applied_master = strip_type_prefix(&p.applied_master);
    let override_list = p.override_list.join(" ");
    let mut attrs: Vec<(&str, &str)> = vec![
        ("Self", p.self_id.as_str()),
        ("Name", p.name.as_str()),
        ("AppliedMaster", applied_master),
        ("ItemTransform", item_transform),
        ("GeometricBounds", &bounds),
        ("MasterPageTransform", identity),
    ];
    if !override_list.is_empty() {
        attrs.push(("OverrideList", override_list.as_str()));
    }
    if let Some(m) = p.margins {
        b.start("Page", &attrs);
        let (top, bottom, left, right) = (
            format_f32(m.top),
            format_f32(m.bottom),
            format_f32(m.left),
            format_f32(m.right),
        );
        let column_count = m.column_count.to_string();
        let column_gutter = format_f32(m.column_gutter);
        b.empty(
            "MarginPreference",
            &[
                ("ColumnCount", &column_count),
                ("ColumnGutter", &column_gutter),
                ("Top", &top),
                ("Bottom", &bottom),
                ("Left", &left),
                ("Right", &right),
            ],
        );
        b.end("Page");
    } else {
        b.empty("Page", &attrs);
    }
}

fn format_matrix_str(m: [f32; 6]) -> String {
    format!(
        "{} {} {} {} {} {}",
        format_f32(m[0]),
        format_f32(m[1]),
        format_f32(m[2]),
        format_f32(m[3]),
        format_f32(m[4]),
        format_f32(m[5]),
    )
}

fn strip_type_prefix(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}
