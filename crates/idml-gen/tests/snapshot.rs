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
fn text_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::text::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::text::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn text_round_trips_through_parser() {
    let sample = idml_gen::samples::text::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    assert_eq!(container.designmap.stories.len(), sample.stories.len());
}

#[test]
fn text_advanced_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::text_advanced::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::text_advanced::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn text_advanced_round_trips_through_parser() {
    let sample = idml_gen::samples::text_advanced::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    assert_eq!(container.designmap.stories.len(), sample.stories.len());
}

#[test]
fn effects_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::effects::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::effects::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn effects_round_trips_through_parser() {
    let sample = idml_gen::samples::effects::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    assert_eq!(container.designmap.stories.len(), sample.stories.len());
}

#[test]
fn gradients_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::gradients::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::gradients::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn tables_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::tables::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::tables::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn tables_round_trips_through_parser() {
    let sample = idml_gen::samples::tables::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    // Every body story must parse a Table out of its host paragraph.
    let mut tables_found = 0;
    for entry_path in container.entries.keys() {
        if !entry_path.starts_with("Stories/") {
            continue;
        }
        let xml = &container.entries[entry_path];
        let story = idml_parse::Story::parse(xml).expect("Story::parse");
        for p in &story.paragraphs {
            if p.table.is_some() {
                tables_found += 1;
            }
        }
    }
    assert_eq!(tables_found, sample.spreads.len());
}

#[test]
fn gradients_round_trips_through_parser() {
    let sample = idml_gen::samples::gradients::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    // The Graphic.xml must register all five gradient swatches.
    let graphic_xml = container
        .entries
        .get("Resources/Graphic.xml")
        .expect("Resources/Graphic.xml must be present");
    let graphic = idml_parse::Graphic::parse(graphic_xml).expect("Graphic::parse");
    assert_eq!(graphic.gradients.len(), 5);
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

#[test]
fn geometry_groups_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::geometry_groups::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::geometry_groups::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn geometry_groups_round_trips_through_parser() {
    let sample = idml_gen::samples::geometry_groups::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
}

#[test]
fn transparency_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::transparency::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::transparency::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn transparency_round_trips_through_parser() {
    let sample = idml_gen::samples::transparency::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    assert_eq!(container.designmap.stories.len(), sample.stories.len());
    // Every page must round-trip its TransparencySetting payload —
    // either a BlendingSetting (Opacity/BlendMode) or a
    // DropShadowSetting must surface on at least one rectangle per
    // spread, since that's the variant the page is testing.
    let mut blends = 0;
    let mut shadows = 0;
    for entry_path in container.entries.keys() {
        if !entry_path.starts_with("Spreads/") {
            continue;
        }
        let xml = &container.entries[entry_path];
        let spread = idml_parse::Spread::parse(xml).expect("Spread::parse");
        for r in &spread.rectangles {
            if r.drop_shadow.is_some() {
                shadows += 1;
            }
        }
        // BlendingSetting comes back via the parser's transparency
        // hooks — surface it via the raw XML so the assertion is
        // independent of which struct field exposes it. `entries`
        // stores raw bytes; needle is a constant ASCII slice.
        let needle = b"<BlendingSetting";
        if xml.windows(needle.len()).any(|w| w == needle) {
            blends += 1;
        }
    }
    // 9 of the 12 variants set a BlendingSetting (every variant
    // except the two pure drop-shadow pages and… wait, the pure
    // drop-shadow pages omit blending, so 12 - 2 = 10). Re-counting
    // against the variant table: 3 opacity + 5 blend + 2 combos =
    // 10 pages with `<BlendingSetting>`.
    assert_eq!(blends, 10, "expected 10 BlendingSetting blocks, got {blends}");
    // 3 variants set a DropShadow (default, explicit, shadow+opacity
    // combo).
    assert_eq!(shadows, 3, "expected 3 DropShadow blocks, got {shadows}");
}

#[test]
fn images_emit_is_byte_deterministic() {
    let a = idml_gen::write_idml(&idml_gen::samples::images::build()).unwrap();
    let b = idml_gen::write_idml(&idml_gen::samples::images::build()).unwrap();
    assert_eq!(sha256(&a), sha256(&b));
}

#[test]
fn images_round_trips_through_parser() {
    let sample = idml_gen::samples::images::build();
    let bytes = idml_gen::write_idml(&sample).unwrap();
    let container = idml_parse::Container::open(&bytes).expect("Container::open");
    assert_eq!(container.designmap.spreads.len(), sample.spreads.len());
    // Every spread's Rectangle must surface a placed-image link via
    // the parser, and every spread's FrameFittingOption must round-
    // trip with its FittingOnEmptyFrame string. Belt-and-braces:
    // this also guards against a regression where the writer drops
    // the nested `<Image>` element.
    let mut images_found = 0;
    let mut fittings_found = 0;
    for entry_path in container.entries.keys() {
        if !entry_path.starts_with("Spreads/") {
            continue;
        }
        let xml = &container.entries[entry_path];
        let spread = idml_parse::Spread::parse(xml).expect("Spread::parse");
        for r in &spread.rectangles {
            if r.image_link.is_some() {
                images_found += 1;
            }
            if r.frame_fitting.is_some() {
                fittings_found += 1;
            }
        }
    }
    assert_eq!(
        images_found,
        sample.spreads.len(),
        "every spread must surface a placed image link",
    );
    assert_eq!(
        fittings_found,
        sample.spreads.len(),
        "every spread must round-trip its FrameFittingOption",
    );
}
