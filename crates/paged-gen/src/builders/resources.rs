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

//! Bare-minimum `Resources/*.xml` files plus the
//! `META-INF/container.xml` entry. The shapes here are stripped to the
//! smallest set InDesign actually requires to open the package without
//! complaint — Phase 0 samples don't need rich style cascades; the
//! builders make richer resources when later phases need them.

use crate::xml::XmlBuilder;

/// `META-INF/container.xml` — UCF rootfile pointer.
pub fn container_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "container",
        &[
            ("version", "1.0"),
            ("xmlns", "urn:oasis:names:tc:opendocument:xmlns:container"),
        ],
    );
    b.start("rootfiles", &[]);
    b.empty(
        "rootfile",
        &[("full-path", "designmap.xml"), ("media-type", "text/xml")],
    );
    b.end("rootfiles");
    b.end("container");
    b.into_bytes()
}

/// One extra colour to emit alongside the built-in Black + Paper.
/// Used by samples that need additional swatches without re-defining
/// the whole resource file.
pub struct ExtraColor {
    pub self_id: String,
    pub name: String,
    /// `"RGB"`, `"CMYK"`, `"LAB"`, `"Spot"`, `"MixedInk"` —
    /// passed straight through to the IDML `Space` attribute.
    pub space: &'static str,
    /// Whitespace-separated channel values matching `space`. RGB is
    /// `"r g b"` on the 0..255 scale (yes, IDML serialises RGB that
    /// way despite emitting CMYK on 0..100).
    pub value: String,
}

/// One gradient swatch declaration. Stops reference Color self-ids
/// either from the built-in pair (`Color/Black`, `Color/Paper`) or
/// from `ExtraColor` entries declared alongside.
pub struct ExtraGradient {
    pub self_id: String,
    pub name: String,
    /// `"Linear"` or `"Radial"`.
    pub kind: &'static str,
    pub stops: Vec<GradientStop>,
}

pub struct GradientStop {
    pub stop_color: String,
    /// `Location` attribute, 0..=100 in IDML.
    pub location_pct: f32,
}

/// One `<TableStyle>` declaration for the styles manifest. Only the
/// attributes the samples need are modelled; absent `Option`s are
/// omitted so InDesign / the parser fall back to their defaults.
/// A `<Table>` references this by setting its `AppliedTableStyle` to
/// the same `self_id`.
#[derive(Default)]
pub struct TableStyleSpec {
    pub self_id: String,
    pub name: String,
    /// `AlternatingFills` — `"AlternatingRows"` or
    /// `"AlternatingColumns"`. `None` ⇒ omit (no alternating fill).
    pub alternating_fills: Option<&'static str>,
    /// Start/End fill swatch references for the alternating pattern.
    /// The renderer reads the row OR column set depending on
    /// `alternating_fills`; the gen builder emits whichever pair is
    /// set under the matching attribute names.
    pub start_row_fill_color: Option<String>,
    pub start_row_fill_count: Option<u32>,
    pub end_row_fill_color: Option<String>,
    pub end_row_fill_count: Option<u32>,
    pub start_column_fill_color: Option<String>,
    pub start_column_fill_count: Option<u32>,
    pub end_column_fill_color: Option<String>,
    pub end_column_fill_count: Option<u32>,
}

/// Same as [`graphic_xml`] but appends caller-supplied custom swatches
/// after the built-in Black + Paper.
pub fn graphic_xml_with_extras(extras: &[ExtraColor]) -> Vec<u8> {
    write_graphic(extras, &[])
}

/// Like [`graphic_xml_with_extras`] but also emits gradient swatches.
pub fn graphic_xml_with_extras_and_gradients(
    extras: &[ExtraColor],
    gradients: &[ExtraGradient],
) -> Vec<u8> {
    write_graphic(extras, gradients)
}

