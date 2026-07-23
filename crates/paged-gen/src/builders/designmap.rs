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

//! `designmap.xml` — root manifest pointing at every Resources/,
//! MasterSpreads/, Spreads/, Stories/ entry the package contains.

use crate::xml::XmlBuilder;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);

pub struct DesignMap {
    /// Self-id of the document — typically `d`.
    pub self_id: String,
    pub master_spreads: Vec<String>,
    pub spreads: Vec<String>,
    pub stories: Vec<String>,
}

/// W1.4 — document-level marker resources (text variables, hyperlinks,
/// destinations) bolted onto a `DesignMap`. Kept separate so the
/// existing samples' 4-field `DesignMap` literals (and their emitted
/// designmaps) stay byte-identical; only the markers sample passes a
/// non-empty set via [`write_designmap_with_markers`].
#[derive(Default)]
pub struct MarkerResources {
    pub text_variables: Vec<TextVariableDef>,
    pub hyperlinks: Vec<HyperlinkDef>,
    pub hyperlink_destinations: Vec<HyperlinkDestinationDef>,
    /// W1.8 — optional document-level `<FootnoteOption>` carrying the
    /// separator rule and footnote spacing. `None` emits nothing, keeping
    /// every other sample's designmap byte-identical.
    pub footnote_option: Option<FootnoteOptionDef>,
    /// W1.18b — `<Section>` definitions for chapter numbering / page
    /// labels. Empty for samples that don't exercise sections.
    pub sections: Vec<SectionDef>,
    /// W4.8 — `<Bookmark>` definitions (named anchors → destination).
    /// Empty for samples that don't exercise navigation bookmarks.
    pub bookmarks: Vec<BookmarkDef>,
    /// W4.8 — `<Topic>` definitions for the document index. Empty for
    /// samples without an index.
    pub index_topics: Vec<IndexTopicDef>,
    /// `<Layer>` definitions for cross-shape z-ordering. Page items bind to
    /// a layer via `ItemLayer="<self_id>"`; the renderer z-sorts by layer
    /// order then XML order. Empty for samples without explicit layers.
    pub layers: Vec<LayerDef>,
}

/// A document-level `<Layer>` (z-order band). Mirrors
/// `idml_import::designmap::Layer`.
pub struct LayerDef {
    pub self_id: String,
    pub name: String,
}

/// W4.8 — a document-level `<Bookmark>` (a named anchor pointing at a
/// hyperlink destination). Mirrors `idml_import::designmap::Bookmark`.
pub struct BookmarkDef {
    pub self_id: String,
    pub name: String,
    /// Destination ref (`HyperlinkTextDestination/<id>` /
    /// `HyperlinkPageDestination/<id>`).
    pub destination: String,
}

/// W4.8 — a document-level `<Topic>` for the index. Mirrors
/// `idml_import::designmap::IndexTopic`.
pub struct IndexTopicDef {
    pub self_id: String,
    pub name: String,
}

/// W1.8 — a document-level `<FootnoteOption>` to emit. Attribute names
/// mirror InDesign's DOM `FootnoteOption` object. Only the subset the
/// renderer consumes is exposed; all fields are optional and omitted
/// from the XML when `None`.
#[derive(Default)]
pub struct FootnoteOptionDef {
    pub rule_on: Option<bool>,
    pub rule_color: Option<String>,
    pub rule_tint: Option<f32>,
    pub rule_line_weight: Option<f32>,
    pub rule_width: Option<f32>,
    pub rule_left_indent: Option<f32>,
    pub rule_offset: Option<f32>,
    pub separator_text: Option<String>,
    pub spacer: Option<f32>,
    pub space_between: Option<f32>,
}

/// W1.4 / W1.18 — a `<TextVariable>` to emit. `contents` populates the
/// `<TextVariablePreference Contents="...">` child for custom-text
/// variables; `date_format` the `Format` of a date variable;
/// `running_header_style` / `running_header_use` the pickup style +
/// First/LastOnPage choice of a running-header variable. The renderer
/// resolves each type itself.
#[derive(Default)]
pub struct TextVariableDef {
    pub self_id: String,
    pub name: String,
    pub variable_type: String,
    pub contents: Option<String>,
    /// W1.18a — `<TextVariablePreference Format="...">` for date types.
    pub date_format: Option<String>,
    /// W1.18c — running-header pickup style (an
    /// `AppliedParagraphStyle` ref).
    pub running_header_style: Option<String>,
    /// W1.18c — `Use="FirstOnPage|LastOnPage"`.
    pub running_header_use: Option<String>,
}

/// W1.4 — a `<Hyperlink>` (source span → destination resource).
pub struct HyperlinkDef {
    pub self_id: String,
    pub name: String,
    pub source: String,
    pub destination: String,
}

/// W1.4 / W1.19 — a hyperlink destination resource.
pub enum HyperlinkDestinationDef {
    /// `<HyperlinkURLDestination Self=... DestinationURL=...>`.
    Url { self_id: String, url: String },
    /// `<HyperlinkPageDestination Self=... DestinationPage=...>`.
    Page { self_id: String, page: String },
    /// W1.19 — `<HyperlinkTextDestination Self=... DestinationText=...>`
    /// — an in-story text anchor. The renderer resolves it to the page
    /// the destination story landed on (post-layout), so a cross-
    /// reference "see page N" re-resolves when the story moves.
    TextAnchor { self_id: String, story: String },
}

