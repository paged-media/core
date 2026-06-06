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

//! `designmap.xml` — the root manifest that lists referenced spreads,
//! stories, masters, preferences, and so on.
//!
//! Only a tiny subset of attributes is extracted here — enough to drive
//! seed-corpus round-trips. Full schema coverage lands during Phase 0.

use quick_xml::events::Event;
use serde::Serialize;

use crate::util::attr;
use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct DesignMap {
    pub spreads: Vec<SpreadRef>,
    pub stories: Vec<StoryRef>,
    pub master_spreads: Vec<String>,
    /// `DOMVersion` attribute on the root `<Document>` element (e.g.
    /// `"18.5"` for InDesign 2023). Surfaced read-only so tooling can
    /// report the authoring DOM; the parser is version-agnostic and
    /// does **not** branch on it yet (no version negotiation).
    pub dom_version: Option<String>,
    /// Document-level color management settings, extracted from the
    /// root `<Document>` element. Drives ICC transform construction —
    /// the renderer matches `color_settings.cmyk_profile` against its
    /// bundled profile set and falls back to a naive CMYK→sRGB
    /// approximation when the named profile isn't shipped.
    pub color_settings: ColorSettings,
    /// `<DocumentPreference>` bleed/slug offsets (points). Unread by
    /// the renderer (pages rasterise at trim); the PDF exporter uses
    /// them for BleedBox/MediaBox geometry. Zeros when the element
    /// or attribute is absent.
    pub document_preference: DocumentPreference,
    /// Document layers, in serialization order (which mirrors the
    /// stacking order — first layer = bottom of the z-stack). Each
    /// page item references its layer via `ItemLayer="<self_id>"`.
    /// The renderer skips items whose layer is hidden or non-printable.
    pub layers: Vec<Layer>,
    /// `<TextVariable>` definitions. Each carries a `VariableType`
    /// (`FileNameVariable`, `RunningHeaderVariable`, `ChapterNumberType`,
    /// `XrefPageNumberType`, etc.) and is referenced from stories via
    /// `<TextVariableInstance AssociatedTextVariable="TextVariable/<id>"
    /// ResultText="..."/>`. The renderer treats `ResultText` as the
    /// authoritative value at the moment InDesign exported the IDML —
    /// "live" recomputation per page is a future task.
    pub text_variables: Vec<TextVariable>,
    /// SDK Phase 5 (v1 sweep) — `<Article>` definitions. Each is a
    /// named ordered list of `ArticleMember` refs that group
    /// related stories for accessibility / linked-text reading
    /// order. The renderer doesn't branch on them today; the
    /// editor surfaces them via the Articles panel.
    pub articles: Vec<Article>,
    /// SDK Phase 5 (v1 sweep) — `<Hyperlink>` definitions. Each
    /// has a name + a source (HyperlinkTextSource ref) + a
    /// destination (URL, page, or anchor).
    pub hyperlinks: Vec<Hyperlink>,
    /// SDK Phase 5 (v1 sweep) — `<Bookmark>` definitions. Each
    /// is a named anchor pointing at a destination (typically a
    /// hyperlink-page-destination or text-anchor).
    pub bookmarks: Vec<Bookmark>,
    /// SDK Phase 5 (v1 sweep) — `<CrossReferenceSource>` markers.
    /// Each names a CrossReferenceFormat + the destination.
    pub cross_references: Vec<CrossReference>,
    /// SDK Phase 5 (v1 sweep) — `<Topic>` definitions for the
    /// document's index. Flat list (the IDML schema's nested
    /// topics are flattened to one entry per Self for v1).
    pub index_topics: Vec<IndexTopic>,
    /// `<Section>` definitions, in document order. Each anchors at a
    /// `<Page>` (via `PageStart`) and carries the numbering style /
    /// start value / prefix InDesign uses to label that section's
    /// pages. The renderer consults these to compute a page label when
    /// the `<Page>` itself carries no baked `Name` (and they feed the
    /// auto-page-number marker reflow). When `Name` is present it stays
    /// authoritative.
    pub sections: Vec<Section>,
}

/// Page-numbering style for a `<Section>` (`PageNumberStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NumberingStyle {
    Arabic,
    UpperRoman,
    LowerRoman,
    UpperAlpha,
    LowerAlpha,
}

