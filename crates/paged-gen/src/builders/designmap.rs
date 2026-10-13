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

use crate::builders::resources::{ConditionSetSpec, ConditionSpec};
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
    /// Conditional-text definitions, written as `<Document>` children in
    /// InDesign's spelling (see [`ConditionSpec`]). Empty for samples
    /// without conditional text.
    pub conditions: Vec<ConditionSpec>,
    pub condition_sets: Vec<ConditionSetSpec>,
    /// Named `<NumberingList>`s (see [`NumberingListSpec`]). Empty for
    /// samples whose lists all use the default list.
    pub numbering_lists: Vec<NumberingListSpec>,
    /// Swatch groups (see [`ColorGroupSpec`]). Empty for samples without
    /// colour groups.
    pub color_groups: Vec<ColorGroupSpec>,
}

/// W1.22 — a named `<NumberingList>`: paragraphs that apply it
/// (`AppliedNumberingList`) share one counter, and
/// `ContinueNumbersAcrossStories` decides whether it runs on from one
/// story into the next.
///
/// InDesign keeps numbering lists in `designmap.xml`, right after the
/// `<idPkg:Styles>` reference (271 of the 305 census documents; its own
/// export of a list added through the DOM, 2026-10-12). This generator
/// used to write them into `Resources/Styles.xml` inside an invented
/// `<RootNumberingListGroup>`, which InDesign does not read: the
/// references numbered every list-bound paragraph in InDesign's default
/// list, so story B restarted at 1 where the fixture continued at 3.
pub struct NumberingListSpec {
    /// e.g. `"NumberingList/Shared"` — the value `AppliedNumberingList`
    /// names.
    pub self_id: String,
    pub name: String,
    pub continue_across_stories: bool,
    pub continue_across_documents: bool,
}

/// W4.7 — a named swatch group, written as InDesign writes it:
///
/// ```xml
/// <ColorGroup Self="ColorGroup/Brand" Name="Brand" IsRootColorGroup="false">
///   <ColorGroupSwatch Self="…" SwatchItemRef="Color/…"/>
/// </ColorGroup>
/// ```
///
/// in `designmap.xml`, after the stories and the navigation block (265
/// of the 305 census documents; its own export, 2026-10-12). This
/// generator used to write a `<ColorGroup ColorGroupSwatches="…">` into
/// `Resources/Graphic.xml`, a spelling InDesign never writes: it made the
/// group by name and left it EMPTY, every member staying in the root
/// group. The root group (InDesign's container for every ungrouped
/// swatch) is left for InDesign to build on open, which it does.
pub struct ColorGroupSpec {
    pub self_id: String,
    pub name: String,
    /// The member swatches' `Self` refs (`Color/<id>`), in order.
    pub members: Vec<String>,
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
    /// The preference element InDesign itself writes for this type
    /// (`<MatchParagraphStylePreference>`, `<PageNumberVariablePreference>`,
    /// …). When set it REPLACES the `<TextVariablePreference>` above, a
    /// spelling InDesign never writes and ignores on open (measured,
    /// InDesign 20.0.1: the `variables` reference printed nothing for a
    /// running header and a date written that way — ADR 033).
    /// `None` keeps the older samples' bytes.
    pub preference: Option<VariablePreference>,
}

