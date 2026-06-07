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
    /// `Name` attribute on the root `<Document>` element — the source
    /// `.indd` file name (e.g. `"generated.indd"`). W1.4: feeds the
    /// `FileNameVariable` text variable. `None` when absent.
    pub document_name: Option<String>,
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
    /// W1.8 — document-level `<FootnoteOption>` settings (separator
    /// rule + footnote spacing). Default (all-`None`, `present=false`)
    /// when the element is absent; the renderer then applies InDesign's
    /// built-in defaults. See [`FootnoteOptions`].
    pub footnote_options: FootnoteOptions,
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
    /// W1.4 — `<HyperlinkURLDestination>` / `<HyperlinkPageDestination>`
    /// / `<HyperlinkTextDestination>` resources, keyed by `Self`. A
    /// `<Hyperlink Destination="...">` resolves through this table to a
    /// concrete URL or page target.
    pub hyperlink_destinations: Vec<HyperlinkDestination>,
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

/// W1.4 — a hyperlink destination resource. IDML declares three
/// flavours at the document level, each referenced from a
/// `<Hyperlink Destination="...">`:
///
/// - `HyperlinkURLDestination` carries an external `DestinationURL`.
/// - `HyperlinkPageDestination` points at a `<Page>` by `Self`
///   (`DestinationPage`), optionally with a zoom setting.
/// - `HyperlinkTextDestination` is an in-story text anchor whose
///   `DestinationText` references the hosting story; the renderer
///   resolves it to the page the anchor lands on (best-effort: the
///   first page hosting that story).
#[derive(Debug, Clone, Serialize)]
pub struct HyperlinkDestination {
    pub self_id: String,
    pub kind: HyperlinkDestinationKind,
}

/// The destination flavour + its payload.
#[derive(Debug, Clone, Serialize)]
pub enum HyperlinkDestinationKind {
    /// External URL (e.g. `https://paged.media`).
    Url(String),
    /// `DestinationPage` — the `Self` id of the target `<Page>`.
    Page(String),
    /// `DestinationText` — the `Self` id of the target text anchor /
    /// story. Resolved to a page index downstream.
    TextAnchor(String),
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

/// IDML `<TextVariable>` declaration. W1.4: the renderer resolves the
/// value per `variable_type` at emit time (falling back to each
/// instance's baked `ResultText` when the type's inputs aren't
/// modelled). The `<TextVariablePreference>` child carries the
/// type-specific payload — the literal contents of a custom variable,
/// the date `Format` string, and the surrounding `TextBefore` /
/// `TextAfter` decoration.
#[derive(Debug, Clone, Serialize)]
pub struct TextVariable {
    pub self_id: String,
    pub name: Option<String>,
    /// `VariableType` — e.g. `CustomTextType`, `FileNameType`,
    /// `PageCountType`, `CreationDateType`, `ModificationDateType`,
    /// `OutputDateType`, `ChapterNumberType`, `RunningHeaderType`.
    pub variable_type: Option<String>,
    /// `<TextVariablePreference Contents="...">` — the literal value of
    /// a `CustomTextType` variable (verbatim). `None` for other types.
    pub contents: Option<String>,
    /// `<TextVariablePreference Format="...">` — the date/time format
    /// pattern for the date variable types (InDesign/ICU-style tokens).
    /// `None` when absent.
    pub date_format: Option<String>,
    /// `<TextVariablePreference TextBefore="...">` decoration prepended
    /// to the resolved value.
    pub text_before: Option<String>,
    /// `<TextVariablePreference TextAfter="...">` decoration appended to
    /// the resolved value.
    pub text_after: Option<String>,
    /// W1.18c — `<TextVariablePreference AppliedParagraphStyle="...">`
    /// (or `AppliedCharacterStyle`) for `RunningHeaderType` variables:
    /// the style whose nearest on-page occurrence supplies the header
    /// text. `None` for non-header variables.
    pub running_header_style: Option<String>,
    /// W1.18c — `<TextVariablePreference Use="FirstOnPage|LastOnPage">`
    /// — which on-page match a running header picks up. `None` ⇒
    /// FirstOnPage (InDesign's default).
    pub running_header_use: Option<String>,
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

/// `<FootnoteOption>` — document-level footnote separator + spacing
/// settings. In IDML this element is serialised inside the document's
/// `<RootFootnoteStory>` (or directly under `<Document>`); its attribute
/// names mirror the InDesign DOM `FootnoteOption` object exactly. Only
/// the subset the renderer consumes is modelled here.
///
/// W1.8 — the renderer draws a separator rule above each frame's
/// footnote pool when `rule_on` is true, using `rule_*`. The `spacer`
/// (minimum gap between body and first footnote) and `space_between`
/// (gap between footnotes) feed the pool layout's vertical metrics.
///
/// `None` everywhere is the absent-element default; the renderer then
/// falls back to InDesign's own defaults (`rule_on = true`, a 0.5pt
/// black rule 50% of the column wide). See [`FootnoteOptions::is_default`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FootnoteOptions {
    /// True when the element was present in the designmap. When false
    /// the struct is all-`None` and the renderer applies its built-in
    /// defaults; we keep the flag so "rule explicitly off" (`rule_on =
    /// Some(false)`) is distinguishable from "no FootnoteOption at all".
    pub present: bool,
    /// `RuleOn` — draw the separator rule above the first footnote.
    pub rule_on: Option<bool>,
    /// `RuleColor` — swatch id (`Color/...`) for the rule stroke.
    pub rule_color: Option<String>,
    /// `RuleTint` — tint percent (0–100) of the rule colour.
    pub rule_tint: Option<f32>,
    /// `RuleLineWeight` — stroke weight of the rule, in points.
    pub rule_line_weight: Option<f32>,
    /// `RuleWidth` — length of the rule, in points (the drawn segment;
    /// InDesign measures it from `rule_left_indent`).
    pub rule_width: Option<f32>,
    /// `RuleLeftIndent` — left inset of the rule from the column edge,
    /// in points.
    pub rule_left_indent: Option<f32>,
    /// `RuleOffset` — vertical offset of the rule above the first
    /// footnote's baseline-anchored top, in points.
    pub rule_offset: Option<f32>,
    /// `SeparatorText` — string between the footnote marker number and
    /// its text (e.g. `"\t"`). The renderer expands `^t`/`^m` markers.
    pub separator_text: Option<String>,
    /// `Spacer` — minimum vertical space between the text-column bottom
    /// and the first footnote, in points.
    pub spacer: Option<f32>,
    /// `SpaceBetween` — vertical space between consecutive footnotes,
    /// in points.
    pub space_between: Option<f32>,
}

impl FootnoteOptions {
    /// True when no `<FootnoteOption>` was parsed (or it carried no
    /// recognised attributes). Lets the renderer cheaply skip the
    /// separator/spacing machinery for the overwhelmingly common case
    /// of a document with no customised footnote settings.
    pub fn is_default(&self) -> bool {
        !self.present
    }

