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
    styles_xml_with_stroke_styles(&[])
}

/// Like [`styles_xml`] but also emits custom `<…StrokeStyle>` resources
/// (W1.2) so a page item can reference a striped / wavy / gap-coloured
/// dash via `StrokeType="StrokeStyle/<id>"`.
pub fn styles_xml_with_stroke_styles(stroke_styles: &[StrokeStyleSpec]) -> Vec<u8> {
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
        let mut attrs: Vec<(&str, &str)> =
            vec![("Self", ss.self_id), ("Name", ss.name)];
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
