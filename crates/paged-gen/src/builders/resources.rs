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
    /// `Midpoint` attribute, 0..=100 in IDML (50 = linear). `None`
    /// omits it, which InDesign reads as 50. InDesign stores the
    /// diamond between two stops on the LATER of the two (measured on
    /// the `gradient-geometry` fixture).
    pub midpoint_pct: Option<f32>,
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
    write_graphic(extras, &[], &[])
}

/// Like [`graphic_xml_with_extras`] but also emits gradient swatches.
pub fn graphic_xml_with_extras_and_gradients(
    extras: &[ExtraColor],
    gradients: &[ExtraGradient],
) -> Vec<u8> {
    write_graphic(extras, gradients, &[])
}

/// Like [`graphic_xml_with_extras`] but also declares stroke styles, so
/// a page item can reference one via `StrokeType="<the style's Self>"`:
/// InDesign's built-in styles ([`BUILTIN_STROKE_STYLES`]) and the custom
/// ones (W1.2, see [`StrokeStyleSpec`]). Both live in Graphic.xml, where
/// InDesign writes them.
///
/// The built-ins are declared because InDesign resolves a built-in
/// `StrokeType` only against its declaration in the package: asked of
/// InDesign 20.0.1 (2026-10-12), every built-in reference of the
/// strokes-fills fixture — Dashed, Canned Dotted, Japanese Dots, Wavy —
/// read back as Solid while Graphic.xml declared none of them (the
/// line-ends fixture found the same for Dashed).
pub fn graphic_xml_with_stroke_styles(
    extras: &[ExtraColor],
    stroke_styles: &[StrokeStyleSpec],
) -> Vec<u8> {
    write_graphic(extras, &[], stroke_styles)
}

/// InDesign's built-in stroke styles, as every Graphic.xml it writes
/// declares them (`<StrokeStyle Self="StrokeStyle/$ID/<name>"
/// Name="$ID/<name>"/>`), in its order. There is no `Dotted` among them:
/// InDesign's dotted built-in is `Canned Dotted`.
pub const BUILTIN_STROKE_STYLES: &[&str] = &[
    "Triple_Stroke",
    "ThickThinThick",
    "ThinThickThin",
    "ThickThick",
    "ThickThin",
    "ThinThick",
    "ThinThin",
    "Japanese Dots",
    "White Diamond",
    "Left Slant Hash",
    "Right Slant Hash",
    "Straight Hash",
    "Wavy",
    "Canned Dotted",
    "Canned Dashed 3x2",
    "Canned Dashed 4x4",
    "Dashed",
    "Solid",
];

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
    /// The swatch this one is a TINT OF — `Color/<id>`. Set it together
    /// with [`tint`](Self::tint) and the swatch is written as IDML's
    /// `<Tint>` element, which is the only spelling InDesign reads.
    ///
    /// A `TintValue` attribute on a `<Color>` is a private convention:
    /// our own `ColorEntry::effective_cmyk` multiplies it into the
    /// channels, so a document written that way looks right in every
    /// paged surface and round-trips through us perfectly — and opens
    /// in InDesign at FULL STRENGTH, because InDesign drops the
    /// attribute. Measured, not guessed: a 20% vermilion panel came
    /// back from an InDesign re-export as `ColorValue="0 85 90 5"` with
    /// no tint of any kind, and printed red.
    ///
    /// `None` keeps the legacy `<Color TintValue>` spelling for callers
    /// that have not been moved over.
    pub base_color: Option<String>,
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
/// (spot / mixed-ink / standalone-tint) and `<Swatch>` aliases alongside
/// the built-in Black + Paper. Kept separate from [`write_graphic`] so
/// the existing `ExtraColor` call sites stay byte-identical.
///
/// Colour groups are NOT written here: InDesign keeps `<ColorGroup>` in
/// `designmap.xml` (see `designmap::ColorGroupSpec`).
pub fn graphic_xml_rich(colors: &[RichColor], swatches: &[SwatchSpec]) -> Vec<u8> {
    graphic_xml_rich_full(colors, &[], swatches)
}