impl NumberingStyle {
    /// Map an IDML `PageNumberStyle` value. Unknown / unsupported
    /// styles (Kanji, Katakana, …) fall back to Arabic.
    pub fn from_idml(s: &str) -> Self {
        match s {
            "UpperRoman" => NumberingStyle::UpperRoman,
            "LowerRoman" => NumberingStyle::LowerRoman,
            "UpperLetters" => NumberingStyle::UpperAlpha,
            "LowerLetters" => NumberingStyle::LowerAlpha,
            _ => NumberingStyle::Arabic,
        }
    }

    /// Stable lower-camel wire name for the editor's section panel
    /// (panels.md gaps 9/10/19). Distinct from `format`, which
    /// renders a number; this names the *style* itself.
    pub fn as_str(self) -> &'static str {
        match self {
            NumberingStyle::Arabic => "arabic",
            NumberingStyle::UpperRoman => "upperRoman",
            NumberingStyle::LowerRoman => "lowerRoman",
            NumberingStyle::UpperAlpha => "upperAlpha",
            NumberingStyle::LowerAlpha => "lowerAlpha",
        }
    }

    /// Format a 1-based page number in this style. `0` (or anything
    /// the roman/alpha encoders can't represent) renders as the bare
    /// Arabic digits so the label is never empty.
    pub fn format(self, n: u32) -> String {
        match self {
            NumberingStyle::Arabic => n.to_string(),
            NumberingStyle::UpperRoman => to_roman(n).unwrap_or_else(|| n.to_string()),
            NumberingStyle::LowerRoman => to_roman(n)
                .map(|r| r.to_lowercase())
                .unwrap_or_else(|| n.to_string()),
            NumberingStyle::UpperAlpha => to_alpha(n).unwrap_or_else(|| n.to_string()),
            NumberingStyle::LowerAlpha => to_alpha(n)
                .map(|a| a.to_lowercase())
                .unwrap_or_else(|| n.to_string()),
        }
    }
}

/// Classic additive Roman numerals (1..=3999). Returns `None` outside
/// that range so callers fall back to Arabic.
fn to_roman(mut n: u32) -> Option<String> {
    if n == 0 || n > 3999 {
        return None;
    }
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, sym) in TABLE {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    Some(out)
}

/// Spreadsheet-style alphabetic numbering: 1→A, 26→Z, 27→AA, 28→AB.
/// `None` for 0.
fn to_alpha(mut n: u32) -> Option<String> {
    if n == 0 {
        return None;
    }
    let mut out = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    Some(String::from_utf8(out).expect("ascii"))
}

/// IDML `<Section>` definition.
#[derive(Debug, Clone, Serialize)]
pub struct Section {
    pub self_id: String,
    /// `PageStart` — the `Self` of the `<Page>` this section begins at.
    pub page_start: Option<String>,
    /// `ContinueNumbering="true"` — the section continues the running
    /// page number from the previous section rather than restarting.
    pub continue_numbering: bool,
    /// `PageNumberStart` — the number the section's first page takes
    /// when `continue_numbering` is false. Defaults to 1.
    pub start_at: Option<u32>,
    /// `PageNumberStyle`, defaulting to Arabic.
    pub numbering_style: NumberingStyle,
    /// `SectionPrefix` — prepended to the formatted number when
    /// `include_prefix` is set (e.g. `"A-"` → "A-1").
    pub section_prefix: Option<String>,
    /// `Marker` — the section marker text (chapter marker). Captured
    /// for round-trip / tooling; not used in the page label today.
    pub marker: Option<String>,
    /// `IncludeSectionPrefix="true"` — whether the prefix shows in the
    /// page label.
    pub include_prefix: bool,
}

/// IDML `<Article>` definition. Members reference stories via
/// `ArticleMember/ItemRef`.
#[derive(Debug, Clone, Serialize)]
pub struct Article {
    pub self_id: String,
    pub name: Option<String>,
    /// Member self_ids the article wraps (typically Story refs).
    pub members: Vec<String>,
}

/// IDML `<Hyperlink>` definition.
#[derive(Debug, Clone, Serialize)]
pub struct Hyperlink {
    pub self_id: String,
    pub name: Option<String>,
    /// Source ref (typically `HyperlinkTextSource/<id>`).
    pub source: Option<String>,
    /// Destination ref (URL / page / anchor). May be a
    /// `HyperlinkURLDestination`, `HyperlinkPageDestination`,
    /// or `HyperlinkTextDestination` self_id depending on the
    /// kind of hyperlink.
    pub destination: Option<String>,
}