/// W4.7 — a fully-specified `<Color>` swatch for [`graphic_xml_rich`].
/// Covers the spot-ink / mixed-ink / standalone-tint variants the
/// minimal [`ExtraColor`] can't express: a `Model` (`Process` / `Spot`
/// / `MixedInk`), an optional CMYK/RGB *alternate* (the fallback the
/// renderer previews spot + mixed inks through), and an optional
/// swatch-level `TintValue` (a standalone "base ink at N%" tint
/// swatch).
pub struct RichColor {
    pub self_id: String,
    pub name: String,
    /// `Model` — `"Process"`, `"Spot"`, `"MixedInk"`.
    pub model: &'static str,
    /// `Space` of the primary `ColorValue` — `"CMYK"`, `"RGB"`,
    /// `"LAB"`.
    pub space: &'static str,
    /// Whitespace-separated primary channel values.
    pub value: String,
    /// `AlternateSpace` (e.g. `"CMYK"`) for the spot/mixed-ink
    /// preview fallback. `None` ⇒ omit.
    pub alternate_space: Option<&'static str>,
    /// `AlternateColorValue` channels matching `alternate_space`.
    pub alternate_value: Option<String>,
    /// `TintValue` (0..=100) — a standalone tint swatch's swatch-level
    /// tint. `None` ⇒ omit (full strength).
    pub tint: Option<f32>,
}

/// W4.7 — a `<ColorGroup>` named grouping of swatch self-ids.
pub struct ColorGroupSpec {
    pub self_id: String,
    pub name: String,
    /// `ColorGroupSwatches` — the member `Color/<id>` refs.
    pub members: Vec<String>,
}

/// W4.7 — a `<Swatch>` that wraps (aliases) a `<Color>` by `Self`
/// reference. IDML uses these for the named-swatch-over-color layer
/// the editor's Swatches panel surfaces.
pub struct SwatchSpec {
    pub self_id: String,
    pub name: String,
    /// The wrapped `Color/<id>` reference.
    pub color_ref: String,
}

/// W4.7 — `Resources/Graphic.xml` carrying rich `<Color>` swatches
/// (spot / mixed-ink / standalone-tint), `<ColorGroup>` groupings, and
/// `<Swatch>` aliases alongside the built-in Black + Paper. Kept
/// separate from [`write_graphic`] so the existing `ExtraColor` call
/// sites stay byte-identical.
pub fn graphic_xml_rich(
    colors: &[RichColor],
    groups: &[ColorGroupSpec],
    swatches: &[SwatchSpec],
) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Graphic",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    // The two reserved swatches every IDML carries.
    b.empty(
        "Color",
        &[
            ("Self", "Color/Black"),
            ("Model", "Process"),
            ("Space", "CMYK"),
            ("ColorValue", "0 0 0 100"),
            ("ColorOverride", "Specialblack"),
            ("Name", "Black"),
            ("ColorEditable", "false"),
            ("ColorRemovable", "false"),
            ("Visible", "true"),
        ],
    );
    b.empty(
        "Color",
        &[
            ("Self", "Color/Paper"),
            ("Model", "Process"),
            ("Space", "CMYK"),
            ("ColorValue", "0 0 0 0"),
            ("ColorOverride", "Specialpaper"),
            ("Name", "Paper"),
            ("ColorEditable", "true"),
            ("ColorRemovable", "false"),
            ("Visible", "true"),
        ],
    );
    for c in colors {
        let tint_str;
        let mut attrs: Vec<(&str, &str)> = vec![
            ("Self", c.self_id.as_str()),
            ("Model", c.model),
            ("Space", c.space),
            ("ColorValue", c.value.as_str()),
            ("Name", c.name.as_str()),
        ];
        if let Some(s) = c.alternate_space {
            attrs.push(("AlternateSpace", s));
        }
        if let Some(v) = c.alternate_value.as_deref() {
            attrs.push(("AlternateColorValue", v));
        }
        if let Some(t) = c.tint {
            tint_str = crate::xml::format_f32(t);
            attrs.push(("TintValue", tint_str.as_str()));
        }
        b.empty("Color", &attrs);
    }
    for s in swatches {
        b.empty(
            "Swatch",
            &[
                ("Self", s.self_id.as_str()),
                ("Name", s.name.as_str()),
                ("ColorEditable", "true"),
                ("ColorRemovable", "true"),
                ("Visible", "true"),
                // The parser reads the wrapped colour from `Color`
                // (or `ColorEditorHotGraphic`); emit the canonical
                // `Color` attribute.
                ("Color", s.color_ref.as_str()),
            ],
        );
    }
    for g in groups {
        let members = g.members.join(" ");
        b.empty(
            "ColorGroup",
            &[
                ("Self", g.self_id.as_str()),
                ("Name", g.name.as_str()),
                ("ColorGroupSwatches", members.as_str()),
            ],
        );
    }
    b.end("idPkg:Graphic");
    b.into_bytes()
}