    /// Effective `rule_on`, applying InDesign's default (rule ON) when
    /// the document didn't say.
    pub fn rule_on_effective(&self) -> bool {
        self.rule_on.unwrap_or(true)
    }
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
        // W1.4 — the `<TextVariable>` currently being parsed (the
        // wrapping form parks here so its `<TextVariablePreference>`
        // child can fold in before `</TextVariable>` pushes it).
        let mut current_text_variable: Option<TextVariable> = None;

        loop {
            let ev = reader.read_event_into(&mut buf)?;
            if let Event::End(ref e) = ev {
                if e.name().as_ref() == b"Layer" {
                    layer_stack.pop();
                }
                if e.name().as_ref() == b"TextVariable" {
                    if let Some(var) = current_text_variable.take() {
                        out.text_variables.push(var);
                    }
                }
            }
            let is_start = matches!(ev, Event::Start(_));
            match ev {
                Event::Start(e) | Event::Empty(e) => {
                    if e.name().as_ref() == b"Document" {
                        out.dom_version = attr(&e, b"DOMVersion");
                        out.document_name = attr(&e, b"Name");
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
                    // W1.8 — `<FootnoteOption>` document-level footnote
                    // separator + spacing settings. InDesign serialises
                    // this once per document (inside `<RootFootnoteStory>`
                    // or directly under `<Document>`); we match on the
                    // element name wherever it appears. Attribute names
                    // mirror the DOM `FootnoteOption` object.
                    if e.name().as_ref() == b"FootnoteOption" {
                        let f = |name: &[u8]| -> Option<f32> {
                            attr(&e, name).and_then(|s| s.parse().ok())
                        };
                        out.footnote_options = FootnoteOptions {
                            present: true,
                            rule_on: attr(&e, b"RuleOn").and_then(|s| s.parse().ok()),
                            rule_color: attr(&e, b"RuleColor"),
                            rule_tint: f(b"RuleTint"),
                            rule_line_weight: f(b"RuleLineWeight"),
                            rule_width: f(b"RuleWidth"),
                            rule_left_indent: f(b"RuleLeftIndent"),
                            rule_offset: f(b"RuleOffset"),
                            separator_text: attr(&e, b"SeparatorText"),
                            spacer: f(b"Spacer"),
                            space_between: f(b"SpaceBetween"),
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
                            let var = TextVariable {
                                self_id,
                                name: attr(&e, b"Name"),
                                variable_type: attr(&e, b"VariableType"),
                                contents: None,
                                date_format: None,
                                text_before: None,
                                text_after: None,
                                running_header_style: None,
                                running_header_use: None,
                            };
                            // A self-closing `<TextVariable/>` carries no
                            // preference child; push it straight away.
                            // The wrapping form parks it until `</TextVariable>`
                            // so the `<TextVariablePreference>` child folds in.
                            if is_start {
                                current_text_variable = Some(var);
                            } else {
                                out.text_variables.push(var);
                            }
                        }
                    }
                    // `<TextVariablePreference>` carries the type-specific
                    // payload of the enclosing `<TextVariable>`. Real
                    // exports vary which attribute they use per type:
                    // CustomText → `Contents`; the date types → `Format`;
                    // both decorated by `TextBefore` / `TextAfter`.
                    if e.name().as_ref() == b"TextVariablePreference" {
                        if let Some(var) = current_text_variable.as_mut() {
                            var.contents = attr(&e, b"Contents").or(var.contents.take());
                            var.date_format = attr(&e, b"Format").or(var.date_format.take());
                            var.text_before = attr(&e, b"TextBefore").or(var.text_before.take());
                            var.text_after = attr(&e, b"TextAfter").or(var.text_after.take());
                            // W1.18c — running-header pickup: the style
                            // whose nearest on-page occurrence supplies
                            // the text, plus the First/LastOnPage choice.
                            // InDesign serialises the style under either
                            // `AppliedParagraphStyle` or
                            // `AppliedCharacterStyle` depending on the
                            // MatchParagraphStyle vs MatchCharacterStyle
                            // variant; either fills the same slot.
                            var.running_header_style = attr(&e, b"AppliedParagraphStyle")
                                .or_else(|| attr(&e, b"AppliedCharacterStyle"))
                                .or(var.running_header_style.take());
                            var.running_header_use =
                                attr(&e, b"Use").or(var.running_header_use.take());
                        }
                    }
                    // W1.4 — hyperlink destination resources.
                    if e.name().as_ref() == b"HyperlinkURLDestination" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            let url = attr(&e, b"DestinationURL").unwrap_or_default();
                            out.hyperlink_destinations.push(HyperlinkDestination {
                                self_id,
                                kind: HyperlinkDestinationKind::Url(url),
                            });
                        }
                    }
                    if e.name().as_ref() == b"HyperlinkPageDestination" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            if let Some(page) = attr(&e, b"DestinationPage") {
                                out.hyperlink_destinations.push(HyperlinkDestination {
                                    self_id,
                                    kind: HyperlinkDestinationKind::Page(page),
                                });
                            }
                        }
                    }
                    if e.name().as_ref() == b"HyperlinkTextDestination" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            if let Some(text) = attr(&e, b"DestinationText") {
                                out.hyperlink_destinations.push(HyperlinkDestination {
                                    self_id,
                                    kind: HyperlinkDestinationKind::TextAnchor(text),
                                });
                            }
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
                                start_at: attr(&e, b"PageNumberStart").and_then(|s| s.parse().ok()),
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