/// IDML `<Bookmark>` definition.
#[derive(Debug, Clone, Serialize)]
pub struct Bookmark {
    pub self_id: String,
    pub name: Option<String>,
    /// Destination ref (`HyperlinkTextDestination/<id>` or
    /// `HyperlinkPageDestination/<id>`).
    pub destination: Option<String>,
}

/// IDML `<CrossReferenceSource>` marker.
#[derive(Debug, Clone, Serialize)]
pub struct CrossReference {
    pub self_id: String,
    pub name: Option<String>,
    /// `AppliedFormat` — ref to a `<CrossReferenceFormat>`.
    pub format: Option<String>,
    /// `Destination` — anchor / text-destination ref.
    pub destination: Option<String>,
}

/// IDML `<Topic>` definition for an index entry.
#[derive(Debug, Clone, Serialize)]
pub struct IndexTopic {
    pub self_id: String,
    pub name: Option<String>,
    /// Sort key (`SortOrder` attribute). Some IDMLs use this to
    /// override the alphabetical order.
    pub sort_order: Option<String>,
}

/// IDML `<TextVariable>` declaration. Parsed for completeness; the
/// rendered value comes from each `<TextVariableInstance>`'s
/// `ResultText` attribute, which the parser inlines into the host
/// run's text.
#[derive(Debug, Clone, Serialize)]
pub struct TextVariable {
    pub self_id: String,
    pub name: Option<String>,
    pub variable_type: Option<String>,
}

/// IDML `<Layer>` definition. Only the fields the renderer needs
/// today; visibility / printability decide whether items on that
/// layer are emitted at all.
#[derive(Debug, Clone, Serialize)]
pub struct Layer {
    pub self_id: String,
    pub name: Option<String>,
    /// `Visible="true|false"` — when false the layer is hidden in
    /// InDesign's view and PDF export skips it.
    pub visible: bool,
    /// `Locked="true|false"` — purely an editor concern; the renderer
    /// ignores it but we surface the field so future tooling can
    /// honour it.
    pub locked: bool,
    /// `Printable="true|false"` — InDesign's "Print Layer" checkbox.
    /// Non-printable layers are skipped during rendering.
    pub printable: bool,
    /// `Self` of the enclosing `<Layer>` when this layer is nested
    /// inside a layer group (folder) in InDesign's Layers panel.
    /// `None` for a top-level layer — the overwhelmingly common case,
    /// where every `<Layer>` is a self-closing peer. The render-time
    /// visibility / lock resolution ANDs/ORs a layer with its ancestors
    /// so an item on a visible child layer inside a hidden parent group
    /// is still hidden.
    pub parent_id: Option<String>,
}

/// Document-level color management config. Mirrors the attributes that
/// real InDesign exports carry on the `<Document>` element (CS6 / IDML
/// 8.0). Empty defaults match "no opinion" and let the renderer pick
/// a global fallback.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ColorSettings {
    /// `CMYKProfile` attribute, e.g. `"Coated FOGRA39 (ISO 12647-2:2004)"`.
    pub cmyk_profile: Option<String>,
    /// `RGBProfile` attribute, e.g. `"sRGB IEC61966-2.1"`.
    pub rgb_profile: Option<String>,
    /// `SolidColorIntent` — typically `"UseColorSettings"` (use the
    /// document's working spaces) or one of `Perceptual`,
    /// `Saturation`, `RelativeColorimetric`, `AbsoluteColorimetric`.
    pub solid_color_intent: Option<String>,
    /// `AfterBlendingIntent` — same value space as `solid_color_intent`.
    pub after_blending_intent: Option<String>,
    /// `DefaultImageIntent` — same value space.
    pub default_image_intent: Option<String>,
}