/// `Resources/Graphic.xml` — registers `Color/Black` and `Color/Paper`,
/// the two swatches every IDML carries by default.
pub fn graphic_xml() -> Vec<u8> {
    write_graphic(&[], &[])
}

fn write_graphic(extras: &[ExtraColor], gradients: &[ExtraGradient]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Graphic",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.empty(
        "Color",
        &[
            ("Self", "Color/Black"),
            ("Model", "Process"),
            ("Space", "CMYK"),
            ("ColorValue", "0 0 0 100"),
            ("ColorOverride", "Specialblack"),
            ("Name", "Black"),
            ("ColorEditable", "false"),
            ("ColorRemovable", "false"),
            ("Visible", "true"),
        ],
    );
    b.empty(
        "Color",
        &[
            ("Self", "Color/Paper"),
            ("Model", "Process"),
            ("Space", "CMYK"),
            ("ColorValue", "0 0 0 0"),
            ("ColorOverride", "Specialpaper"),
            ("Name", "Paper"),
            ("ColorEditable", "true"),
            ("ColorRemovable", "false"),
            ("Visible", "true"),
        ],
    );
    for extra in extras {
        b.empty(
            "Color",
            &[
                ("Self", extra.self_id.as_str()),
                ("Model", "Process"),
                ("Space", extra.space),
                ("ColorValue", extra.value.as_str()),
                ("Name", extra.name.as_str()),
            ],
        );
    }
    for grad in gradients {
        b.start(
            "Gradient",
            &[
                ("Self", grad.self_id.as_str()),
                ("Name", grad.name.as_str()),
                ("Type", grad.kind),
            ],
        );
        for stop in &grad.stops {
            let loc = crate::xml::format_f32(stop.location_pct);
            b.empty(
                "GradientStop",
                &[
                    ("StopColor", stop.stop_color.as_str()),
                    ("Location", loc.as_str()),
                ],
            );
        }
        b.end("Gradient");
    }
    b.end("idPkg:Graphic");
    b.into_bytes()
}

/// `Resources/Fonts.xml` — declares the `Open Sans` family. The
/// renderer's existing fixture fonts include OpenSans.ttf so the
/// generated samples render with the same face InDesign substitutes
/// when importing.
pub fn fonts_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Fonts",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.start(
        "FontFamily",
        &[("Self", "FontFamily/OpenSans"), ("Name", "Open Sans")],
    );
    b.empty(
        "Font",
        &[
            ("Self", "Font/OpenSans"),
            ("FontFamily", "Open Sans"),
            ("Name", "Open Sans"),
            ("PostScriptName", "OpenSans"),
            ("Status", "Installed"),
            ("FontStyleName", "Regular"),
            ("FontType", "TrueType"),
        ],
    );
    b.end("FontFamily");
    b.end("idPkg:Fonts");
    b.into_bytes()
}

/// One custom `<…StrokeStyle>` resource to emit in `Resources/Styles.xml`.
/// Covers the four W1.2 stroke-STYLE families. Only the fields each kind
/// uses are honoured; the rest stay `None`.
pub struct StrokeStyleSpec {
    pub self_id: &'static str,
    pub name: &'static str,
    /// `"Dashed"`, `"Dotted"`, `"Striped"`, or `"Wavy"`.
    pub kind: &'static str,
    /// Dashed/Dotted `Pattern` attribute (space-separated pt lengths).
    pub pattern: Option<&'static str>,
    /// `GapColor` / `GapTint` for the gap-colour under-stroke pass.
    pub gap_color: Option<&'static str>,
    pub gap_tint: Option<&'static str>,
    /// `<Stripe Left=… Width=…/>` children (0..1 ratios) for Striped.
    pub stripes: &'static [(f32, f32)],
    /// Wavy `Width` / `Wavelength` (0..1 ratios).
    pub wave_width: Option<&'static str>,
    pub wave_length: Option<&'static str>,
}