    #[test]
    fn parses_hyperlink_resources_and_destinations() {
        // W1.4 — hyperlink definitions + their destination resources.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" Name="brochure.indd">
  <HyperlinkURLDestination Self="d1" DestinationURL="https://paged.media"/>
  <HyperlinkPageDestination Self="d2" DestinationPage="Page/p3"/>
  <HyperlinkTextDestination Self="d3" DestinationText="Story/s9"/>
  <Hyperlink Self="h1" Name="web" Source="HyperlinkTextSource/src1" Destination="d1"/>
  <Hyperlink Self="h2" Name="jump" Source="HyperlinkTextSource/src2" Destination="d2"/>
</Document>"#;
        let dm = DesignMap::parse(xml).unwrap();
        assert_eq!(dm.document_name.as_deref(), Some("brochure.indd"));
        assert_eq!(dm.hyperlinks.len(), 2);
        assert_eq!(
            dm.hyperlinks[0].source.as_deref(),
            Some("HyperlinkTextSource/src1")
        );
        assert_eq!(dm.hyperlinks[0].destination.as_deref(), Some("d1"));
        assert_eq!(dm.hyperlink_destinations.len(), 3);
        assert!(matches!(
            &dm.hyperlink_destinations[0].kind,
            HyperlinkDestinationKind::Url(u) if u == "https://paged.media"
        ));
        assert!(matches!(
            &dm.hyperlink_destinations[1].kind,
            HyperlinkDestinationKind::Page(p) if p == "Page/p3"
        ));
        assert!(matches!(
            &dm.hyperlink_destinations[2].kind,
            HyperlinkDestinationKind::TextAnchor(t) if t == "Story/s9"
        ));
    }