/// Like [`graphic_xml_rich`] but also emits `<Gradient>` swatches
/// (stops referencing any of the declared colours). The rich builder
/// historically had no gradient lane — gradients only existed on the
/// minimal [`graphic_xml_with_extras_and_gradients`] path, which can't
/// express spot/tint/Lab colours — so a fixture wanting BOTH (the
/// annual-base brand ramp) goes through here. An empty `gradients`
/// slice makes this byte-identical to [`graphic_xml_rich`].
pub fn graphic_xml_rich_full(
    colors: &[RichColor],
    gradients: &[ExtraGradient],
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
        match (&c.base_color, c.tint) {
            // A named tint swatch: IDML spells it `<Tint>` with a
            // `BaseColor`, and that is what InDesign reads. The channel
            // values ride along so a reader that resolves the base
            // itself and one that trusts the written values agree.
            (Some(base), Some(_)) => {
                attrs.push(("BaseColor", base.as_str()));
                b.empty("Tint", &attrs);
            }
            _ => b.empty("Color", &attrs),
        }
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
            write_gradient_stop(&mut b, stop);
        }
        b.end("Gradient");
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
    b.end("idPkg:Graphic");
    b.into_bytes()
}

/// One `<GradientStop>`; `Midpoint` only when the stop carries one, so
/// every existing fixture's Graphic.xml stays byte-identical.
fn write_gradient_stop(b: &mut XmlBuilder, stop: &GradientStop) {
    let loc = crate::xml::format_f32(stop.location_pct);
    let mid = stop.midpoint_pct.map(crate::xml::format_f32);
    let mut attrs = vec![
        ("StopColor", stop.stop_color.as_str()),
        ("Location", loc.as_str()),
    ];
    if let Some(m) = mid.as_deref() {
        attrs.push(("Midpoint", m));
    }
    b.empty("GradientStop", &attrs);
}

/// `Resources/Graphic.xml` — registers `Color/Black` and `Color/Paper`,
/// the two swatches every IDML carries by default.
pub fn graphic_xml() -> Vec<u8> {
    write_graphic(&[], &[], &[])
}

fn write_graphic(
    extras: &[ExtraColor],
    gradients: &[ExtraGradient],
    stroke_styles: &[StrokeStyleSpec],
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
            write_gradient_stop(&mut b, stop);
        }
        b.end("Gradient");
    }
    if !stroke_styles.is_empty() {
        for name in BUILTIN_STROKE_STYLES {
            let self_id = format!("StrokeStyle/$ID/{name}");
            let id_name = format!("$ID/{name}");
            b.empty("StrokeStyle", &[("Self", &self_id), ("Name", &id_name)]);
        }
    }
    for ss in stroke_styles {
        write_stroke_style(&mut b, ss);
    }
    b.end("idPkg:Graphic");
    b.into_bytes()
}

/// One face of `Resources/Fonts.xml`, spelled as the INSTANCE InDesign
/// binds: the `fvar` named instance's PostScript name (`OpenSansRoman-
/// Regular` — the `Roman` infix only the file carries) and its design
/// axes. InDesign 20.0.1 substitutes a family-only declaration
/// (`Name="Open Sans" PostScriptName="OpenSans"`) even with the family
/// installed; the values here were read off the corpus files' `fvar`
/// records, and the attribute set + order is InDesign's own (an export
/// of a variable-font document).
pub struct FontInstance {
    pub style: &'static str,
    pub postscript_name: &'static str,
    /// `(axis name, coordinate)`, in the file's axis order; empty for a
    /// static face.
    pub axes: &'static [(&'static str, &'static str)],
}

/// One `<FontFamily>` of `Resources/Fonts.xml` with its instances.
pub struct FontFamilySpec {
    pub family: &'static str,
    /// `FontType` + `Version` as the corpus file reports them.
    pub font_type: &'static str,
    pub version: &'static str,
    pub instances: &'static [FontInstance],
}