/// `Resources/Styles.xml` — declares the implicit `[No paragraph
/// style]` and `[No character style]` plus a default Open Sans
/// paragraph style for body text.
pub fn styles_xml() -> Vec<u8> {
    styles_xml_full(&[], &[])
}

/// Like [`styles_xml`] but also emits a `<RootTableStyleGroup>`
/// carrying the supplied table styles. Used by the tables sample to
/// drive the renderer's alternating-fill path (which resolves off an
/// `AppliedTableStyle`, not per-cell fills).
pub fn styles_xml_with_table_styles(table_styles: &[TableStyleSpec]) -> Vec<u8> {
    styles_xml_full(table_styles, &[])
}

/// Like [`styles_xml`] but also emits custom `<…StrokeStyle>` resources
/// (W1.2) so a page item can reference a striped / wavy / gap-coloured
/// dash via `StrokeType="StrokeStyle/<id>"`.
pub fn styles_xml_with_stroke_styles(stroke_styles: &[StrokeStyleSpec]) -> Vec<u8> {
    styles_xml_full(&[], stroke_styles)
}

/// W1.22 (engine gap 22) — `Resources/Styles.xml` carrying a single
/// `<NumberingList Self="NumberingList/Shared">` resource (plus the
/// default `[No ...]` styles). `continue_across_stories` toggles the
/// list's `ContinueNumbersAcrossStories` flag — the numbering sample
/// emits two variants (true / false) to exercise the renderer's
/// cross-story continuity decision. Built by post-processing the
/// default `styles_xml()` output: splice the `<RootNumberingListGroup>`
/// in just before the closing `</idPkg:Styles>` tag so the existing
/// builder family stays the single source of the default structure.
pub fn styles_xml_with_numbering_list(continue_across_stories: bool) -> Vec<u8> {
    let base = styles_xml();
    let group = format!(
        "<RootNumberingListGroup>\
<NumberingList Self=\"NumberingList/Shared\" Name=\"Shared\" \
ContinueNumbersAcrossStories=\"{continue_across_stories}\" \
ContinueNumbersAcrossDocuments=\"false\"/>\
</RootNumberingListGroup>"
    );
    let text = String::from_utf8(base).expect("styles_xml is valid utf-8");
    let closing = "</idPkg:Styles>";
    let spliced = match text.rfind(closing) {
        Some(idx) => format!("{}{}{}", &text[..idx], group, &text[idx..]),
        None => format!("{text}{group}"),
    };
    spliced.into_bytes()
}

/// W4.9 — splice an arbitrary raw style-XML `fragment` in just before
/// the closing `</idPkg:Styles>` tag of the default [`styles_xml`]
/// output. The same post-processing trick
/// [`styles_xml_with_numbering_list`] uses, but generic: lets a sample
/// emit richer style cascades (next-style chains, `<CellStyle>` /
/// `<TableStyle>` BasedOn, named-list definitions, OTF-bearing
/// `<CharacterStyle>`s) without growing the typed builder family.
/// The parser keys every style element by `Self` regardless of which
/// `Root…Group` wrapper holds it, so the caller may inline whatever
/// well-formed fragment it needs.
pub fn styles_xml_with_raw(fragment: &str) -> Vec<u8> {
    let text = String::from_utf8(styles_xml()).expect("styles_xml is valid utf-8");
    let closing = "</idPkg:Styles>";
    let spliced = match text.rfind(closing) {
        Some(idx) => format!("{}{}{}", &text[..idx], fragment, &text[idx..]),
        None => format!("{text}{fragment}"),
    };
    spliced.into_bytes()
}