/// `<DocumentPreference>` page-setup values the renderer ignores but
/// print export needs. All offsets are points. NOTE the IDML quirk:
/// bleed spells "…InsideOrLeft/…OutsideOrRight" while slug flips the
/// word order to "SlugRightOrOutsideOffset" — that's faithful to the
/// spec, not a typo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct DocumentPreference {
    pub bleed_top: f32,
    pub bleed_bottom: f32,
    pub bleed_inside_or_left: f32,
    pub bleed_outside_or_right: f32,
    pub slug_top: f32,
    pub slug_bottom: f32,
    pub slug_inside_or_left: f32,
    pub slug_right_or_outside: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpreadRef {
    pub src: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoryRef {
    pub src: String,
}

impl DesignMap {
    /// Parse a `designmap.xml` byte slice.
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(false);

        let mut out = DesignMap::default();
        let mut buf = Vec::new();
        // Stack of currently-open `<Layer Self=...>` ids, so a nested
        // `<Layer>` (layer group / folder) records its parent. Only
        // `Event::Start` opens a scope; a self-closing `<Layer/>` (the
        // flat common case) records `parent_id` from the stack top but
        // doesn't push, keeping flat documents byte-identical.
        let mut layer_stack: Vec<String> = Vec::new();

        loop {
            let ev = reader.read_event_into(&mut buf)?;
            if let Event::End(ref e) = ev {
                if e.name().as_ref() == b"Layer" {
                    layer_stack.pop();
                }
            }
            let is_start = matches!(ev, Event::Start(_));
            match ev {
                Event::Start(e) | Event::Empty(e) => {
                    if e.name().as_ref() == b"Document" {
                        out.dom_version = attr(&e, b"DOMVersion");
                        out.color_settings = ColorSettings {
                            cmyk_profile: attr(&e, b"CMYKProfile"),
                            rgb_profile: attr(&e, b"RGBProfile"),
                            solid_color_intent: attr(&e, b"SolidColorIntent"),
                            after_blending_intent: attr(&e, b"AfterBlendingIntent"),
                            default_image_intent: attr(&e, b"DefaultImageIntent"),
                        };
                    }
                    if e.name().as_ref() == b"DocumentPreference" {
                        let f = |name: &[u8]| -> f32 {
                            attr(&e, name).and_then(|s| s.parse().ok()).unwrap_or(0.0)
                        };
                        out.document_preference = DocumentPreference {
                            bleed_top: f(b"DocumentBleedTopOffset"),
                            bleed_bottom: f(b"DocumentBleedBottomOffset"),
                            bleed_inside_or_left: f(b"DocumentBleedInsideOrLeftOffset"),
                            bleed_outside_or_right: f(b"DocumentBleedOutsideOrRightOffset"),
                            slug_top: f(b"SlugTopOffset"),
                            slug_bottom: f(b"SlugBottomOffset"),
                            slug_inside_or_left: f(b"SlugInsideOrLeftOffset"),
                            slug_right_or_outside: f(b"SlugRightOrOutsideOffset"),
                        };
                    }
                    if e.name().as_ref() == b"Layer" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.layers.push(Layer {
                                self_id: self_id.clone(),
                                name: attr(&e, b"Name"),
                                visible: attr(&e, b"Visible")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(true),
                                locked: attr(&e, b"Locked")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(false),
                                printable: attr(&e, b"Printable")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(true),
                                parent_id: layer_stack.last().cloned(),
                            });
                            // A non-self-closing <Layer> opens a group
                            // scope; its descendant layers inherit it.
                            if is_start {
                                layer_stack.push(self_id);
                            }
                        }
                    }
                    if e.name().as_ref() == b"TextVariable" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.text_variables.push(TextVariable {
                                self_id,
                                name: attr(&e, b"Name"),
                                variable_type: attr(&e, b"VariableType"),
                            });
                        }
                    }
                    if e.name().as_ref() == b"Section" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.sections.push(Section {
                                self_id,
                                page_start: attr(&e, b"PageStart"),
                                continue_numbering: attr(&e, b"ContinueNumbering")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(false),
                                start_at: attr(&e, b"PageNumberStart")
                                    .and_then(|s| s.parse().ok()),
                                numbering_style: attr(&e, b"PageNumberStyle")
                                    .map(|s| NumberingStyle::from_idml(&s))
                                    .unwrap_or(NumberingStyle::Arabic),
                                section_prefix: attr(&e, b"SectionPrefix"),
                                marker: attr(&e, b"Marker"),
                                include_prefix: attr(&e, b"IncludeSectionPrefix")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(false),
                            });
                        }
                    }
                    if e.name().as_ref() == b"Article" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            // `MemberItemRefs` is the typical attribute on
                            // a self-closing Article; nested
                            // <ArticleMember> children are flattened to
                            // their `ItemRef` attribute by a future polish.
                            let members = attr(&e, b"MemberItemRefs")
                                .map(|s| {
                                    s.split_whitespace()
                                        .map(|t| t.to_string())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            out.articles.push(Article {
                                self_id,
                                name: attr(&e, b"Name"),
                                members,
                            });
                        }
                    }
                    if e.name().as_ref() == b"Hyperlink" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.hyperlinks.push(Hyperlink {
                                self_id,
                                name: attr(&e, b"Name"),
                                source: attr(&e, b"Source"),
                                destination: attr(&e, b"DestinationUniqueKey")
                                    .or_else(|| attr(&e, b"Destination")),
                            });
                        }
                    }
                    if e.name().as_ref() == b"Bookmark" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.bookmarks.push(Bookmark {
                                self_id,
                                name: attr(&e, b"Name"),
                                destination: attr(&e, b"Destination"),
                            });
                        }
                    }
                    if e.name().as_ref() == b"CrossReferenceSource" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.cross_references.push(CrossReference {
                                self_id,
                                name: attr(&e, b"Name"),
                                format: attr(&e, b"AppliedFormat"),
                                destination: attr(&e, b"Destination"),
                            });
                        }
                    }
                    if e.name().as_ref() == b"Topic" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.index_topics.push(IndexTopic {
                                self_id,
                                name: attr(&e, b"Name"),
                                sort_order: attr(&e, b"SortOrder"),
                            });
                        }
                    }
                    let src = attr(&e, b"src");
                    match e.name().as_ref() {
                        b"idPkg:Spread" => {
                            if let Some(src) = src {
                                out.spreads.push(SpreadRef { src });
                            }
                        }
                        b"idPkg:Story" => {
                            if let Some(src) = src {
                                out.stories.push(StoryRef { src });
                            }
                        }
                        b"idPkg:MasterSpread" => {
                            if let Some(src) = src {
                                out.master_spreads.push(src);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:MasterSpread src="MasterSpreads/MasterSpread_ua.xml"/>
  <idPkg:Spread src="Spreads/Spread_u1.xml"/>
  <idPkg:Spread src="Spreads/Spread_u2.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#;

    #[test]
    fn parses_spread_and_story_manifest() {
        let dm = DesignMap::parse(SAMPLE).unwrap();
        assert_eq!(dm.spreads.len(), 2);
        assert_eq!(dm.stories.len(), 1);
        assert_eq!(dm.master_spreads.len(), 1);
        assert_eq!(dm.spreads[0].src, "Spreads/Spread_u1.xml");
        assert_eq!(dm.stories[0].src, "Stories/Story_u10.xml");
    }

    const LAYERS_SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Layer Self="ua" Name="Layer 1" Visible="true" Locked="false" Printable="true"/>
  <Layer Self="ub" Name="Guides" Visible="true" Locked="true" Printable="false"/>
  <Layer Self="uc" Name="Hidden" Visible="false" Printable="true"/>
  <Layer Self="ud" Name="Defaults"/>
</Document>"#;

    #[test]
    fn q17_layer_printable_attribute_round_trips() {
        let dm = DesignMap::parse(LAYERS_SAMPLE).unwrap();
        assert_eq!(dm.layers.len(), 4);
        let printable: Vec<bool> = dm.layers.iter().map(|l| l.printable).collect();
        assert_eq!(printable, vec![true, false, true, true]);
        let visible: Vec<bool> = dm.layers.iter().map(|l| l.visible).collect();
        assert_eq!(visible, vec![true, true, false, true]);
    }

    #[test]
    fn flat_layers_have_no_parent() {
        let dm = DesignMap::parse(LAYERS_SAMPLE).unwrap();
        assert!(dm.layers.iter().all(|l| l.parent_id.is_none()));
    }

    #[test]
    fn nested_layers_capture_parent() {
        // A layer group (folder): the non-self-closing <Layer> opens a
        // scope; its child <Layer> records the parent's Self. A sibling
        // top-level layer after the group closes is parentless again.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Layer Self="grp" Name="Group">
    <Layer Self="child1" Name="Child 1"/>
    <Layer Self="child2" Name="Child 2"/>
  </Layer>
  <Layer Self="peer" Name="Peer"/>
</Document>"#;
        let dm = DesignMap::parse(xml).unwrap();
        assert_eq!(dm.layers.len(), 4);
        let by_id = |id: &str| dm.layers.iter().find(|l| l.self_id == id).unwrap();
        assert_eq!(by_id("grp").parent_id, None);
        assert_eq!(by_id("child1").parent_id.as_deref(), Some("grp"));
        assert_eq!(by_id("child2").parent_id.as_deref(), Some("grp"));
        assert_eq!(by_id("peer").parent_id, None);
    }

    #[test]
    fn numbering_style_formats() {
        assert_eq!(NumberingStyle::Arabic.format(3), "3");
        assert_eq!(NumberingStyle::UpperRoman.format(4), "IV");
        assert_eq!(NumberingStyle::LowerRoman.format(3), "iii");
        assert_eq!(NumberingStyle::LowerRoman.format(9), "ix");
        assert_eq!(NumberingStyle::UpperAlpha.format(1), "A");
        assert_eq!(NumberingStyle::UpperAlpha.format(27), "AA");
        assert_eq!(NumberingStyle::LowerAlpha.format(2), "b");
        // 0 / out-of-range fall back to Arabic digits, never empty.
        assert_eq!(NumberingStyle::UpperRoman.format(0), "0");
    }

    #[test]
    fn parses_section_definitions() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Section Self="sec1" PageStart="page1" PageNumberStyle="LowerRoman"
           PageNumberStart="1" ContinueNumbering="false"/>
  <Section Self="sec2" PageStart="page3" PageNumberStyle="Arabic"
           SectionPrefix="A-" IncludeSectionPrefix="true" PageNumberStart="1"/>
</Document>"#;
        let dm = DesignMap::parse(xml).unwrap();
        assert_eq!(dm.sections.len(), 2);
        assert_eq!(dm.sections[0].page_start.as_deref(), Some("page1"));
        assert_eq!(dm.sections[0].numbering_style, NumberingStyle::LowerRoman);
        assert_eq!(dm.sections[0].start_at, Some(1));
        assert_eq!(dm.sections[1].numbering_style, NumberingStyle::Arabic);
        assert_eq!(dm.sections[1].section_prefix.as_deref(), Some("A-"));
        assert!(dm.sections[1].include_prefix);
    }

    #[test]
    fn reads_dom_version_when_present() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="18.5">
  <idPkg:Spread src="Spreads/Spread_u1.xml"/>
</Document>"#;
        let dm = DesignMap::parse(xml).unwrap();
        assert_eq!(dm.dom_version.as_deref(), Some("18.5"));
    }

    #[test]
    fn dom_version_absent_is_none() {
        // SAMPLE's <Document> carries no DOMVersion attribute.
        let dm = DesignMap::parse(SAMPLE).unwrap();
        assert_eq!(dm.dom_version, None);
    }
}