/// W1.18b — a `<Section>` definition for chapter numbering.
#[derive(Default)]
pub struct SectionDef {
    pub self_id: String,
    /// `PageStart` — the `<Page Self>` the section begins at.
    pub page_start: String,
    /// `PageNumberStyle` (Arabic / UpperRoman / …).
    pub number_style: Option<String>,
    /// `PageNumberStart` — the section's first number.
    pub start_at: Option<u32>,
    /// `Marker` — an explicit chapter label (wins verbatim).
    pub marker: Option<String>,
}

pub fn write_designmap(dm: &DesignMap) -> Vec<u8> {
    write_designmap_with_markers(dm, &MarkerResources::default())
}

/// As [`write_designmap`] but also emits document-level marker
/// resources (W1.4). When `markers` is empty the output is identical
/// to `write_designmap`.
pub fn write_designmap_with_markers(dm: &DesignMap, markers: &MarkerResources) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    // <?aid?> processing instruction. InDesign's IDML reader rejects
    // documents without it as "format not supported" — even when the
    // DOMVersion is correct and the ZIP is well-formed. Fields:
    //   style="50"          IDML format style
    //   type="document"     top-level document (vs snippet/icml)
    //   readerVersion="6.0" minimum IDML reader version (CS6+)
    //   featureSet="257"    feature bitmask the document uses
    //   product="20.0(32)"  exporter product version
    b.write_pi(
        "aid",
        r#"style="50" type="document" readerVersion="6.0" featureSet="257" product="20.0(32)""#,
    );
    b.start(
        "Document",
        &[
            PKG_NS,
            ("DOMVersion", "20.0"),
            ("Self", dm.self_id.as_str()),
            ("StoryList", &dm.stories.join(" ")),
            ("Name", "generated.indd"),
            // ColorSettings — match InDesign's default ICC profiles so
            // the inspect binary picks up the host's installed Adobe
            // ICC profiles (FOGRA39 / sRGB) and routes CMYK swatches
            // through lcms2. Without these attributes the renderer
            // falls back to naive `(1-cv)*(1-kv)` math which produces
            // pure black for `Color/Black` (CMYK K=100) instead of the
            // ~(35,31,32) sRGB warm dark gray that real K=100 ink
            // prints to. The 0/20 → high-pass-rate jump on
            // `geometry.idml` traces directly to this declaration.
            ("CMYKProfile", "Coated FOGRA39 (ISO 12647-2:2004)"),
            ("RGBProfile", "sRGB IEC61966-2.1"),
            ("SolidColorIntent", "UseColorSettings"),
            ("AfterBlendingIntent", "UseColorSettings"),
            ("DefaultImageIntent", "UseColorSettings"),
        ],
    );
    // <Layer> definitions come first inside <Document> (InDesign order).
    // Empty for every existing sample, so their designmaps stay identical.
    for l in &markers.layers {
        b.start(
            "Layer",
            &[
                ("Self", l.self_id.as_str()),
                ("Name", l.name.as_str()),
                ("Visible", "true"),
                ("Locked", "false"),
            ],
        );
        b.end("Layer");
    }
    // W1.4 — marker resources (text variables, hyperlinks,
    // destinations). Emitted before the idPkg refs, matching where
    // InDesign serialises document-level resources. Skipped entirely
    // when empty so existing samples' designmaps stay byte-identical.
    for v in &markers.text_variables {
        b.start(
            "TextVariable",
            &[
                ("Self", v.self_id.as_str()),
                ("Name", v.name.as_str()),
                ("VariableType", v.variable_type.as_str()),
            ],
        );
        // Real exports nest a <TextVariablePreference> carrying the
        // type-specific payload: `Contents` for custom text, `Format`
        // for dates, `AppliedParagraphStyle` + `Use` for running
        // headers. Emit only the attributes this variable sets so the
        // existing custom/page-count call sites stay byte-identical.
        let contents = v.contents.as_deref().unwrap_or("");
        let mut attrs: Vec<(&str, &str)> = vec![("Contents", contents)];
        if let Some(fmt) = v.date_format.as_deref() {
            attrs.push(("Format", fmt));
        }
        if let Some(style) = v.running_header_style.as_deref() {
            attrs.push(("AppliedParagraphStyle", style));
        }
        if let Some(use_v) = v.running_header_use.as_deref() {
            attrs.push(("Use", use_v));
        }
        b.empty("TextVariablePreference", &attrs);
        b.end("TextVariable");
    }
    for sec in &markers.sections {
        let mut attrs: Vec<(&str, &str)> = vec![
            ("Self", sec.self_id.as_str()),
            ("PageStart", sec.page_start.as_str()),
        ];
        let start_buf;
        if let Some(start) = sec.start_at {
            start_buf = start.to_string();
            attrs.push(("PageNumberStart", &start_buf));
        }
        if let Some(style) = sec.number_style.as_deref() {
            attrs.push(("PageNumberStyle", style));
        }
        if let Some(marker) = sec.marker.as_deref() {
            attrs.push(("Marker", marker));
        }
        b.empty("Section", &attrs);
    }
    for d in &markers.hyperlink_destinations {
        match d {
            HyperlinkDestinationDef::Url { self_id, url } => {
                b.empty(
                    "HyperlinkURLDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", url.as_str()),
                        ("DestinationURL", url.as_str()),
                        ("Hidden", "false"),
                    ],
                );
            }
            HyperlinkDestinationDef::Page { self_id, page } => {
                b.empty(
                    "HyperlinkPageDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", self_id.as_str()),
                        ("DestinationPage", page.as_str()),
                        ("DestinationPageSetting", "FitVisible"),
                        ("Hidden", "false"),
                    ],
                );
            }
            HyperlinkDestinationDef::TextAnchor { self_id, story } => {
                b.empty(
                    "HyperlinkTextDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", self_id.as_str()),
                        ("DestinationText", story.as_str()),
                        ("Hidden", "false"),
                    ],
                );
            }
        }
    }
    for h in &markers.hyperlinks {
        b.empty(
            "Hyperlink",
            &[
                ("Self", h.self_id.as_str()),
                ("Name", h.name.as_str()),
                ("Source", h.source.as_str()),
                ("Destination", h.destination.as_str()),
                ("Visible", "true"),
                ("Hidden", "false"),
            ],
        );
    }
    // W4.8 — document-level `<Bookmark>` anchors. InDesign nests these
    // in a `<RootBookmark>` tree; the parser keys each `<Bookmark>` by
    // `Self` regardless of wrapper, so a flat emission round-trips.
    for bm in &markers.bookmarks {
        b.empty(
            "Bookmark",
            &[
                ("Self", bm.self_id.as_str()),
                ("Name", bm.name.as_str()),
                ("Destination", bm.destination.as_str()),
            ],
        );
    }
    // W4.8 — document-level `<Topic>` definitions for the index.
    for t in &markers.index_topics {
        b.empty(
            "Topic",
            &[("Self", t.self_id.as_str()), ("Name", t.name.as_str())],
        );
    }
    // W1.8 — document-level footnote separator/spacing settings. InDesign
    // wraps the `<FootnoteOption>` in a `<RootFootnoteStory>`; we mirror
    // that. Each attribute is omitted when its field is `None`.
    if let Some(fo) = markers.footnote_option.as_ref() {
        let mut attrs: Vec<(&str, String)> = Vec::new();
        if let Some(v) = fo.rule_on {
            attrs.push(("RuleOn", v.to_string()));
        }
        if let Some(v) = fo.rule_color.as_deref() {
            attrs.push(("RuleColor", v.to_string()));
        }
        if let Some(v) = fo.rule_tint {
            attrs.push(("RuleTint", crate::xml::format_f32(v)));
        }
        if let Some(v) = fo.rule_line_weight {
            attrs.push(("RuleLineWeight", crate::xml::format_f32(v)));
        }
        if let Some(v) = fo.rule_width {
            attrs.push(("RuleWidth", crate::xml::format_f32(v)));
        }
        if let Some(v) = fo.rule_left_indent {
            attrs.push(("RuleLeftIndent", crate::xml::format_f32(v)));
        }
        if let Some(v) = fo.rule_offset {
            attrs.push(("RuleOffset", crate::xml::format_f32(v)));
        }
        if let Some(v) = fo.separator_text.as_deref() {
            attrs.push(("SeparatorText", v.to_string()));
        }
        if let Some(v) = fo.spacer {
            attrs.push(("Spacer", crate::xml::format_f32(v)));
        }
        if let Some(v) = fo.space_between {
            attrs.push(("SpaceBetween", crate::xml::format_f32(v)));
        }
        let attr_refs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        b.start("RootFootnoteStory", &[]);
        b.empty("FootnoteOption", &attr_refs);
        b.end("RootFootnoteStory");
    }
    b.empty("idPkg:Graphic", &[("src", "Resources/Graphic.xml")]);
    b.empty("idPkg:Fonts", &[("src", "Resources/Fonts.xml")]);
    b.empty("idPkg:Styles", &[("src", "Resources/Styles.xml")]);
    b.empty("idPkg:Preferences", &[("src", "Resources/Preferences.xml")]);
    b.empty("idPkg:Tags", &[("src", "XML/Tags.xml")]);
    for ms in &dm.master_spreads {
        b.empty(
            "idPkg:MasterSpread",
            &[("src", &format!("MasterSpreads/MasterSpread_{ms}.xml"))],
        );
    }
    for s in &dm.spreads {
        b.empty(
            "idPkg:Spread",
            &[("src", &format!("Spreads/Spread_{s}.xml"))],
        );
    }
    for s in &dm.stories {
        b.empty("idPkg:Story", &[("src", &format!("Stories/Story_{s}.xml"))]);
    }
    b.empty("idPkg:BackingStory", &[("src", "XML/BackingStory.xml")]);
    b.end("Document");
    b.into_bytes()
}