/// `Self` of a family element: `FontFamily/<family without
/// non-alphanumerics>` — the same id the IDML exporter mints, so the
/// two spell a family identically.
fn font_family_self(family: &str) -> String {
    format!(
        "FontFamily/{}",
        family
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect::<String>()
    )
}

/// Percent-encode an axis name the way InDesign spells
/// `DesignAxesName` (`Optical%20Size`).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Write `Resources/Fonts.xml` for `families`, one `<Font>` per
/// instance in InDesign's attribute order.
pub fn fonts_xml_for(families: &[FontFamilySpec]) -> Vec<u8> {
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
    for fam in families {
        let family_self = font_family_self(fam.family);
        b.start(
            "FontFamily",
            &[("Self", &family_self), ("Name", fam.family)],
        );
        for inst in fam.instances {
            let name = format!("{} {}", fam.family, inst.style);
            let self_id = format!("{family_self}Fontn{name}");
            let num_axes = inst.axes.len().to_string();
            let axes_names = inst
                .axes
                .iter()
                .map(|(n, _)| percent_encode(n))
                .collect::<Vec<_>>()
                .join(" ");
            let axes_values = inst
                .axes
                .iter()
                .map(|(_, v)| *v)
                .collect::<Vec<_>>()
                .join(" ");
            let mut attrs: Vec<(&str, &str)> = vec![
                ("Self", &self_id),
                ("FontFamily", fam.family),
                ("Name", &name),
                ("PostScriptName", inst.postscript_name),
                ("Status", "Installed"),
                ("FontStyleName", inst.style),
                ("FontType", fam.font_type),
                ("WritingScript", "0"),
                ("FullName", &name),
                ("FullNameNative", &name),
                ("FontStyleNameNative", inst.style),
                ("PlatformName", "$ID/"),
                ("Version", fam.version),
                ("TypekitID", "$ID/"),
            ];
            if !inst.axes.is_empty() {
                attrs.push(("NumDesignAxes", &num_axes));
                attrs.push(("DesignAxesName", &axes_names));
                attrs.push(("DesignAxesValues", &axes_values));
            }
            b.empty("Font", &attrs);
        }
        b.end("FontFamily");
    }
    b.end("idPkg:Fonts");
    b.into_bytes()
}

/// The `Open Sans` family as the corpus files carry it
/// (`OpenSans.ttf` / `OpenSans-Italic.ttf`, both variable): the four
/// instances the generated samples apply.
pub const OPEN_SANS: FontFamilySpec = FontFamilySpec {
    family: "Open Sans",
    font_type: "OpenTypeTT",
    version: "Version 3.003",
    instances: &[
        FontInstance {
            style: "Regular",
            postscript_name: "OpenSansRoman-Regular",
            axes: &[("Weight", "400"), ("Width", "100")],
        },
        FontInstance {
            style: "Bold",
            postscript_name: "OpenSansRoman-Bold",
            axes: &[("Weight", "700"), ("Width", "100")],
        },
        FontInstance {
            style: "Italic",
            postscript_name: "OpenSansItalic-Regular",
            axes: &[("Weight", "400"), ("Width", "100")],
        },
        FontInstance {
            style: "Bold Italic",
            postscript_name: "OpenSansItalic-Bold",
            axes: &[("Weight", "700"), ("Width", "100")],
        },
    ],
};

/// `Resources/Fonts.xml` — declares the `Open Sans` family the samples
/// apply, instance by instance (see [`FontInstance`]). The renderer's
/// fixture fonts include OpenSans.ttf so the generated samples render
/// with the face InDesign binds when importing.
pub fn fonts_xml() -> Vec<u8> {
    fonts_xml_for(&[OPEN_SANS])
}