/// A `<TextVariable>`'s type-specific child as InDesign 20.0.1 serialises
/// it (read back from its own IDML export, 2026-10-01). Attribute order
/// follows InDesign's.
pub enum VariablePreference {
    /// `MatchParagraphStyleType` / `MatchCharacterStyleType` (running
    /// headers). `style` is the `ParagraphStyle/…` or `CharacterStyle/…`
    /// ref; `search_strategy` `FirstOnPage` / `LastOnPage`; `change_case`
    /// `None` / `Uppercase` / `Lowercase` / `Titlecase` / `Sentencecase`
    /// (InDesign's spelling, lower-case `c`).
    MatchStyle {
        character: bool,
        style: String,
        search_strategy: &'static str,
        change_case: &'static str,
        delete_end_punctuation: bool,
    },
    /// `LastPageNumberType`: `scope` `DocumentScope` / `SectionScope`,
    /// `format` `Current` or a numbering style (`UpperRoman`).
    PageNumber {
        scope: &'static str,
        format: &'static str,
    },
    /// `ChapterNumberType`: `format` `Current` or a numbering format.
    ChapterNumber { format: &'static str },
    /// `FileNameType`.
    FileName {
        include_path: bool,
        include_extension: bool,
    },
    /// The three date types: an ICU-style `format`.
    Date { format: String },
    /// `CustomTextType`: the literal, as a typed `<Properties>` child.
    CustomText { contents: String },
}

fn write_variable_preference(b: &mut XmlBuilder, pref: &VariablePreference) {
    let bool_str = |v: bool| if v { "true" } else { "false" };
    match pref {
        VariablePreference::MatchStyle {
            character,
            style,
            search_strategy,
            change_case,
            delete_end_punctuation,
        } => {
            let (element, style_attr) = if *character {
                ("MatchCharacterStylePreference", "AppliedCharacterStyle")
            } else {
                ("MatchParagraphStylePreference", "AppliedParagraphStyle")
            };
            b.empty(
                element,
                &[
                    ("TextBefore", ""),
                    ("TextAfter", ""),
                    (style_attr, style.as_str()),
                    ("SearchStrategy", search_strategy),
                    ("ChangeCase", change_case),
                    ("DeleteEndPunctuation", bool_str(*delete_end_punctuation)),
                ],
            );
        }
        VariablePreference::PageNumber { scope, format } => b.empty(
            "PageNumberVariablePreference",
            &[
                ("TextBefore", ""),
                ("Format", format),
                ("TextAfter", ""),
                ("Scope", scope),
            ],
        ),
        VariablePreference::ChapterNumber { format } => b.empty(
            "ChapterNumberVariablePreference",
            &[("TextBefore", ""), ("Format", format), ("TextAfter", "")],
        ),
        VariablePreference::FileName {
            include_path,
            include_extension,
        } => b.empty(
            "FileNameVariablePreference",
            &[
                ("TextBefore", ""),
                ("IncludePath", bool_str(*include_path)),
                ("IncludeExtension", bool_str(*include_extension)),
                ("TextAfter", ""),
            ],
        ),
        VariablePreference::Date { format } => b.empty(
            "DateVariablePreference",
            &[
                ("TextBefore", ""),
                ("Format", format.as_str()),
                ("TextAfter", ""),
            ],
        ),
        VariablePreference::CustomText { contents } => {
            b.start("CustomTextVariablePreference", &[]);
            b.start("Properties", &[]);
            b.start("Contents", &[("type", "string")]);
            b.text(contents);
            b.end("Contents");
            b.end("Properties");
            b.end("CustomTextVariablePreference");
        }
    }
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
    /// `Marker` — the section marker text (`<?ACE 19?>` prints it).
    pub marker: Option<String>,
    /// `ContinueNumbering` — written only when set. InDesign reads an
    /// absent attribute as continuing, so a restart needs `Some(false)`.
    pub continue_numbering: Option<bool>,
    /// `Length` — the section's page count, which InDesign writes on
    /// every section. Written only when set.
    pub length: Option<u32>,
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
    // Conditional text — `<Document>` children after the layers, in the
    // spelling InDesign 20.0.1 was measured to read (see `ConditionSpec`).
    for c in &markers.conditions {
        let visible = if c.visible { "true" } else { "false" };
        b.start(
            "Condition",
            &[
                ("Self", c.self_id),
                ("Name", c.name),
                ("IndicatorMethod", "UseHighlight"),
                ("Visible", visible),
            ],
        );
        b.start("Properties", &[]);
        b.start("IndicatorColor", &[("type", "enumeration")]);
        b.text(c.indicator_color.unwrap_or("Red"));
        b.end("IndicatorColor");
        b.end("Properties");
        b.end("Condition");
    }
    for set in &markers.condition_sets {
        b.start("ConditionSet", &[("Self", set.self_id), ("Name", set.name)]);
        b.start("Properties", &[]);
        b.start("SetConditions", &[]);
        for member in set.conditions {
            b.empty(
                "VisibilityPair",
                &[("Condition", member), ("Visibility", "true")],
            );
        }
        b.end("SetConditions");
        b.end("Properties");
        b.end("ConditionSet");
    }
    if !(markers.conditions.is_empty() && markers.condition_sets.is_empty()) {
        b.empty(
            "ConditionalTextPreference",
            &[
                ("ShowConditionIndicators", "ShowIndicators"),
                ("ActiveConditionSet", "n"),
            ],
        );
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
        if let Some(pref) = v.preference.as_ref() {
            write_variable_preference(&mut b, pref);
            b.end("TextVariable");
            continue;
        }
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
    b.empty("idPkg:Graphic", &[("src", "Resources/Graphic.xml")]);
    b.empty("idPkg:Fonts", &[("src", "Resources/Fonts.xml")]);
    b.empty("idPkg:Styles", &[("src", "Resources/Styles.xml")]);
    // Numbering lists, where InDesign writes them: after the styles
    // reference, before the preferences one.
    for l in &markers.numbering_lists {
        let bool_str = |v: bool| if v { "true" } else { "false" };
        b.empty(
            "NumberingList",
            &[
                ("Self", l.self_id.as_str()),
                ("Name", l.name.as_str()),
                (
                    "ContinueNumbersAcrossStories",
                    bool_str(l.continue_across_stories),
                ),
                (
                    "ContinueNumbersAcrossDocuments",
                    bool_str(l.continue_across_documents),
                ),
            ],
        );
    }
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
    // W1.18b — `<Section>` definitions, after the spread includes, the
    // numbering style as InDesign's typed Properties child (a union type
    // — it may also name a custom list — so never an attribute).
    for sec in &markers.sections {
        // InDesign's own attribute order, and the order is LOAD-BEARING:
        // its reader applies `PageNumberStart` only when
        // `ContinueNumbering="false"` has already been read. Written the
        // other way round, InDesign 20.0.1 opened the section, kept its
        // marker, and numbered straight through (measured 2026-10-01 on
        // `variables`: "Page 3 of 5" where the restart reads "Page 1 of 3").
        let mut attrs: Vec<(&str, &str)> = vec![("Self", sec.self_id.as_str())];
        let length_buf;
        if let Some(length) = sec.length {
            length_buf = length.to_string();
            attrs.push(("Length", &length_buf));
        }
        if let Some(cont) = sec.continue_numbering {
            attrs.push(("ContinueNumbering", if cont { "true" } else { "false" }));
        }
        let start_buf;
        if let Some(start) = sec.start_at {
            start_buf = start.to_string();
            attrs.push(("PageNumberStart", &start_buf));
        }
        if let Some(marker) = sec.marker.as_deref() {
            attrs.push(("Marker", marker));
        }
        attrs.push(("PageStart", sec.page_start.as_str()));
        match sec.number_style.as_deref() {
            Some(style) => {
                b.start("Section", &attrs);
                b.start("Properties", &[]);
                b.start("PageNumberStyle", &[("type", "enumeration")]);
                b.text(style);
                b.end("PageNumberStyle");
                b.end("Properties");
                b.end("Section");
            }
            None => b.empty("Section", &attrs),
        }
    }
    for s in &dm.stories {
        b.empty("idPkg:Story", &[("src", &format!("Stories/Story_{s}.xml"))]);
    }
    // W1.4 / W4.8 — the navigation block: destinations, hyperlinks,
    // bookmarks, index topics. AFTER the story includes, in the spelling
    // InDesign 20.0.1 was measured to read (2026-09-05): our block used to
    // sit before the stories and InDesign bound nothing (0 hyperlinks);
    // `<Hyperlink>` names its destination as a typed Properties child
    // (the `Destination` attribute is ignored, and next to a
    // `DestinationUniqueKey` makes the file unopenable). Keys are
    // allocated 1..n over the destinations.
    let mut keys: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, d) in markers.hyperlink_destinations.iter().enumerate() {
        let id = match d {
            HyperlinkDestinationDef::Url { self_id, .. }
            | HyperlinkDestinationDef::Page { self_id, .. }
            | HyperlinkDestinationDef::TextAnchor { self_id, .. } => self_id.as_str(),
        };
        keys.insert(id, i + 1);
    }
    for d in &markers.hyperlink_destinations {
        match d {
            HyperlinkDestinationDef::Url { self_id, url } => {
                let key = keys[self_id.as_str()].to_string();
                b.empty(
                    "HyperlinkURLDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", url.as_str()),
                        ("DestinationURL", url.as_str()),
                        ("Hidden", "false"),
                        ("DestinationUniqueKey", key.as_str()),
                    ],
                );
            }
            HyperlinkDestinationDef::Page { self_id, page } => {
                let key = keys[self_id.as_str()].to_string();
                b.empty(
                    "HyperlinkPageDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", self_id.as_str()),
                        ("NameManually", "true"),
                        ("DestinationPage", page.as_str()),
                        ("ViewSetting", "Fixed"),
                        ("ViewPercentage", "100"),
                        ("Hidden", "false"),
                        ("DestinationUniqueKey", key.as_str()),
                    ],
                );
            }
            // ◪ InDesign keeps a text destination as an INLINE marker in
            // its story, not as a designmap element; the story builders
            // have no hook for that yet, so the designmap spelling stays
            // (our importer reads it; `idml-export` normalises it to the
            // inline marker on export).
            HyperlinkDestinationDef::TextAnchor { self_id, story } => {
                let key = keys[self_id.as_str()].to_string();
                b.empty(
                    "HyperlinkTextDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", self_id.as_str()),
                        ("DestinationText", story.as_str()),
                        ("Hidden", "false"),
                        ("DestinationUniqueKey", key.as_str()),
                    ],
                );
            }
        }
    }
    for h in &markers.hyperlinks {
        let key = keys.get(h.destination.as_str()).map(|k| k.to_string());
        let mut attrs: Vec<(&str, &str)> = vec![
            ("Self", h.self_id.as_str()),
            ("Name", h.name.as_str()),
            ("Source", h.source.as_str()),
            ("Visible", "false"),
            ("Highlight", "None"),
            ("Width", "Thin"),
            ("BorderStyle", "Solid"),
            ("Hidden", "false"),
        ];
        if let Some(k) = key.as_deref() {
            attrs.push(("DestinationUniqueKey", k));
        }
        b.start("Hyperlink", &attrs);
        b.start("Properties", &[]);
        b.start("BorderColor", &[("type", "enumeration")]);
        b.text("Black");
        b.end("BorderColor");
        b.start("Destination", &[("type", "object")]);
        b.text(h.destination.as_str());
        b.end("Destination");
        b.end("Properties");
        b.end("Hyperlink");
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
    // Swatch groups, after the navigation block as in InDesign's files.
    // A member's `ColorGroupSwatch` Self is the group's plus a running
    // index, the shape InDesign mints (`u12ColorGroupSwatch0`).
    for g in &markers.color_groups {
        b.start(
            "ColorGroup",
            &[
                ("Self", g.self_id.as_str()),
                ("Name", g.name.as_str()),
                ("IsRootColorGroup", "false"),
            ],
        );
        for (i, member) in g.members.iter().enumerate() {
            let member_self = format!("{}ColorGroupSwatch{i:x}", g.self_id);
            b.empty(
                "ColorGroupSwatch",
                &[
                    ("Self", member_self.as_str()),
                    ("SwatchItemRef", member.as_str()),
                ],
            );
        }
        b.end("ColorGroup");
    }
    b.empty("idPkg:BackingStory", &[("src", "XML/BackingStory.xml")]);
    b.end("Document");
    b.into_bytes()
}
