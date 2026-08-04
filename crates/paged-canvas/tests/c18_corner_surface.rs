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

//! C-18 (corner attributes on every page-item kind) + E-1 (the property
//! descriptor was rectangle-only for corners).
//!
//! Three things are pinned here:
//!
//!   1. every kind PARSES the corner vocabulary off its element;
//!   2. every kind exposes the eight corner rows on READ and accepts the
//!      matching WRITE — the C-17 lesson, pinned as a pair rather than
//!      trusted;
//!   3. the read/write pairing AUDIT for the whole frame-property surface
//!      across kinds: every row a kind reads is written back with the
//!      value it just read, and anything answering `UnsupportedProperty`
//!      is compared against an explicit allow-list of genuinely read-only
//!      rows. A new asymmetry fails the test with the offending pair
//!      named, so one cannot be introduced silently.

use std::io::Write;

use paged_canvas::{channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions};
use paged_mutate::{PropertyPath, Value};

/// One spread carrying every page-item kind, each with the full corner
/// vocabulary on it. The radii are deliberately high-precision so a
/// round-trip that reformats them is visible.
fn corners_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" GeometricBounds="10 10 90 90" ItemTransform="1 0 0 1 0 0"
  CornerOption="RoundedCorner" CornerRadius="14.740157480314963"
  TopLeftCornerOption="BevelCorner" TopLeftCornerRadius="14.740157480314963"/>
<Rectangle Self="r1" GeometricBounds="10 110 90 190" ItemTransform="1 0 0 1 0 0"
  CornerOption="RoundedCorner" CornerRadius="44.51279527491718"/>
<Oval Self="ov1" GeometricBounds="10 210 90 290" ItemTransform="1 0 0 1 0 0"
  CornerOption="RoundedCorner" CornerRadius="42.51968503937008"
  TopLeftCornerOption="RoundedCorner" TopLeftCornerRadius="42.51968503937008"/>
<GraphicLine Self="gl1" GeometricBounds="10 310 90 390" ItemTransform="1 0 0 1 0 0"
  CornerRadius="99.21259842519686"
  TopLeftCornerRadius="99.21259842519686" TopRightCornerRadius="99.21259842519686"
  BottomLeftCornerRadius="99.21259842519686" BottomRightCornerRadius="99.21259842519686"/>
<Polygon Self="pg1" GeometricBounds="10 410 90 490" ItemTransform="1 0 0 1 0 0"
  CornerOption="BevelCorner" CornerRadius="16.0625"
  TopLeftCornerOption="BevelCorner" TopLeftCornerRadius="16.0625"/>
<Group Self="g1" ItemTransform="1 0 0 1 0 0"
  CornerOption="RoundedCorner" CornerRadius="70.86614173228347"
  TopLeftCornerOption="RoundedCorner" TopLeftCornerRadius="70.86614173228347">
  <Rectangle Self="gr1" GeometricBounds="110 10 190 90" ItemTransform="1 0 0 1 0 0"/>
</Group>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// The `f32` you get from parsing a corpus spelling.
///
/// The corner radii InDesign writes carry f64-grade precision
/// (`99.21259842519686`), which an `f32` cannot hold — comparing against
/// the literal trips clippy's `excessive_precision`, and hand-truncating
/// it to `99.212_6` would hide WHICH corpus value is under test. Parsing
/// the real spelling keeps the evidence visible and the comparison exact.
fn f(spelling: &str) -> Option<f32> {
    Some(spelling.parse::<f32>().expect("corpus float"))
}

fn model() -> CanvasModel {
    CanvasModel::load("d1", &corners_idml(), CanvasOptions::default()).expect("load corners idml")
}

/// The six page-item kinds, with the element id each carries in the
/// fixture above.
fn every_kind() -> Vec<(&'static str, ElementId)> {
    vec![
        ("TextFrame", ElementId::TextFrame("tf1".into())),
        ("Rectangle", ElementId::Rectangle("r1".into())),
        ("Oval", ElementId::Oval("ov1".into())),
        ("GraphicLine", ElementId::GraphicLine("gl1".into())),
        ("Polygon", ElementId::Polygon("pg1".into())),
        ("Group", ElementId::Group("g1".into())),
    ]
}

const CORNER_PATHS: [PropertyPath; 8] = [
    PropertyPath::FrameCornerOptionTopLeft,
    PropertyPath::FrameCornerRadiusTopLeft,
    PropertyPath::FrameCornerOptionTopRight,
    PropertyPath::FrameCornerRadiusTopRight,
    PropertyPath::FrameCornerOptionBottomRight,
    PropertyPath::FrameCornerRadiusBottomRight,
    PropertyPath::FrameCornerOptionBottomLeft,
    PropertyPath::FrameCornerRadiusBottomLeft,
];