/// One custom stroke style, written into `Resources/Graphic.xml` in the
/// spelling InDesign 20.0.1 writes (asked of it 2026-10-12: styles made
/// through its DOM — `dashedStrokeStyles.add({dashArray: [8, 6]})`,
/// `stripedStrokeStyles.add({stripeArray: [0, 60, 80, 100]})` — and
/// exported as IDML):
///
/// ```xml
/// <DashedStrokeStyle Self="DashedStrokeStyle/Gap Dash" DashArray="8 6"
///   StrokeCornerAdjustment="DashesAndGaps" EndCap="ButtEndCap" Name="Gap Dash"/>
/// <StripedStrokeStyle Self="StripedStrokeStyle/Thick Thin"
///   StripeArray="0 60 80 100" Name="Thick Thin"/>
/// ```
///
/// A page item names the style by that `Self` in its `StrokeType`.
///
/// This generator used to write them into `Resources/Styles.xml`, with
/// the dashes in a `Pattern` attribute and the stripes as `<Stripe>`
/// children. InDesign made the styles from that by name but read neither
/// the dashes nor the stripes: its DOM gave the dashed style its default
/// 12 pt dash and the striped one its default `0 25 75 100`. Two more of
/// the fields were never InDesign's and are gone: a gap colour is a
/// property of the STROKE (`GapColor` / `GapTint` on the page item), not
/// of the style, and InDesign has no user-defined wavy style
/// (`doc.wavyStrokeStyles` does not exist; the wave is the built-in
/// `StrokeStyle/$ID/Wavy`).
pub struct StrokeStyleSpec {
    /// The style's `Self`, which `StrokeType` references. InDesign spells
    /// it `<Kind>StrokeStyle/<Name>`.
    pub self_id: &'static str,
    pub name: &'static str,
    /// `"Dashed"` or `"Striped"`.
    pub kind: &'static str,
    /// Dashed: `DashArray`, dash and gap lengths in points.
    pub dash_array: Option<&'static str>,
    /// Striped: `(left, width)` of each stripe as 0..1 fractions of the
    /// stroke weight, written as `StripeArray`'s percent start/end pairs.
    pub stripes: &'static [(f32, f32)],
}

fn write_stroke_style(b: &mut XmlBuilder, ss: &StrokeStyleSpec) {
    match ss.kind {
        "Dashed" => {
            let dashes = ss
                .dash_array
                .expect("a dashed stroke style needs its DashArray");
            b.empty(
                "DashedStrokeStyle",
                &[
                    ("Self", ss.self_id),
                    ("DashArray", dashes),
                    ("StrokeCornerAdjustment", "DashesAndGaps"),
                    ("EndCap", "ButtEndCap"),
                    ("Name", ss.name),
                ],
            );
        }
        "Striped" => {
            let stripe_array = ss
                .stripes
                .iter()
                .flat_map(|&(left, width)| [left * 100.0, (left + width) * 100.0])
                .map(crate::xml::format_f32)
                .collect::<Vec<_>>()
                .join(" ");
            let attrs = [
                ("Self", ss.self_id),
                ("StripeArray", stripe_array.as_str()),
                ("Name", ss.name),
            ];
            // ◪ The `<Stripe Left Width>` children are the engine's older
            // spelling of the same stripes. The importer this repository
            // pins reads only them, not `StripeArray`; InDesign reads the
            // attribute and passes over the children (asked: its DOM
            // reports `stripeArray` 0,60,80,100 for the strokes-fills
            // style, and 0,25,75,100 — its default — when only the
            // children were written). Drop them once the importer reads
            // `StripeArray`.
            b.start("StripedStrokeStyle", &attrs);
            for &(left, width) in ss.stripes {
                let left_s = crate::xml::format_f32(left);
                let width_s = crate::xml::format_f32(width);
                b.empty("Stripe", &[("Left", &left_s), ("Width", &width_s)]);
            }
            b.end("StripedStrokeStyle");
        }
        "Wavy" => panic!(
            "InDesign has no user-defined wavy stroke style; reference the built-in \
             `StrokeStyle/$ID/Wavy` instead"
        ),
        other => panic!("unknown stroke style kind {other}"),
    }
}

/// `Resources/Styles.xml` — declares the implicit `[No paragraph
/// style]` and `[No character style]` plus a default Open Sans
/// paragraph style for body text.
pub fn styles_xml() -> Vec<u8> {
    styles_xml_full(&[])
}

