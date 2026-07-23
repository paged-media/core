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

//! New-resource injection for save-back (W1.15 lane 2).
//!
//! Swatches / gradients created by ops since load live in the model's
//! `palette` but have no `<Color>` / `<Gradient>` element in
//! `Resources/Graphic.xml`; paragraph / character styles created by ops
//! live in `styles` with no `<ParagraphStyle>` / `<CharacterStyle>` in
//! `Resources/Styles.xml`. A round-trip that leaves them unserialised
//! re-opens with a *referenced-but-undefined* resource — a frame whose
//! `FillColor="Color/u3"` resolves to nothing. This module closes that
//! gap by **injecting** the missing entries into the existing resource
//! XML, just before the matching close tag, in the canonical `paged_gen`
//! shape so a re-parse reproduces the resolved appearance.
//!
//! Both patchers are pure pass-throughs when the model carries nothing
//! the source XML lacks — they re-emit the original token stream and
//! splice nothing, so an unmutated document's resource entries stay
//! byte-identical (the writer then takes the verbatim copy path).

use std::io::Cursor;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

use idml_import::graphic::{ColorEntry, GradientEntry, GradientKind, Graphic};
use idml_import::styles::{CharacterStyleDef, ParagraphStyleDef, StyleSheet};

use crate::rewrite::{escape_attr, format_f32};

/// Read an attribute's decoded value off a start tag (local copy of the
/// rewrite helper — kept private so the two modules stay independent).
fn attr_value(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| std::str::from_utf8(&a.value).ok().map(|s| s.to_string()))
}

/// Emit a self-closing element from `(key, value)` pairs (values
/// escaped). Element name taken verbatim.
fn emit_empty(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, String)],
) -> Result<(), quick_xml::Error> {
    let mut e = BytesStart::new(name.to_string());
    for (k, v) in attrs {
        e.push_attribute((k.as_bytes(), escape_attr(v).as_bytes()));
    }
    writer.write_event(Event::Empty(e))?;
    Ok(())
}

/// Whitespace-separated channel values the IDML way.
fn format_color_value(value: &[f32]) -> String {
    value
        .iter()
        .map(|v| format_f32(*v))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------
// Graphic.xml — colours + gradients
// ---------------------------------------------------------------------

/// Serialise one model `<Color>` swatch in the canonical attribute order
/// (`Self Model Space ColorValue Name [AlternateSpace AlternateColorValue
/// TintValue]`). The parser keys on these attributes, so a re-parse
/// reproduces the swatch and any frame referencing it resolves.
fn write_color(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    c: &ColorEntry,
) -> Result<(), quick_xml::Error> {
    let mut attrs: Vec<(&str, String)> = vec![
        ("Self", c.self_id.clone()),
        ("Model", c.model.as_attr().to_string()),
        ("Space", c.space.as_attr().to_string()),
        ("ColorValue", format_color_value(&c.value)),
    ];
    if let Some(name) = &c.name {
        attrs.push(("Name", name.clone()));
    }
    if let Some(alt) = c.alternate_space {
        attrs.push(("AlternateSpace", alt.as_attr().to_string()));
        attrs.push((
            "AlternateColorValue",
            format_color_value(&c.alternate_value),
        ));
    }
    if let Some(t) = c.tint {
        attrs.push(("TintValue", format_f32(t)));
    }
    emit_empty(writer, "Color", &attrs)
}

/// Serialise one model `<Gradient>` swatch + its stops.
fn write_gradient(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    g: &GradientEntry,
) -> Result<(), quick_xml::Error> {
    let kind = match g.kind {
        GradientKind::Radial => "Radial",
        // Linear is the IDML default for an unknown / linear type.
        GradientKind::Linear | GradientKind::Unknown => "Linear",
    };
    let mut attrs: Vec<(&str, String)> = vec![("Self", g.self_id.clone())];
    if let Some(name) = &g.name {
        attrs.push(("Name", name.clone()));
    }
    attrs.push(("Type", kind.to_string()));
    let mut start = BytesStart::new("Gradient");
    for (k, v) in &attrs {
        start.push_attribute((k.as_bytes(), escape_attr(v).as_bytes()));
    }
    writer.write_event(Event::Start(start))?;
    for s in &g.stops {
        let mut sattrs: Vec<(&str, String)> = vec![
            ("StopColor", s.stop_color.clone()),
            ("Location", format_f32(s.location_pct)),
        ];
        if let Some(m) = s.midpoint_pct {
            sattrs.push(("Midpoint", format_f32(m)));
        }
        emit_empty(writer, "GradientStop", &sattrs)?;
    }
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Gradient")))?;
    Ok(())
}

/// Rewrite `Resources/Graphic.xml` so every model `<Color>` / `<Gradient>`
/// is present. Existing entries pass through verbatim; entries the source
/// lacks are appended just before `</idPkg:Graphic>`. Byte-identical to
/// `original` when nothing new.
pub fn patch_graphic(original: &[u8], palette: &Graphic) -> Result<Vec<u8>, quick_xml::Error> {
    let mut reader = Reader::from_reader(original);
    let config = reader.config_mut();
    config.expand_empty_elements = false;
    config.trim_text(false);

    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();

    let mut seen_colors: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_gradients: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        let ev = reader.read_event_into(&mut buf)?;
        match ev {
            Event::Eof => break,
            Event::Start(ref e) | Event::Empty(ref e) => {
                match e.name().as_ref() {
                    b"Color" => {
                        if let Some(id) = attr_value(e, b"Self") {
                            seen_colors.insert(id);
                        }
                    }
                    b"Gradient" => {
                        if let Some(id) = attr_value(e, b"Self") {
                            seen_gradients.insert(id);
                        }
                    }
                    _ => {}
                }
                writer.write_event(ev.borrow())?;
            }
            Event::End(ref e) if e.name().as_ref() == b"idPkg:Graphic" => {
                // Inject the model entries the source never carried, in
                // the palette's stable BTreeMap order.
                for c in palette.colors.values() {
                    if !seen_colors.contains(&c.self_id) {
                        write_color(&mut writer, c)?;
                    }
                }
                for g in palette.gradients.values() {
                    if !seen_gradients.contains(&g.self_id) {
                        write_gradient(&mut writer, g)?;
                    }
                }
                writer.write_event(ev.borrow())?;
            }
            _ => writer.write_event(ev.borrow())?,
        }
        buf.clear();
    }

    Ok(writer.into_inner().into_inner())
}