#[cfg(test)]
mod document_preference_tests {
    use super::*;

    #[test]
    fn parses_bleed_and_slug_offsets() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="18.5">
  <DocumentPreference PageWidth="595.2755905511812" PageHeight="841.8897637795276"
    DocumentBleedTopOffset="8.503937007874017"
    DocumentBleedBottomOffset="8.503937007874017"
    DocumentBleedInsideOrLeftOffset="8.503937007874017"
    DocumentBleedOutsideOrRightOffset="8.503937007874017"
    SlugTopOffset="14.173228346456694"
    SlugBottomOffset="0"
    SlugInsideOrLeftOffset="0"
    SlugRightOrOutsideOffset="0"/>
</Document>"#;
        let dm = DesignMap::parse(xml).expect("parse");
        let p = dm.document_preference;
        assert!((p.bleed_top - 8.5039).abs() < 1e-3);
        assert!((p.bleed_outside_or_right - 8.5039).abs() < 1e-3);
        assert!((p.slug_top - 14.1732).abs() < 1e-3);
        assert_eq!(p.slug_bottom, 0.0);
    }

    #[test]
    fn absent_element_defaults_to_zero() {
        let xml = br#"<?xml version="1.0"?><Document DOMVersion="18.5"/>"#;
        let dm = DesignMap::parse(xml).expect("parse");
        assert_eq!(dm.document_preference, DocumentPreference::default());
    }
}