/// Like [`styles_xml`] but also emits a `<RootTableStyleGroup>`
/// carrying the supplied table styles. Used by the tables sample to
/// drive the renderer's alternating-fill path (which resolves off an
/// `AppliedTableStyle`, not per-cell fills).
pub fn styles_xml_with_table_styles(table_styles: &[TableStyleSpec]) -> Vec<u8> {
    styles_xml_full(table_styles)
}

/// W4.9 — splice an arbitrary raw style-XML `fragment` in just before
/// the closing `</idPkg:Styles>` tag of the default [`styles_xml`]
/// output. A generic post-processing trick: lets a sample
/// emit richer style cascades (next-style chains, `<CellStyle>` /
/// `<TableStyle>` BasedOn, OTF-bearing `<CharacterStyle>`s, TOC styles)
/// without growing the typed builder family.
/// Our parser keys every style element by `Self` regardless of which
/// wrapper holds it; InDesign does not, and silently drops whatever sits
/// inside a wrapper it does not know (`<RootTOCStyleGroup>`,
/// `<RootNumberingListGroup>`), so a fragment uses only InDesign's own
/// groups. Numbering lists are not styles at all in InDesign's files:
/// they are `designmap.xml` children (`designmap::NumberingListSpec`).
pub fn styles_xml_with_raw(fragment: &str) -> Vec<u8> {
    let mut text = String::from_utf8(styles_xml()).expect("styles_xml is valid utf-8");
    // MERGE INTO the root groups, never append a second one.
    //
    // Appending was the obvious splice and it produced a file with two
    // `<RootParagraphStyleGroup>` elements — the defaults' and the
    // sample's. Our own reader merges them, so nothing here ever
    // complained; InDesign takes ONE and discards the other, which is
    // how the annual's entire style sheet — every typeface it names —
    // became invisible to Adobe while our round trip stayed green.
    // Found by opening the book in InDesign 2025: three fonts, none of
    // the book's twenty.
    for group in [
        "RootCharacterStyleGroup",
        "RootParagraphStyleGroup",
        "RootObjectStyleGroup",
        "RootCellStyleGroup",
        "RootTableStyleGroup",
    ] {
        let open = format!("<{group}>");
        let close = format!("</{group}>");
        let Some(frag_start) = fragment.find(&open) else {
            continue;
        };
        let Some(frag_end) = fragment[frag_start..].find(&close) else {
            continue;
        };
        let inner = &fragment[frag_start + open.len()..frag_start + frag_end];
        if let Some(at) = text.find(&close) {
            text.insert_str(at, inner);
        } else if let Some(at) = text.rfind("</idPkg:Styles>") {
            text.insert_str(at, &format!("{open}{inner}{close}"));
        }
    }
    // Everything that is NOT one of those groups — TOC styles, which
    // InDesign writes as direct children of `<idPkg:Styles>` — still
    // appends whole.
    let rest = strip_known_groups(fragment);
    let closing = "</idPkg:Styles>";
    let spliced = match text.rfind(closing) {
        Some(idx) => format!("{}{}{}", &text[..idx], rest, &text[idx..]),
        None => format!("{text}{rest}"),
    };
    spliced.into_bytes()
}

/// The fragment with each root style group removed — what is left after
/// [`styles_xml_with_raw`] has merged those into the defaults'.
fn strip_known_groups(fragment: &str) -> String {
    let mut out = fragment.to_string();
    for group in [
        "RootCharacterStyleGroup",
        "RootParagraphStyleGroup",
        "RootObjectStyleGroup",
        "RootCellStyleGroup",
        "RootTableStyleGroup",
    ] {
        let open = format!("<{group}>");
        let close = format!("</{group}>");
        while let Some(start) = out.find(&open) {
            let Some(rel_end) = out[start..].find(&close) else {
                break;
            };
            out.replace_range(start..start + rel_end + close.len(), "");
        }
    }
    out
}