/// C-18 (1) — every kind parses the corner vocabulary off its own
/// element. Before this, only Rectangle and Polygon did, and the other
/// four kinds' attributes lived only in the source bytes.
#[test]
fn c18_every_kind_parses_the_corner_vocabulary() {
    let m = model();
    let spread = &m.scene().spreads[0].spread;

    let tf = &spread.text_frames[0];
    assert_eq!(tf.corner_radius, f("14.740157480314963"));
    assert_eq!(tf.corner_option.as_deref(), Some("RoundedCorner"));
    assert_eq!(tf.corners[0].option, Some(paged_model::CornerOption::Bevel));

    let ov = &spread.ovals[0];
    assert_eq!(ov.corner_radius, f("42.51968503937008"));
    assert_eq!(
        ov.corners[0].option,
        Some(paged_model::CornerOption::Rounded)
    );

    let gl = &spread.graphic_lines[0];
    assert_eq!(gl.corner_radius, f("99.21259842519686"));
    // The corpus never puts a `*CornerOption` on a `<GraphicLine>`, and
    // neither does the fixture — the radii are inert carriers.
    assert_eq!(gl.corner_option, None);
    assert_eq!(gl.corners[0].option, None);
    assert_eq!(gl.corners[0].radius, f("99.21259842519686"));

    let g = &spread.groups[0];
    assert_eq!(g.corner_radius, f("70.86614173228347"));
    assert_eq!(
        g.corners[0].option,
        Some(paged_model::CornerOption::Rounded)
    );
}

/// E-1 — the descriptor enumerated the corner slots for RECTANGLES only,
/// so a panel had nothing to bind to on any other kind even after the
/// kernel learned to apply them. Every kind now carries all eight rows.
#[test]
fn e1_every_kind_reads_all_eight_corner_rows() {
    let m = model();
    for (name, id) in every_kind() {
        let props = m
            .element_properties(&id)
            .unwrap_or_else(|| panic!("{name} must answer element_properties"));
        for path in CORNER_PATHS {
            assert!(
                props.entries.iter().any(|e| e.path == path),
                "{name} is missing the {path:?} read row",
            );
        }
    }
}

/// E-1 — …and the write is accepted on every one of them, so no kind
/// exposes a corner slot it would then reject (the C-17 lesson).
#[test]
fn e1_every_kind_accepts_the_matching_corner_write() {
    for (name, id) in every_kind() {
        let mut m = model();
        for path in CORNER_PATHS {
            let value = match path {
                PropertyPath::FrameCornerOptionTopLeft
                | PropertyPath::FrameCornerOptionTopRight
                | PropertyPath::FrameCornerOptionBottomRight
                | PropertyPath::FrameCornerOptionBottomLeft => {
                    Value::Text("InverseRoundedCorner".to_string())
                }
                _ => Value::Length(Some(7.25)),
            };
            m.apply_mutation(&Mutation::SetElementProperty {
                element_id: id.clone(),
                path,
                value,
            })
            .unwrap_or_else(|e| panic!("{name} rejected {path:?}: {e:?}"));
        }
        // …and the writes landed where the read half looks.
        let props = m.element_properties(&id).expect("props after write");
        let radius = props
            .entries
            .iter()
            .find(|e| e.path == PropertyPath::FrameCornerRadiusBottomLeft)
            .and_then(|e| e.value.clone());
        assert_eq!(
            radius,
            Some(Value::Length(Some(7.25))),
            "{name} read half did not observe the write",
        );
    }
}

/// C-18 — the prior value is captured, so a corner write is a bytewise
/// inverse: undo restores the exact source value rather than a
/// re-formatted one.
#[test]
fn c18_corner_write_inverse_restores_the_exact_prior_value() {
    for (name, id) in every_kind() {
        let mut m = model();
        let before = m
            .element_properties(&id)
            .expect("props")
            .entries
            .iter()
            .find(|e| e.path == PropertyPath::FrameCornerRadiusTopLeft)
            .and_then(|e| e.value.clone());
        m.apply_mutation(&Mutation::SetElementProperty {
            element_id: id.clone(),
            path: PropertyPath::FrameCornerRadiusTopLeft,
            value: Value::Length(Some(3.5)),
        })
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(m.undo().is_some(), "{name} undo produced no outcome");
        let after = m
            .element_properties(&id)
            .expect("props")
            .entries
            .iter()
            .find(|e| e.path == PropertyPath::FrameCornerRadiusTopLeft)
            .and_then(|e| e.value.clone());
        assert_eq!(before, after, "{name} undo did not restore the prior value");
    }
}

