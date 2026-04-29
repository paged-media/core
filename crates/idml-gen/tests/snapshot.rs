//! Determinism + structural-correctness gates for `idml-gen`.
//!
//! Two emissions of the same sample must produce a byte-identical
//! archive — the test above hashes both and asserts equality. The
//! second test confirms our own parser accepts what we wrote (5
//! spreads, 5 stories, 5 master spreads for `geometry.idml`).

use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut s = String::with_capacity(64);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[test]
fn geometry_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::geometry::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::geometry::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn geometry_zip_shape_mimetype_first() {
    let bytes = idml_gen::write_idml(&idml_gen::samples::geometry::build()).unwrap();
    // The local file header starts at offset 0; the file name follows
    // the 30-byte fixed header. Verify "mimetype" lands at offset 30,
    // method = Stored (compression flag at offset 8 = 0).
    assert_eq!(&bytes[..4], b"PK\x03\x04", "ZIP local header magic");
    assert_eq!(
        u16::from_le_bytes([bytes[8], bytes[9]]),
        0,
        "mimetype must be Stored (compression method 0)",
    );
    assert_eq!(&bytes[30..38], b"mimetype", "first entry filename");
}

#[test]
fn strokes_fills_round_trips_through_parser() {
    let sample = idml_gen::samples::strokes_fills::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(
        container.designmap.spreads.len(),
        sample.spreads.len(),
        "manifest spread count must match",
    );
}

#[test]
fn strokes_fills_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::strokes_fills::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::strokes_fills::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn geometry_round_trips_through_parser() {
    let sample = idml_gen::samples::geometry::build();
    let expected = sample.spreads.len();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), expected);
    assert_eq!(container.designmap.stories.len(), expected);
    assert_eq!(container.designmap.master_spreads.len(), expected);
    assert_eq!(
        container.mimetype,
        "application/vnd.adobe.indesign-idml-package",
    );
    // Sanity: every variant page is named uniquely. A duplicate would
    // mean the variant list accidentally shadowed an entry.
    let mut names: Vec<String> = Vec::new();
    for (_id, body) in &sample.spreads {
        let xml = std::str::from_utf8(body).unwrap();
        let after = xml.split("Page Self=\"").nth(1).unwrap();
        let after_name = after.split("Name=\"").nth(1).unwrap();
        let name = after_name.split('"').next().unwrap().to_string();
        names.push(name);
    }
    let unique: std::collections::HashSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "duplicate page names: {names:?}");
}