/// One `<Condition>` definition for [`MarkerResources::conditions`]
/// (`crate::builders::designmap`): a `Self` id (the value
/// `AppliedConditions` references), a display `Name`, a `Visible`
/// toggle, and the indicator colour InDesign shows for it.
///
/// Conditions are emitted into `designmap.xml`, direct children of
/// `<Document>`, in InDesign's own spelling — MEASURED on InDesign
/// 20.0.1 (2026-09-05): the `<RootConditionalTextGroup>` wrapper this
/// generator used to write inside `Resources/Styles.xml` is unknown to
/// InDesign and hid every condition inside it (0 read), the
/// `IndicatorColor` attribute was ignored, and a set's `Conditions="a b"`
/// attribute was ignored so every set silently captured ALL conditions.
pub struct ConditionSpec {
    /// e.g. `"Condition/Draft"` — the exact token a run's
    /// `AppliedConditions` must carry to be gated by this condition.
    pub self_id: &'static str,
    pub name: &'static str,
    pub visible: bool,
    /// `<Properties><IndicatorColor type="enumeration">` — an InDesign
    /// UI colour name (`Red`, `Green`, `Yellow`, `Cyan`, `Magenta`, …);
    /// `None` emits InDesign's default (`Red`).
    pub indicator_color: Option<&'static str>,
}

/// W4.8 — a `<ConditionSet>` definition. Emitted as InDesign records a
/// set: `<Properties><SetConditions>` with a `<VisibilityPair
/// Condition="…" Visibility="…"/>` for EVERY condition the document
/// declares, shown when listed here and hidden otherwise (see
/// [`ConditionSpec`]).
pub struct ConditionSetSpec {
    pub self_id: &'static str,
    pub name: &'static str,
    /// The `Condition/<id>` refs the set shows.
    pub conditions: &'static [&'static str],
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
    /// Nested styles, written as InDesign's `<Properties><AllNestedStyles>`
    /// list. Empty writes none.
    pub nested_styles: Vec<NestedStyleSpec>,
}

/// One nested style inside a [`ParagraphStyleSpec`]: a character style
/// applied to a leading portion of the paragraph, bounded by a delimiter.
pub struct NestedStyleSpec {
    pub applied_character_style: &'static str,
    /// `Delimiter` — one of InDesign's enumerations (`AnyWord`, `Tabs`,
    /// `Dropcap`, …; see [`NESTED_DELIMITER_ENUMERATIONS`]) or literal
    /// characters (`:`).
    pub delimiter: &'static str,
    pub repetition: i32,
    pub inclusive: bool,
}

/// The `<Delimiter type="enumeration">` values InDesign 20.0.1 writes for
/// the 17 delimiters its DOM offers (asked 2026-10-12). Anything else is
/// a `type="string"` delimiter: literal characters, any one of which ends
/// a repetition.
pub const NESTED_DELIMITER_ENUMERATIONS: [&str; 17] = [
    "AnyCharacter",
    "AnyWord",
    "AutoPageNumber",
    "Digits",
    "Dropcap",
    "EmSpace",
    "EndNestedStyle",
    "EnSpace",
    "ForcedLineBreak",
    "IndentHereTab",
    "InlineGraphic",
    "Letters",
    "NonbreakingSpace",
    "Repeat",
    "SectionMarker",
    "Sentence",
    "Tabs",
];

/// `<AllNestedStyles type="list">…</AllNestedStyles>` — InDesign's
/// spelling of a nested-style list, for a style's (or a range's)
/// `<Properties>` block. The engine's older `<NestedStyle>` children are
/// not read by InDesign.
pub fn all_nested_styles_xml(entries: &[NestedStyleSpec]) -> String {
    let mut out = String::from("<AllNestedStyles type=\"list\">");
    for ns in entries {
        let kind = if NESTED_DELIMITER_ENUMERATIONS.contains(&ns.delimiter) {
            "enumeration"
        } else {
            "string"
        };
        out.push_str(&format!(
            "<ListItem type=\"record\">\
<AppliedCharacterStyle type=\"object\">{}</AppliedCharacterStyle>\
<Delimiter type=\"{kind}\">{}</Delimiter>\
<Repetition type=\"long\">{}</Repetition>\
<Inclusive type=\"boolean\">{}</Inclusive></ListItem>",
            crate::xml::escape_text(ns.applied_character_style),
            crate::xml::escape_text(ns.delimiter),
            ns.repetition,
            ns.inclusive,
        ));
    }
    out.push_str("</AllNestedStyles>");
    out
}

