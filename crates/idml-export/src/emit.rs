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

//! New-entry emission (C-8) — full Story / Spread parts for model
//! objects MINTED after parse, which have no original ZIP entry the
//! carry-through writer could patch:
//!
//! * a story minted by `InsertNode { NodeSpec::TextFrame { parent_story:
//!   Some(_) } }` (the wire's InsertTextFrame) carries `src: ""` — text
//!   poured into a fresh frame was silently dropped on export;
//! * a spread minted by `InsertPage` carries a fresh
//!   `Spreads/Spread_<id>.xml` src the source archive doesn't contain.
//!
//! Both are serialised here from the in-memory model, in the same
//! vocabulary [`crate::rewrite`] writes when patching (its emit helpers
//! are reused directly), wrapped in the standard `idPkg` part envelope
//! our own parser reads back (`idml_import::Story::parse` /
//! `Spread::parse`). The new entries are then REFERENCED by a minimal
//! `designmap.xml` insertion ([`patch_designmap`]) — an unmutated
//! document never reaches this module, so its designmap round-trips
//! byte-identically.

use std::io::Cursor;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};

use idml_import::{CharacterRun, Spread, Story};

use crate::rewrite;

const PKG_NS: &str = "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging";

/// Sanitize a model `Self` id into an entry-name stem: `/` → `_`.
/// Wire-minted stories are named `Story/u<n>`; an entry path can't
/// carry the slash, and `paged_scene::derive_story_id` re-derives the
/// story id from the entry stem on reopen — so the sanitized form is
/// the id the document carries after a save→reopen round-trip (the
/// frame's `ParentStory` is written sanitized too, see
/// `rewrite::write_new_text_frame`).
pub(crate) fn sanitize_id(id: &str) -> String {
    id.replace('/', "_")
}

/// Entry path for a minted story: `Stories/Story_<sanitized-id>.xml`.
/// `derive_story_id` strips exactly the `Story_` prefix added here, so
/// the reopened story's `self_id` equals [`sanitize_id`] of the minted
/// id.
pub(crate) fn story_src_for(self_id: &str) -> String {
    format!("Stories/Story_{}.xml", sanitize_id(self_id))
}

fn new_part_writer() -> Result<Writer<Cursor<Vec<u8>>>, quick_xml::Error> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    writer.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))?;
    Ok(writer)
}

fn open_pkg_root(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    dom_version: &str,
) -> Result<(), quick_xml::Error> {
    let mut root = BytesStart::new(name);
    root.push_attribute(("xmlns:idPkg", PKG_NS));
    root.push_attribute(("DOMVersion", dom_version));
    writer.write_event(Event::Start(root))?;
    Ok(())
}

/// Serialise a full `Stories/Story_*.xml` part from the in-memory model.
///
/// The body vocabulary is exactly what `rewrite::rewrite_story` owns
/// when patching: one `<ParagraphStyleRange>` per model paragraph
/// (`AppliedParagraphStyle` when set), one `<CharacterStyleRange>` per
/// run carrying the patchable character attributes, run text split
/// across `<Content>` / `<Br/>` / `<Tab/>` via
/// `rewrite::write_run_content`. Attributes the model doesn't set are
/// omitted, so a save→reopen reproduces the model (`None` stays `None`).
/// In-paragraph tables / footnotes are not serialised — a minted story
/// is the K-1 text-pour lane; structured content in a minted story is a
/// documented loss until a consumer needs it.
pub(crate) fn story_part(
    self_id: &str,
    story: &Story,
    dom_version: &str,
) -> Result<Vec<u8>, quick_xml::Error> {
    let mut writer = new_part_writer()?;
    open_pkg_root(&mut writer, "idPkg:Story", dom_version)?;
    let mut s = BytesStart::new("Story");
    s.push_attribute(("Self", self_id));
    writer.write_event(Event::Start(s))?;
    for p in &story.paragraphs {
        let mut attrs: Vec<(&str, String)> = Vec::new();
        if let Some(style) = &p.paragraph_style {
            attrs.push(("AppliedParagraphStyle", style.clone()));
        }
        rewrite::emit_start_with_attrs(&mut writer, "ParagraphStyleRange", &attrs)?;
        for r in &p.runs {
            rewrite::emit_start_with_attrs(
                &mut writer,
                "CharacterStyleRange",
                &character_run_attrs(r),
            )?;
            rewrite::write_run_content(&mut writer, &r.text)?;
            writer.write_event(Event::End(BytesEnd::new("CharacterStyleRange")))?;
        }
        writer.write_event(Event::End(BytesEnd::new("ParagraphStyleRange")))?;
    }
    writer.write_event(Event::End(BytesEnd::new("Story")))?;
    writer.write_event(Event::End(BytesEnd::new("idPkg:Story")))?;
    Ok(writer.into_inner().into_inner())
}

