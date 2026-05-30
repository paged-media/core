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
    b.empty(
        "Page",
        &[
            ("Self", s.page_self_id.as_str()),
            ("Name", s.page_name.as_str()),
            ("AppliedMaster", applied_master),
            ("ItemTransform", &identity),
            ("GeometricBounds", &bounds),
            ("MasterPageTransform", &identity),
        ],
    );
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