/// Combined builder behind the [`styles_xml`] family — the default
/// styles plus table styles in one `Resources/Styles.xml`. (Custom stroke
/// styles are Graphic.xml's: [`graphic_xml_with_stroke_styles`].)
pub fn styles_xml_full(table_styles: &[TableStyleSpec]) -> Vec<u8> {
    styles_xml_full_with(table_styles, &[])
}

/// Like [`styles_xml`] but also emits the supplied named paragraph
/// styles inside the canonical `<RootParagraphStyleGroup>`. Used by the
/// text sample's contrasting "Emphasis Display" style (W2.1) so
/// `applyStyle` is render-provable AND the fixture still round-trips
/// byte-identically through `paged-write`.
pub fn styles_xml_with_paragraph_styles(extra_paragraph_styles: &[ParagraphStyleSpec]) -> Vec<u8> {
    styles_xml_full_with(&[], extra_paragraph_styles)
}

fn styles_xml_full_with(
    table_styles: &[TableStyleSpec],
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
    // The applied font is a typed CHILD of `<Properties>`, never an
    // attribute — the one high-frequency style field IDML spells that
    // way. As an attribute InDesign ignores it and falls back to its
    // own default, which is how a specimen set in twenty faces opened
    // entirely in Minion Pro.
    //
    // And the face is SPELLED next to it: a style that names a font
    // without a `FontStyle` is instanced by InDesign from the point size
    // ("Optical size-9.500", "12pt"), which substitutes every variable
    // font with an optical-size axis (measured 2026-09-05, see
    // `idml-export::face`); `Regular` is the face the engine composes
    // with when nothing in the cascade names one.
    b.start(
        "ParagraphStyle",
        &[
            ("Self", "ParagraphStyle/$ID/[No paragraph style]"),
            ("Name", "$ID/[No paragraph style]"),
            ("PointSize", "12"),
            ("FillColor", "Color/Black"),
            ("FontStyle", "Regular"),
        ],
    );
    b.start("Properties", &[]);
    b.start("AppliedFont", &[("type", "string")]);
    b.text("Open Sans");
    b.end("AppliedFont");
    b.end("Properties");
    b.end("ParagraphStyle");
    // Extra named paragraph styles — emitted in paged-write's canonical
    // attribute order so the byte round-trip gate stays green.
    for ps in extra_paragraph_styles {
        let point_size = crate::xml::format_f32(ps.point_size);
        let attrs = [
            ("Self", ps.self_id),
            ("Name", ps.name),
            ("PointSize", point_size.as_str()),
            ("FillColor", ps.fill_color),
            ("FontStyle", "Regular"),
        ];
        {
            // Same rule as above: the font is a `<Properties>` child, so
            // every style here opens rather than self-closes.
            b.start("ParagraphStyle", &attrs);
            b.start("Properties", &[]);
            b.start("AppliedFont", &[("type", "string")]);
            b.text(ps.applied_font);
            b.end("AppliedFont");
            if !ps.nested_styles.is_empty() {
                b.raw(&all_nested_styles_xml(&ps.nested_styles));
            }
            b.end("Properties");
            b.end("ParagraphStyle");
        }
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
    b.end("idPkg:Styles");
    b.into_bytes()
}

/// `Resources/Preferences.xml` — the one text preference the engine
/// composes under. The renderer reads only what the document uses;
/// InDesign opens the file regardless of which preferences are present,
/// but fills the missing ones from the APPLICATION, and InDesign 2025
/// ships `UseOpticalSize="true"` — every run set in a variable font
/// with an optical-size axis is then re-instanced at its point size
/// (measured 2026-09-05: Source Serif 4 / Fraunces come back
/// SUBSTITUTED even in a native document). The engine applies no axis
/// from the point size, so the default instance is what the document
/// was composed with, and the package says so (the exporter states the
/// same on an older package — `idml-export::preferences`).
pub fn preferences_xml() -> Vec<u8> {
    preferences_xml_with_footnote_option(None)
}

/// W1.8 — the document's `<FootnoteOption>`: the separator rule and the
/// footnote spacing. Attribute names mirror InDesign's DOM
/// `FootnoteOption`; only the subset the renderer consumes is exposed,
/// and a `None` field is left out (InDesign then keeps its own default).
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

/// [`preferences_xml`] plus the document's `<FootnoteOption>`, which
/// InDesign keeps HERE, in `Resources/Preferences.xml`, after the text
/// preference — 271 of the 305 census documents, and its own export of a
/// document whose footnote options were set through the DOM (2026-10-12).
/// This generator used to write it into `designmap.xml` inside a
/// `<RootFootnoteStory>`, which InDesign does not read: every reference
/// printed InDesign's default rule rather than the fixture's.
///
/// `RuleColor` is a typed `<Properties>` child in InDesign's spelling
/// (`<RuleColor type="object">Color/…</RuleColor>`), never an attribute:
/// asked of InDesign 20.0.1, a rule colour written only as an attribute
/// read back Black, the child alone read back the colour. ◪ It is ALSO
/// written as that attribute, the engine's older spelling, because the
/// importer this repository pins reads only the attribute (InDesign
/// passes over it); drop it once the importer reads the child.
pub fn preferences_xml_with_footnote_option(footnote: Option<&FootnoteOptionDef>) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Preferences",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    if let Some(fo) = footnote {
        write_footnote_option(&mut b, fo);
    }
    b.end("idPkg:Preferences");
    b.into_bytes()
}