/// One `<Condition>` definition for [`styles_xml_with_conditions`]:
/// a `Self` id (the value `AppliedConditions` references), a display
/// `Name`, and a `Visible` toggle. `IndicatorMethod` is fixed to the
/// InDesign default — only `Visible` matters to the renderer's
/// pre-layout drop rule.
pub struct ConditionSpec {
    /// e.g. `"Condition/Draft"` — the exact token a run's
    /// `AppliedConditions` must carry to be gated by this condition.
    pub self_id: &'static str,
    pub name: &'static str,
    pub visible: bool,
}

/// W4.3 — `Resources/Styles.xml` carrying a `<RootConditionalTextGroup>`
/// with the supplied `<Condition>` definitions (plus the default
/// `[No ...]` styles). Closes the W2.14 honest gap: no corpus IDML
/// previously carried `<Condition Visible="false">` defs, so the
/// renderer's conditional-text DROP path had no end-to-end fixture. A
/// run whose `AppliedConditions` reference a `Visible="false"` condition
/// is dropped pre-layout; `Visible="true"` (and unknown) refs render.
/// Built by post-processing `styles_xml()` exactly like
/// [`styles_xml_with_numbering_list`] so the default structure stays the
/// single source of truth.
pub fn styles_xml_with_conditions(conditions: &[ConditionSpec]) -> Vec<u8> {
    let mut group = String::from("<RootConditionalTextGroup>");
    for c in conditions {
        group.push_str(&format!(
            "<Condition Self=\"{}\" Name=\"{}\" Visible=\"{}\" \
IndicatorMethod=\"UseHighlight\"/>",
            c.self_id, c.name, c.visible
        ));
    }
    group.push_str("</RootConditionalTextGroup>");
    let text = String::from_utf8(styles_xml()).expect("styles_xml is valid utf-8");
    let closing = "</idPkg:Styles>";
    let spliced = match text.rfind(closing) {
        Some(idx) => format!("{}{}{}", &text[..idx], group, &text[idx..]),
        None => format!("{text}{group}"),
    };
    spliced.into_bytes()
}

/// W4.8 — a `<ConditionSet>` definition: a named grouping of
/// `Condition` self_ids the document organises into one toggleable
/// set. The renderer doesn't branch on sets (visibility resolution
/// walks individual conditions), but the data round-trips for the
/// editor's Conditions panel.
pub struct ConditionSetSpec {
    pub self_id: &'static str,
    pub name: &'static str,
    /// Member `Condition/<id>` refs.
    pub conditions: &'static [&'static str],
}

/// W4.8 — like [`styles_xml_with_conditions`] but also emits
/// `<ConditionSet>` groupings inside the `<RootConditionalTextGroup>`.
/// Lets the conditions fixture exercise the condition-SET round-trip
/// alongside the individual-condition drop rule.
pub fn styles_xml_with_conditions_and_sets(
    conditions: &[ConditionSpec],
    sets: &[ConditionSetSpec],
) -> Vec<u8> {
    let mut group = String::from("<RootConditionalTextGroup>");
    for c in conditions {
        group.push_str(&format!(
            "<Condition Self=\"{}\" Name=\"{}\" Visible=\"{}\" \
IndicatorMethod=\"UseHighlight\"/>",
            c.self_id, c.name, c.visible
        ));
    }
    for s in sets {
        let members = s.conditions.join(" ");
        group.push_str(&format!(
            "<ConditionSet Self=\"{}\" Name=\"{}\" Conditions=\"{members}\"/>",
            s.self_id, s.name
        ));
    }
    group.push_str("</RootConditionalTextGroup>");
    let text = String::from_utf8(styles_xml()).expect("styles_xml is valid utf-8");
    let closing = "</idPkg:Styles>";
    let spliced = match text.rfind(closing) {
        Some(idx) => format!("{}{}{}", &text[..idx], group, &text[idx..]),
        None => format!("{text}{group}"),
    };
    spliced.into_bytes()
}