// ---------------------------------------------------------------------
// Styles.xml — paragraph + character styles
// ---------------------------------------------------------------------

/// Common authoring fields shared by paragraph + character styles. Only
/// the high-frequency knobs are serialised; the rest cascade from
/// `BasedOn` / the document default (a freshly-created style carries only
/// name + based_on until a `SetStyleProperty` writes a field).
fn push_style_common(
    attrs: &mut Vec<(&'static str, String)>,
    name: &Option<String>,
    based_on: &Option<String>,
    font: &Option<String>,
    font_style: &Option<String>,
    point_size: Option<f32>,
    fill_color: &Option<String>,
) {
    if let Some(n) = name {
        attrs.push(("Name", n.clone()));
    }
    if let Some(b) = based_on {
        attrs.push(("BasedOn", b.clone()));
    }
    if let Some(f) = font {
        attrs.push(("AppliedFont", f.clone()));
    }
    if let Some(fs) = font_style {
        attrs.push(("FontStyle", fs.clone()));
    }
    if let Some(sz) = point_size {
        attrs.push(("PointSize", format_f32(sz)));
    }
    if let Some(fc) = fill_color {
        attrs.push(("FillColor", fc.clone()));
    }
}

fn write_paragraph_style(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    s: &ParagraphStyleDef,
) -> Result<(), quick_xml::Error> {
    let mut attrs: Vec<(&str, String)> = vec![("Self", s.self_id.clone())];
    push_style_common(
        &mut attrs,
        &s.name,
        &s.based_on,
        &s.font,
        &s.font_style,
        s.point_size,
        &s.fill_color,
    );
    emit_empty(writer, "ParagraphStyle", &attrs)
}

fn write_character_style(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    s: &CharacterStyleDef,
) -> Result<(), quick_xml::Error> {
    let mut attrs: Vec<(&str, String)> = vec![("Self", s.self_id.clone())];
    push_style_common(
        &mut attrs,
        &s.name,
        &s.based_on,
        &s.font,
        &s.font_style,
        s.point_size,
        &s.fill_color,
    );
    emit_empty(writer, "CharacterStyle", &attrs)
}

/// Rewrite `Resources/Styles.xml` so every model paragraph / character
/// style is present. New paragraph styles are injected before
/// `</RootParagraphStyleGroup>`, character styles before
/// `</RootCharacterStyleGroup>`. Byte-identical to `original` when
/// nothing new (and when the source has no group, the new styles flush
/// at `</idPkg:Styles>` so they aren't silently dropped).
pub fn patch_styles(original: &[u8], styles: &StyleSheet) -> Result<Vec<u8>, quick_xml::Error> {
    let mut reader = Reader::from_reader(original);
    let config = reader.config_mut();
    config.expand_empty_elements = false;
    config.trim_text(false);

    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();

    let mut seen_para: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_char: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut para_group_closed = false;
    let mut char_group_closed = false;

    loop {
        let ev = reader.read_event_into(&mut buf)?;
        match ev {
            Event::Eof => break,
            Event::Start(ref e) | Event::Empty(ref e) => {
                match e.name().as_ref() {
                    b"ParagraphStyle" => {
                        if let Some(id) = attr_value(e, b"Self") {
                            seen_para.insert(id);
                        }
                    }
                    b"CharacterStyle" => {
                        if let Some(id) = attr_value(e, b"Self") {
                            seen_char.insert(id);
                        }
                    }
                    _ => {}
                }
                writer.write_event(ev.borrow())?;
            }
            Event::End(ref e) if e.name().as_ref() == b"RootParagraphStyleGroup" => {
                for s in styles.paragraph_styles.values() {
                    if !seen_para.contains(&s.self_id) {
                        write_paragraph_style(&mut writer, s)?;
                    }
                }
                para_group_closed = true;
                writer.write_event(ev.borrow())?;
            }
            Event::End(ref e) if e.name().as_ref() == b"RootCharacterStyleGroup" => {
                for s in styles.character_styles.values() {
                    if !seen_char.contains(&s.self_id) {
                        write_character_style(&mut writer, s)?;
                    }
                }
                char_group_closed = true;
                writer.write_event(ev.borrow())?;
            }
            Event::End(ref e) if e.name().as_ref() == b"idPkg:Styles" => {
                // Fallback: a source with no Root*StyleGroup (rare) still
                // gets the new defs so a reference never dangles.
                if !para_group_closed {
                    for s in styles.paragraph_styles.values() {
                        if !seen_para.contains(&s.self_id) {
                            write_paragraph_style(&mut writer, s)?;
                        }
                    }
                }
                if !char_group_closed {
                    for s in styles.character_styles.values() {
                        if !seen_char.contains(&s.self_id) {
                            write_character_style(&mut writer, s)?;
                        }
                    }
                }
                writer.write_event(ev.borrow())?;
            }
            _ => writer.write_event(ev.borrow())?,
        }
        buf.clear();
    }

    Ok(writer.into_inner().into_inner())
}