    #[test]
    fn parses_text_variable_with_preference_contents() {
        // W1.4 — a custom text variable folds in its
        // <TextVariablePreference Contents="..."> child.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <TextVariable Self="TextVariable/v1" Name="Season" VariableType="CustomTextType">
    <TextVariablePreference Contents="Spring 2026"/>
  </TextVariable>
  <TextVariable Self="TextVariable/v2" Name="Pages" VariableType="PageCountType"/>
</Document>"#;
        let dm = DesignMap::parse(xml).unwrap();
        assert_eq!(dm.text_variables.len(), 2);
        let custom = &dm.text_variables[0];
        assert_eq!(custom.variable_type.as_deref(), Some("CustomTextType"));
        assert_eq!(custom.contents.as_deref(), Some("Spring 2026"));
        let pc = &dm.text_variables[1];
        assert_eq!(pc.variable_type.as_deref(), Some("PageCountType"));
        assert_eq!(pc.contents, None);
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

    #[test]
    fn parses_footnote_option_rule_and_spacing() {
        // W1.8 — a document-level <FootnoteOption> as InDesign
        // serialises it (PascalCase DOM-mirroring attributes). The
        // parser must lift the separator-rule and spacing settings.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0">
  <RootFootnoteStory>
    <FootnoteOption RuleOn="true" RuleColor="Color/FootRule" RuleTint="100"
      RuleLineWeight="1.5" RuleWidth="120" RuleLeftIndent="6" RuleOffset="4"
      SeparatorText="^t" Spacer="9" SpaceBetween="3"/>
  </RootFootnoteStory>
</Document>"#;
        let dm = DesignMap::parse(xml).expect("parse");
        let fo = &dm.footnote_options;
        assert!(fo.present);
        assert!(!fo.is_default());
        assert_eq!(fo.rule_on, Some(true));
        assert!(fo.rule_on_effective());
        assert_eq!(fo.rule_color.as_deref(), Some("Color/FootRule"));
        assert_eq!(fo.rule_tint, Some(100.0));
        assert_eq!(fo.rule_line_weight, Some(1.5));
        assert_eq!(fo.rule_width, Some(120.0));
        assert_eq!(fo.rule_left_indent, Some(6.0));
        assert_eq!(fo.rule_offset, Some(4.0));
        assert_eq!(fo.separator_text.as_deref(), Some("^t"));
        assert_eq!(fo.spacer, Some(9.0));
        assert_eq!(fo.space_between, Some(3.0));
    }

    #[test]
    fn footnote_option_rule_off_is_distinct_from_absent() {
        // RuleOn="false" must round-trip as Some(false) — the renderer
        // distinguishes "rule explicitly off" from "no element at all"
        // (which defaults to rule ON, InDesign's behaviour).
        let off = br#"<?xml version="1.0"?><Document><FootnoteOption RuleOn="false"/></Document>"#;
        let dm = DesignMap::parse(off).expect("parse");
        assert!(dm.footnote_options.present);
        assert_eq!(dm.footnote_options.rule_on, Some(false));
        assert!(!dm.footnote_options.rule_on_effective());

        let absent = br#"<?xml version="1.0"?><Document/>"#;
        let dm = DesignMap::parse(absent).expect("parse");
        assert!(dm.footnote_options.is_default());
        assert_eq!(dm.footnote_options.rule_on, None);
        // Absent ⇒ default to rule ON.
        assert!(dm.footnote_options.rule_on_effective());
    }
}