/// `<FootnoteOption>` with InDesign's attribute order (`SeparatorText`,
/// `SpaceBetween`, `Spacer`, then the rule) and its `<Properties>` child.
fn write_footnote_option(b: &mut XmlBuilder, fo: &FootnoteOptionDef) {
    let num = crate::xml::format_f32;
    let mut attrs: Vec<(&str, String)> = Vec::new();
    if let Some(v) = fo.separator_text.as_deref() {
        attrs.push(("SeparatorText", v.to_string()));
    }
    if let Some(v) = fo.space_between {
        attrs.push(("SpaceBetween", num(v)));
    }
    if let Some(v) = fo.spacer {
        attrs.push(("Spacer", num(v)));
    }
    if let Some(v) = fo.rule_on {
        attrs.push(("RuleOn", v.to_string()));
    }
    if let Some(v) = fo.rule_line_weight {
        attrs.push(("RuleLineWeight", num(v)));
    }
    if let Some(v) = fo.rule_tint {
        attrs.push(("RuleTint", num(v)));
    }
    if let Some(v) = fo.rule_left_indent {
        attrs.push(("RuleLeftIndent", num(v)));
    }
    if let Some(v) = fo.rule_width {
        attrs.push(("RuleWidth", num(v)));
    }
    if let Some(v) = fo.rule_offset {
        attrs.push(("RuleOffset", num(v)));
    }
    if let Some(v) = fo.rule_color.as_deref() {
        // The older spelling, for the pinned importer (see
        // `preferences_xml_with_footnote_option`).
        attrs.push(("RuleColor", v.to_string()));
    }
    let attr_refs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    match fo.rule_color.as_deref() {
        Some(color) => {
            b.start("FootnoteOption", &attr_refs);
            b.start("Properties", &[]);
            b.start("RuleColor", &[("type", "object")]);
            b.text(color);
            b.end("RuleColor");
            b.end("Properties");
            b.end("FootnoteOption");
        }
        None => b.empty("FootnoteOption", &attr_refs),
    }
}