/// Did the kernel DISPATCH this (kind, path) pair at all?
///
/// `apply_mutation` funnels every kernel error into one
/// `WorkerError::NotImplemented { what }` string, so the variant cannot
/// be matched on — the message is the only signal. Two of them matter:
///
/// * `"property X is not supported on Y"` — `set_property` fell through
///   to its catch-all: this kind has NO arm for this path. That is an
///   asymmetry when the descriptor reads it.
/// * `"value type for property X doesn't match"` — the arm ran and
///   objected to the VALUE. That proves the pair is dispatched, which is
///   what the audit is asking about.
fn is_dispatched(r: Result<impl Sized, paged_canvas::channel::WorkerError>) -> bool {
    match r {
        Ok(_) => true,
        Err(e) => !format!("{e:?}").contains("is not supported on"),
    }
}

/// Rows that a kind exposes on READ and deliberately rejects on WRITE.
///
/// Every entry is a considered decision, not a gap:
///
/// * `FrameNextTextFrame` / `FramePreviousTextFrame` — threading is
///   structural; it is edited through `LinkFrames`, not a property write
///   (W3.A0 pinned the rejection deliberately).
/// * `FrameBounds` on a Group — a group's extent is the UNION of its
///   members' AABBs, a derived read. Resizing a group means resizing its
///   members; there is nothing on the group itself to write.
///
/// The audit below fails on anything NOT in this list, so a future
/// read-without-write lands as a test failure naming the exact pair.
/// * `FrameStrokeAlignment` on a Polygon — a PRE-EXISTING asymmetry this
///   audit surfaced, not one C-18 introduced (verified against HEAD:
///   `model.rs` has read rows for Rectangle and Polygon, `set_property.rs`
///   has a write arm only for `NodeId::Rectangle`). The Polygon
///   descriptor's own comment claims the alignment mutation is "now
///   apply-wired for all path kinds", which is false. Recorded rather
///   than fixed here because closing it means deciding how
///   `StrokeAlignment` offsets a closed Bezier outline — `W1.5`'s
///   `stroke_alignment_inward` lane — which is a renderer change, not a
///   descriptor one.
const KNOWN_READ_ONLY: &[(&str, PropertyPath)] = &[
    ("TextFrame", PropertyPath::NextTextFrame),
    ("TextFrame", PropertyPath::PreviousTextFrame),
    ("Group", PropertyPath::FrameBounds),
    ("Polygon", PropertyPath::FrameStrokeAlignment),
];

/// E-1's audit half — read/write pairing across the whole frame-property
/// surface, for every kind.
///
/// Method: read the descriptor, then write each row BACK with the value
/// it just answered. A same-value write is always type-correct, so an
/// `UnsupportedProperty` can only mean "this kind has no write arm for a
/// property it happily reads" — which is exactly the C-17 failure mode.
/// Errors that are not `UnsupportedProperty` are ignored: they mean the
/// arm exists and objected to something else (a value range, a missing
/// sibling), which is not an asymmetry.
#[test]
fn e1_read_write_pairing_audit_across_kinds() {
    let mut asymmetries: Vec<(String, PropertyPath)> = Vec::new();
    for (name, id) in every_kind() {
        let read = model().element_properties(&id).expect("props");
        for entry in &read.entries {
            let Some(value) = entry.value.clone() else {
                continue;
            };
            // PluginMetadata is a carrier row, not a frame property.
            if matches!(entry.path, PropertyPath::PluginMetadata) {
                continue;
            }
            let mut m = model();
            let r = m.apply_mutation(&Mutation::SetElementProperty {
                element_id: id.clone(),
                path: entry.path,
                value,
            });
            if !is_dispatched(r) {
                asymmetries.push((name.to_string(), entry.path));
            }
        }
    }
    let unexpected: Vec<_> = asymmetries
        .iter()
        .filter(|(k, p)| !KNOWN_READ_ONLY.iter().any(|(ek, ep)| ek == k && ep == p))
        .collect();
    assert!(
        unexpected.is_empty(),
        "read/write asymmetry: these kinds expose a property on read but \
         reject the write. Either add the apply arm or record it in \
         KNOWN_READ_ONLY with the reason: {unexpected:#?}",
    );
}