/// The `<CharacterStyleRange>` attributes for one run — the same key
/// set `rewrite::character_attr_patch` owns, emitted only when set.
fn character_run_attrs(r: &CharacterRun) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let string_attrs: [(&'static str, &Option<String>); 9] = [
        ("AppliedCharacterStyle", &r.character_style),
        ("AppliedFont", &r.font),
        ("FontStyle", &r.font_style),
        ("FillColor", &r.fill_color),
        ("StrokeColor", &r.stroke_color),
        ("Capitalization", &r.capitalization),
        ("Position", &r.position),
        ("KerningMethod", &r.kerning_method),
        ("AppliedLanguage", &r.applied_language),
    ];
    for (k, v) in string_attrs {
        if let Some(v) = v {
            out.push((k, v.clone()));
        }
    }
    let f32_attrs: [(&'static str, Option<f32>); 9] = [
        ("PointSize", r.point_size),
        ("FillTint", r.fill_tint),
        ("StrokeWeight", r.stroke_weight),
        ("Leading", r.leading),
        ("Tracking", r.tracking),
        ("BaselineShift", r.baseline_shift),
        ("HorizontalScale", r.horizontal_scale),
        ("VerticalScale", r.vertical_scale),
        ("Skew", r.skew),
    ];
    for (k, v) in f32_attrs {
        if let Some(v) = v {
            out.push((k, rewrite::format_f32(v)));
        }
    }
    let bool_attrs: [(&'static str, Option<bool>); 3] = [
        ("Underline", r.underline),
        ("StrikeThru", r.strikethru),
        ("Ligatures", r.ligatures_on),
    ];
    for (k, v) in bool_attrs {
        if let Some(v) = v {
            out.push((k, v.to_string()));
        }
    }
    out
}

/// Serialise a full `Spreads/Spread_*.xml` part from the in-memory
/// model: the `<Spread>` start tag, one `<Page>` per model page, then
/// every page item via `rewrite::write_inserted_items` (with an empty
/// seen-set every item is "inserted" — a minted spread's items all
/// arrived through ops). Groups / guides on a minted spread are not
/// serialised (the group-insert lane is a separate, documented defer).
pub(crate) fn spread_part(spread: &Spread, dom_version: &str) -> Result<Vec<u8>, quick_xml::Error> {
    let mut writer = new_part_writer()?;
    open_pkg_root(&mut writer, "idPkg:Spread", dom_version)?;
    let mut attrs: Vec<(&str, String)> = Vec::new();
    if let Some(id) = &spread.self_id {
        attrs.push(("Self", id.clone()));
    }
    if let Some(m) = &spread.item_transform {
        attrs.push(("ItemTransform", rewrite::format_matrix(m)));
    }
    rewrite::emit_start_with_attrs(&mut writer, "Spread", &attrs)?;
    for p in &spread.pages {
        let mut pa: Vec<(&str, String)> = Vec::new();
        if let Some(id) = &p.self_id {
            pa.push(("Self", id.clone()));
        }
        if let Some(n) = &p.name {
            pa.push(("Name", n.clone()));
        }
        if let Some(m) = &p.applied_master {
            pa.push(("AppliedMaster", m.clone()));
        }
        if let Some(m) = &p.item_transform {
            pa.push(("ItemTransform", rewrite::format_matrix(m)));
        }
        // GeometricBounds is the one attribute the parser requires to
        // accept a `<Page>` at all.
        pa.push((
            "GeometricBounds",
            format!(
                "{} {} {} {}",
                rewrite::format_f32(p.bounds.top),
                rewrite::format_f32(p.bounds.left),
                rewrite::format_f32(p.bounds.bottom),
                rewrite::format_f32(p.bounds.right),
            ),
        ));
        if let Some(m) = &p.master_page_transform {
            pa.push(("MasterPageTransform", rewrite::format_matrix(m)));
        }
        if !p.override_list.is_empty() {
            pa.push(("OverrideList", p.override_list.join(" ")));
        }
        if let Some(b) = p.show_master_items {
            pa.push(("ShowMasterItems", b.to_string()));
        }
        rewrite::emit_empty_with_attrs(&mut writer, "Page", &pa)?;
    }
    rewrite::write_inserted_items(&mut writer, spread, &std::collections::HashSet::new())?;
    writer.write_event(Event::End(BytesEnd::new("Spread")))?;
    writer.write_event(Event::End(BytesEnd::new("idPkg:Spread")))?;
    Ok(writer.into_inner().into_inner())
}

/// Patch `designmap.xml` so the new entries are referenced. Every
/// original event passes through verbatim; only the new `<idPkg:Spread
/// src="..."/>` / `<idPkg:Story src="..."/>` elements are injected:
///
/// * a new spread ref goes immediately AFTER the existing spread ref
///   named by its anchor (`None` ⇒ before the first existing one) —
///   spread manifest order IS page order, so a minted spread must land
///   next to its host;
/// * new story refs are appended after the LAST existing `idPkg:Story`
///   (story manifest order only drives `doc.stories` order, and minted
///   stories were appended there too).
///
/// Anything left unplaced (no existing refs of that kind, an anchor
/// that vanished) is flushed just before `</Document>` — a reference is
/// never silently dropped.
pub(crate) fn patch_designmap(
    original: &[u8],
    new_spreads: &[(Option<String>, String)],
    new_stories: &[String],
) -> Result<Vec<u8>, quick_xml::Error> {
    // Pass 1 — count the existing story refs so "after the last one" is
    // recognisable in the single forward pass below.
    let story_total = {
        let mut reader = Reader::from_reader(original);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        let mut n = 0usize;
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Empty(e) | Event::Start(e) if e.name().as_ref() == b"idPkg:Story" => {
                    n += 1;
                }
                _ => {}
            }
            buf.clear();
        }
        n
    };

    let mut reader = Reader::from_reader(original);
    let config = reader.config_mut();
    config.expand_empty_elements = false;
    config.trim_text(false);
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();

    let mut spread_placed = vec![false; new_spreads.len()];
    let mut stories_placed = false;
    let mut first_spread_seen = false;
    let mut story_seen = 0usize;

    fn write_ref(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        kind: &str,
        src: &str,
    ) -> Result<(), quick_xml::Error> {
        rewrite::emit_empty_with_attrs(writer, kind, &[("src", src.to_string())])
    }

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Empty(e) if e.name().as_ref() == b"idPkg:Spread" => {
                if !first_spread_seen {
                    first_spread_seen = true;
                    // Anchor-less spreads precede every existing one.
                    for (i, (anchor, src)) in new_spreads.iter().enumerate() {
                        if anchor.is_none() && !spread_placed[i] {
                            write_ref(&mut writer, "idPkg:Spread", src)?;
                            spread_placed[i] = true;
                        }
                    }
                }
                let this_src = attr_value(&e, b"src");
                writer.write_event(Event::Empty(e.into_owned()))?;
                if let Some(this_src) = this_src {
                    for (i, (anchor, src)) in new_spreads.iter().enumerate() {
                        if !spread_placed[i] && anchor.as_deref() == Some(this_src.as_str()) {
                            write_ref(&mut writer, "idPkg:Spread", src)?;
                            spread_placed[i] = true;
                        }
                    }
                }
            }
            Event::Empty(e) if e.name().as_ref() == b"idPkg:Story" => {
                story_seen += 1;
                writer.write_event(Event::Empty(e.into_owned()))?;
                if story_seen == story_total && !stories_placed {
                    for src in new_stories {
                        write_ref(&mut writer, "idPkg:Story", src)?;
                    }
                    stories_placed = true;
                }
            }
            Event::End(e) if e.name().as_ref() == b"Document" => {
                // Flush everything still unplaced before the root closes.
                for (i, (_, src)) in new_spreads.iter().enumerate() {
                    if !spread_placed[i] {
                        write_ref(&mut writer, "idPkg:Spread", src)?;
                        spread_placed[i] = true;
                    }
                }
                if !stories_placed {
                    for src in new_stories {
                        write_ref(&mut writer, "idPkg:Story", src)?;
                    }
                    stories_placed = true;
                }
                writer.write_event(Event::End(e))?;
            }
            other => writer.write_event(other)?,
        }
        buf.clear();
    }

    Ok(writer.into_inner().into_inner())
}

/// Read an attribute's decoded value off a start tag (local copy of
/// `rewrite::attr_value`, which is private to that module's hot path).
fn attr_value(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| std::str::from_utf8(&a.value).ok().map(|s| s.to_string()))
}
