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
fn geometry_round_trips_through_parser() {
    let bytes = idml_gen::write_idml(&idml_gen::samples::geometry::build()).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), 5);
    assert_eq!(container.designmap.stories.len(), 5);
    assert_eq!(container.designmap.master_spreads.len(), 5);
    assert_eq!(
        container.mimetype,
        "application/vnd.adobe.indesign-idml-package",
    );
}
