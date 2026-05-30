//! Tiny XML emitter on top of `quick_xml::Writer`.
//!
//! The generator wants two things `quick_xml` doesn't enforce:
//!
//! 1. **Stable attribute order.** XML is unordered semantically, but
//!    diffing two emitted IDMLs across runs becomes hopeless if the
//!    attributes shuffle. Builders supply `&[(name, value)]` pairs;
//!    we emit them in the order given. Builders take responsibility
//!    for ordering attributes consistently.
//! 2. **Fixed-precision floats.** InDesign serialises `f32`s to a few
//!    decimals; `format_f32` rounds to 4 (matching real exports).
//!    Trailing zeros are trimmed so `1.0` stays `1` — also matching
//!    real exports — and `-0` is normalised to `0`.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use std::io::Cursor;

/// Round `v` to 4 decimals and drop trailing zeros / decimal point.
/// `-0` collapses to `0` so two equivalent matrices format identically.
pub fn format_f32(v: f32) -> String {
    let rounded = (v * 10_000.0).round() / 10_000.0;
    if rounded == 0.0 {
        return "0".to_string();
    }
    // 4 decimals max; trim trailing zeros and a dangling '.'
    let mut s = format!("{rounded:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// Helper struct that wraps a `Writer<Cursor<Vec<u8>>>` and exposes
/// a small façade matching how the IDML builders want to emit XML.
pub struct XmlBuilder {
    writer: Writer<Cursor<Vec<u8>>>,
}

impl Default for XmlBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl XmlBuilder {
    pub fn new() -> Self {
        Self {
            writer: Writer::new(Cursor::new(Vec::new())),
        }
    }

    /// Emit `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>`
    /// — the declaration every IDML XML file carries.
    pub fn write_decl(&mut self) {
        self.writer
            .write_event(Event::Decl(BytesDecl::new(
                "1.0",
                Some("UTF-8"),
                Some("yes"),
            )))
            .expect("write decl");
    }

    /// Emit a processing instruction `<?target content?>`. Used by
    /// designmap.xml to carry the `<?aid?>` PI that real InDesign
    /// readers gate IDML import on (without it, the file is rejected
    /// as "format not supported"). Content is emitted verbatim — no
    /// escaping; callers are responsible for keeping the body XML-PI
    /// safe.
    pub fn write_pi(&mut self, target: &str, content: &str) {
        let body = format!("{} {}", target, content);
        self.writer
            .write_event(Event::PI(quick_xml::events::BytesPI::new(body)))
            .expect("write pi");
    }

    /// Emit `<name attr="value" ...>` (open tag only). Attributes are
    /// emitted in the slice's order. Caller closes via `end`.
    pub fn start(&mut self, name: &str, attrs: &[(&str, &str)]) {
        let mut elem = BytesStart::new(name);
        for (k, v) in attrs {
            elem.push_attribute((*k, *v));
        }
        self.writer.write_event(Event::Start(elem)).expect("start");
    }

    /// Emit `<name attr="value" .../>` (self-closing).
    pub fn empty(&mut self, name: &str, attrs: &[(&str, &str)]) {
        let mut elem = BytesStart::new(name);
        for (k, v) in attrs {
            elem.push_attribute((*k, *v));
        }
        self.writer.write_event(Event::Empty(elem)).expect("empty");
    }

    /// Emit `</name>`.
    pub fn end(&mut self, name: &str) {
        self.writer
            .write_event(Event::End(BytesEnd::new(name)))
            .expect("end");
    }

    /// Emit raw text content (escaped automatically).
    pub fn text(&mut self, s: &str) {
        self.writer
            .write_event(Event::Text(BytesText::new(s)))
            .expect("text");
    }

    /// Consume the writer and return the assembled bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.writer.into_inner().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_format_drops_trailing_zeros() {
        assert_eq!(format_f32(1.0), "1");
        assert_eq!(format_f32(1.5), "1.5");
        assert_eq!(format_f32(1.234_56), "1.2346");
        assert_eq!(format_f32(0.0), "0");
        assert_eq!(format_f32(-0.000_01), "0");
    }

    #[test]
    fn xml_builder_round_trips() {
        let mut b = XmlBuilder::new();
        b.write_decl();
        b.start("Doc", &[("Self", "u1")]);
        b.empty("Item", &[("X", "1")]);
        b.end("Doc");
        let s = String::from_utf8(b.into_bytes()).unwrap();
        assert!(s.contains("<Doc Self=\"u1\">"));
        assert!(s.contains("<Item X=\"1\"/>"));
        assert!(s.contains("</Doc>"));
    }
}
