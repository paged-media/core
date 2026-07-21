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

use crate::util::attr;
use crate::ParseError;

pub use paged_model::DesignMap;

pub use paged_model::{
    Article, Bookmark, ColorSettings, CrossReference, DocumentPreference, FootnoteOptions,
    GridPreference, Hyperlink, HyperlinkDestination, HyperlinkDestinationKind, IndexTopic, Layer,
    NumberingStyle, Section, SpreadRef, StoryRef, TextVariable,
};

/// Parse a `designmap.xml` byte slice.
pub fn parse_designmap(xml: &[u8]) -> Result<DesignMap, ParseError> {
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
                // W2.5 — `<GridPreference>` baseline-grid + document-
                // grid settings (serialised once under `<Document>`).
                // Surfaced read-only for the editor's baseline panel +
                // overlay; the renderer never draws it.
                if e.name().as_ref() == b"GridPreference" {
                    let f = |name: &[u8]| -> Option<f32> {
                        attr(&e, name).and_then(|s| s.parse().ok())
                    };
                    out.grid_preference = GridPreference {
                        present: true,
                        baseline_start: f(b"BaselineStart"),
                        baseline_division: f(b"BaselineDivision"),
                        baseline_grid_shown: attr(&e, b"BaselineGridShown")
                            .and_then(|s| s.parse().ok()),
                        baseline_grid_relative_option: attr(&e, b"BaselineGridRelativeOption"),
                        baseline_color: attr(&e, b"BaselineColor"),
                        horizontal_gridline_division: f(b"HorizontalGridlineDivision"),
                        vertical_gridline_division: f(b"VerticalGridlineDivision"),
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
                        var.running_header_use = attr(&e, b"Use").or(var.running_header_use.take());
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
        let dm = parse_designmap(SAMPLE).unwrap();
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
        let dm = parse_designmap(LAYERS_SAMPLE).unwrap();
        assert_eq!(dm.layers.len(), 4);
        let printable: Vec<bool> = dm.layers.iter().map(|l| l.printable).collect();
        assert_eq!(printable, vec![true, false, true, true]);
        let visible: Vec<bool> = dm.layers.iter().map(|l| l.visible).collect();
        assert_eq!(visible, vec![true, true, false, true]);
    }

    #[test]
    fn flat_layers_have_no_parent() {
        let dm = parse_designmap(LAYERS_SAMPLE).unwrap();
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
        let dm = parse_designmap(xml).unwrap();
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
        let dm = parse_designmap(xml).unwrap();
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
        let dm = parse_designmap(xml).unwrap();
        assert_eq!(dm.dom_version.as_deref(), Some("18.5"));
    }

    #[test]
    fn dom_version_absent_is_none() {
        // SAMPLE's <Document> carries no DOMVersion attribute.
        let dm = parse_designmap(SAMPLE).unwrap();
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
        let dm = parse_designmap(xml).unwrap();
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
        let dm = parse_designmap(xml).unwrap();
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
        let dm = parse_designmap(xml).expect("parse");
        let p = dm.document_preference;
        assert!((p.bleed_top - 8.5039).abs() < 1e-3);
        assert!((p.bleed_outside_or_right - 8.5039).abs() < 1e-3);
        assert!((p.slug_top - 14.1732).abs() < 1e-3);
        assert_eq!(p.slug_bottom, 0.0);
    }

    #[test]
    fn absent_element_defaults_to_zero() {
        let xml = br#"<?xml version="1.0"?><Document DOMVersion="18.5"/>"#;
        let dm = parse_designmap(xml).expect("parse");
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
        let dm = parse_designmap(xml).expect("parse");
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
        let dm = parse_designmap(off).expect("parse");
        assert!(dm.footnote_options.present);
        assert_eq!(dm.footnote_options.rule_on, Some(false));
        assert!(!dm.footnote_options.rule_on_effective());

        let absent = br#"<?xml version="1.0"?><Document/>"#;
        let dm = parse_designmap(absent).expect("parse");
        assert!(dm.footnote_options.is_default());
        assert_eq!(dm.footnote_options.rule_on, None);
        // Absent ⇒ default to rule ON.
        assert!(dm.footnote_options.rule_on_effective());
    }

    #[test]
    fn parses_grid_preference_baseline_grid() {
        // W2.5 — `<GridPreference>` as InDesign serialises it. The
        // parser lifts the baseline-grid subset (start / division /
        // shown / relative-to / colour) for the editor's baseline panel.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0">
  <GridPreference BaselineStart="48" BaselineDivision="12"
    BaselineGridShown="true" BaselineGridRelativeOption="TopMargin"
    BaselineColor="Color/Grid"
    HorizontalGridlineDivision="72" VerticalGridlineDivision="72"/>
</Document>"#;
        let dm = parse_designmap(xml).expect("parse");
        let gp = &dm.grid_preference;
        assert!(gp.present);
        assert_eq!(gp.baseline_start, Some(48.0));
        assert_eq!(gp.baseline_division, Some(12.0));
        assert_eq!(gp.baseline_grid_shown, Some(true));
        assert_eq!(
            gp.baseline_grid_relative_option.as_deref(),
            Some("TopMargin")
        );
        assert_eq!(gp.baseline_color.as_deref(), Some("Color/Grid"));
        assert_eq!(gp.horizontal_gridline_division, Some(72.0));
        assert_eq!(gp.vertical_gridline_division, Some(72.0));
    }

    #[test]
    fn grid_preference_absent_is_default() {
        let absent = br#"<?xml version="1.0"?><Document/>"#;
        let dm = parse_designmap(absent).expect("parse");
        assert!(!dm.grid_preference.present);
        assert_eq!(dm.grid_preference, GridPreference::default());
        assert_eq!(dm.grid_preference.baseline_division, None);
    }
}