/// One extra named `<ParagraphStyle>` to emit inside the canonical
/// `<RootParagraphStyleGroup>` alongside the implicit `[No paragraph
/// style]`. Attributes are emitted in the SAME order `paged-write`'s
/// reader→writer canonicaliser uses (`Self Name AppliedFont PointSize
/// FillColor`), so the fixture survives the byte-identical round-trip
/// gate. Only those round-trippable axes are exposed — a contrasting
/// PointSize + FillColor is enough to make an `applyStyle` swap
/// render-provable without relying on attributes `paged-write` drops
/// (e.g. `Justification`).
pub struct ParagraphStyleSpec {
    pub self_id: &'static str,
    pub name: &'static str,
    pub applied_font: &'static str,
    pub point_size: f32,
    pub fill_color: &'static str,
}

/// Combined builder behind the [`styles_xml`] family — table styles
/// and custom stroke styles in one `Resources/Styles.xml`.
pub fn styles_xml_full(
    table_styles: &[TableStyleSpec],
    stroke_styles: &[StrokeStyleSpec],
) -> Vec<u8> {
    styles_xml_full_with(table_styles, stroke_styles, &[])
}

/// Like [`styles_xml`] but also emits the supplied named paragraph
/// styles inside the canonical `<RootParagraphStyleGroup>`. Used by the
/// text sample's contrasting "Emphasis Display" style (W2.1) so
/// `applyStyle` is render-provable AND the fixture still round-trips
/// byte-identically through `paged-write`.
pub fn styles_xml_with_paragraph_styles(extra_paragraph_styles: &[ParagraphStyleSpec]) -> Vec<u8> {
    styles_xml_full_with(&[], &[], extra_paragraph_styles)
}

