//! Empty master-spread builder. One master per body page so a single
//! variant page can't pollute its neighbours via inherited master
//! items.

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
}

pub fn write_master(m: &Master) -> Vec<u8> {
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
            ("Self", m.self_id.as_str()),
            ("Name", "$ID/None"),
            ("PageCount", "1"),
            ("ShowMasterItems", "false"),
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
    b.end("MasterSpread");
    b.end("idPkg:MasterSpread");
    b.into_bytes()
}