/// The OTHER direction of the same audit — properties a kind ACCEPTS on
/// write but never surfaces on read, so an editor can set them and then
/// has nothing to show.
///
/// This is the larger of the two gaps and it is deliberately RECORDED
/// rather than closed in this pass: closing it means designing what an
/// Oval's inspector shows, which is a panel decision, not a kernel one.
/// The list below is the measured truth as of C-18; the test fails if it
/// GROWS, so the debt cannot quietly get worse, and shrinks are welcome
/// (remove the entry when you add the read row).
///
/// The standout is `Oval`: it had no descriptor arm at all before C-18
/// (it fell through to `_ => None`), so every paint property its apply
/// arms already accepted — fill, stroke, tint, opacity, dash, gap, type,
/// alignment — was unreadable. C-18 gave it bounds + transform + the
/// eight corner rows; the paint set stays on this list.
const KNOWN_WRITE_ONLY: &[(&str, PropertyPath)] = &[
    // Oval — the paint set its apply arms accept but the descriptor
    // still does not surface (see the note above).
    ("Oval", PropertyPath::FrameFillColor),
    ("Oval", PropertyPath::FrameFillTint),
    ("Oval", PropertyPath::FrameStrokeColor),
    ("Oval", PropertyPath::FrameStrokeWeight),
    ("Oval", PropertyPath::FrameStrokeType),
    // (`FrameStrokeAlignment` is NOT here: only `NodeId::Rectangle` has
    // that write arm, so on an Oval it is neither readable nor writable —
    // symmetric, if empty.)
    ("Oval", PropertyPath::FrameStrokeGapColor),
    ("Oval", PropertyPath::FrameStrokeGapTint),
    ("Oval", PropertyPath::FrameStrokeDashArray),
    ("Oval", PropertyPath::FrameOpacity),
    ("Oval", PropertyPath::FrameBlendMode),
    ("Oval", PropertyPath::FrameOverprintFill),
    ("Oval", PropertyPath::FrameOverprintStroke),
    ("Oval", PropertyPath::AppliedObjectStyle),
    // GraphicLine — no fill (a line has none), but these paint/flag rows
    // are writable and unread.
    ("GraphicLine", PropertyPath::FrameOverprintStroke),
    // Polygon — same shape.
    ("Polygon", PropertyPath::FrameOverprintFill),
    ("Polygon", PropertyPath::FrameOverprintStroke),
];

#[test]
fn e1_write_without_read_audit_across_kinds() {
    // The frame-property surface a page item can plausibly carry. Listed
    // explicitly rather than scraped from `paged-introspect`'s catalog:
    // an audit should state what it probed, and paged-canvas does not
    // otherwise depend on that crate.
    let probes: [PropertyPath; 18] = [
        PropertyPath::FrameFillColor,
        PropertyPath::FrameFillTint,
        PropertyPath::FrameStrokeColor,
        PropertyPath::FrameStrokeWeight,
        PropertyPath::FrameStrokeType,
        PropertyPath::FrameStrokeJoin,
        PropertyPath::FrameStrokeMiterLimit,
        PropertyPath::FrameStrokeAlignment,
        PropertyPath::FrameStrokeGapColor,
        PropertyPath::FrameStrokeGapTint,
        PropertyPath::FrameStrokeDashArray,
        PropertyPath::FrameStrokeEndCap,
        PropertyPath::FrameOpacity,
        PropertyPath::FrameBlendMode,
        PropertyPath::FrameOverprintFill,
        PropertyPath::FrameOverprintStroke,
        PropertyPath::FrameNonprinting,
        PropertyPath::AppliedObjectStyle,
    ];

    let mut write_only: Vec<(String, PropertyPath)> = Vec::new();
    for (name, id) in every_kind() {
        let read_paths: Vec<PropertyPath> = model()
            .element_properties(&id)
            .expect("props")
            .entries
            .iter()
            .map(|e| e.path)
            .collect();
        for path in &probes {
            if read_paths.contains(path) {
                continue;
            }
            // One probe value is enough: `is_dispatched` treats a
            // value-type complaint as proof the arm ran, so the wrong
            // shape still answers the question.
            let mut m = model();
            let r = m.apply_mutation(&Mutation::SetElementProperty {
                element_id: id.clone(),
                path: *path,
                value: Value::Text(String::new()),
            });
            if is_dispatched(r) {
                write_only.push((name.to_string(), *path));
            }
        }
    }
    let unexpected: Vec<_> = write_only
        .iter()
        .filter(|(k, p)| !KNOWN_WRITE_ONLY.iter().any(|(ek, ep)| ek == k && ep == p))
        .collect();
    assert!(
        unexpected.is_empty(),
        "NEW write-without-read asymmetry (a kind accepts the write but \
         the descriptor never shows the value). Add the read row, or \
         record it in KNOWN_WRITE_ONLY: {unexpected:#?}",
    );
    // …and the recorded debt must be the MEASURED debt, so a stale entry
    // can't make the list look worse (or a closed gap look open) than it
    // is. Remove the entry when you add the read row.
    let stale: Vec<_> = KNOWN_WRITE_ONLY
        .iter()
        .filter(|(ek, ep)| !write_only.iter().any(|(k, p)| k == ek && p == ep))
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN_WRITE_ONLY lists pairs that are no longer asymmetric — \
         the gap was closed; delete these entries: {stale:#?}",
    );
}