fn styles_xml_full_with(
    table_styles: &[TableStyleSpec],
    stroke_styles: &[StrokeStyleSpec],
    extra_paragraph_styles: &[ParagraphStyleSpec],
) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Styles",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.start("RootCharacterStyleGroup", &[]);
    b.empty(
        "CharacterStyle",
        &[
            ("Self", "CharacterStyle/$ID/[No character style]"),
            ("Name", "$ID/[No character style]"),
        ],
    );
    b.end("RootCharacterStyleGroup");
    b.start("RootParagraphStyleGroup", &[]);
    b.empty(
        "ParagraphStyle",
        &[
            ("Self", "ParagraphStyle/$ID/[No paragraph style]"),
            ("Name", "$ID/[No paragraph style]"),
            ("AppliedFont", "Open Sans"),
            ("PointSize", "12"),
            ("FillColor", "Color/Black"),
        ],
    );
    // Extra named paragraph styles — emitted in paged-write's canonical
    // attribute order so the byte round-trip gate stays green.
    for ps in extra_paragraph_styles {
        let point_size = crate::xml::format_f32(ps.point_size);
        b.empty(
            "ParagraphStyle",
            &[
                ("Self", ps.self_id),
                ("Name", ps.name),
                ("AppliedFont", ps.applied_font),
                ("PointSize", point_size.as_str()),
                ("FillColor", ps.fill_color),
            ],
        );
    }
    b.end("RootParagraphStyleGroup");
    // ObjectStyle root — declares `[None]` with the no-stroke /
    // no-fill cascade. Without this, InDesign falls back to its
    // built-in `[Normal Graphics Frame]` style which has a 1pt
    // black stroke, overriding our explicit `StrokeColor="Swatch/None"`
    // and `StrokeWeight="0"` on every Rectangle.
    b.start("RootObjectStyleGroup", &[]);
    b.empty(
        "ObjectStyle",
        &[
            ("Self", "ObjectStyle/$ID/[None]"),
            ("Name", "$ID/[None]"),
            ("FillColor", "Swatch/None"),
            ("StrokeColor", "Swatch/None"),
            ("StrokeWeight", "0"),
            (
                "AppliedParagraphStyle",
                "ParagraphStyle/$ID/[No paragraph style]",
            ),
            ("CornerOption", "None"),
            ("CornerRadius", "0"),
            ("EndCap", "ButtEndCap"),
            ("EndJoin", "MiterEndJoin"),
            ("MiterLimit", "4"),
            ("StrokeAlignment", "CenterAlignment"),
            ("StrokeType", "StrokeStyle/$ID/Solid"),
            ("Nonprinting", "false"),
        ],
    );
    b.end("RootObjectStyleGroup");
    // Table styles. The renderer's alternating-fill path resolves off
    // an `AppliedTableStyle`, so a `<Table>` that wants fills must
    // reference one of these.
    b.start("RootTableStyleGroup", &[]);
    // The built-in `[No table style]` every IDML carries.
    b.empty(
        "TableStyle",
        &[
            ("Self", "TableStyle/$ID/[No table style]"),
            ("Name", "$ID/[No table style]"),
        ],
    );
    for ts in table_styles {
        let start_row_count_s: String;
        let end_row_count_s: String;
        let start_col_count_s: String;
        let end_col_count_s: String;
        let mut a: Vec<(&str, &str)> =
            vec![("Self", ts.self_id.as_str()), ("Name", ts.name.as_str())];
        if let Some(af) = ts.alternating_fills {
            a.push(("AlternatingFills", af));
        }
        if let Some(c) = &ts.start_row_fill_color {
            a.push(("StartRowFillColor", c.as_str()));
        }
        if let Some(n) = ts.start_row_fill_count {
            start_row_count_s = n.to_string();
            a.push(("StartRowFillCount", start_row_count_s.as_str()));
        }
        if let Some(c) = &ts.end_row_fill_color {
            a.push(("EndRowFillColor", c.as_str()));
        }
        if let Some(n) = ts.end_row_fill_count {
            end_row_count_s = n.to_string();
            a.push(("EndRowFillCount", end_row_count_s.as_str()));
        }
        if let Some(c) = &ts.start_column_fill_color {
            a.push(("StartColumnFillColor", c.as_str()));
        }
        if let Some(n) = ts.start_column_fill_count {
            start_col_count_s = n.to_string();
            a.push(("StartColumnFillCount", start_col_count_s.as_str()));
        }
        if let Some(c) = &ts.end_column_fill_color {
            a.push(("EndColumnFillColor", c.as_str()));
        }
        if let Some(n) = ts.end_column_fill_count {
            end_col_count_s = n.to_string();
            a.push(("EndColumnFillCount", end_col_count_s.as_str()));
        }
        b.empty("TableStyle", &a);
    }
    b.end("RootTableStyleGroup");
    // Custom stroke-style resources (W1.2). InDesign serialises these as
    // top-level children of `idPkg:Styles`; the parser keys them by
    // `Self`, and page items reference them via `StrokeType`.
    for ss in stroke_styles {
        let elem = match ss.kind {
            "Dashed" => "DashedStrokeStyle",
            "Dotted" => "DottedStrokeStyle",
            "Striped" => "StripedStrokeStyle",
            "Wavy" => "WavyStrokeStyle",
            other => panic!("unknown stroke style kind {other}"),
        };
        let mut attrs: Vec<(&str, &str)> = vec![("Self", ss.self_id), ("Name", ss.name)];
        if let Some(p) = ss.pattern {
            attrs.push(("Pattern", p));
        }
        if let Some(gc) = ss.gap_color {
            attrs.push(("GapColor", gc));
        }
        if let Some(gt) = ss.gap_tint {
            attrs.push(("GapTint", gt));
        }
        if let Some(w) = ss.wave_width {
            attrs.push(("Width", w));
        }
        if let Some(wl) = ss.wave_length {
            attrs.push(("Wavelength", wl));
        }
        if ss.stripes.is_empty() {
            b.empty(elem, &attrs);
        } else {
            b.start(elem, &attrs);
            for &(left, width) in ss.stripes {
                let left_s = crate::xml::format_f32(left);
                let width_s = crate::xml::format_f32(width);
                b.empty("Stripe", &[("Left", &left_s), ("Width", &width_s)]);
            }
            b.end(elem);
        }
    }
    b.end("idPkg:Styles");
    b.into_bytes()
}

/// `Resources/Preferences.xml` — empty manifest. The renderer reads
/// only what the document uses; InDesign opens the file regardless of
/// which preferences are present.
pub fn preferences_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.empty(
        "idPkg:Preferences",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.into_bytes()
}
