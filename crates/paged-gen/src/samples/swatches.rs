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

//! W4.7 mega-file: `swatches.idml`.
//!
//! The first generated fixture to populate the colour / swatch model
//! beyond the basic process / spot / gradient swatches the other
//! samples touch. Closes the conformance gap for the swatch sub-system:
//!
//!   * **standalone tint swatches** — a spot swatch carried at full
//!     strength (`Color/InkFull`) plus a sibling `Color/InkHalf` that is
//!     the same ink at swatch-level `TintValue="50"`. The renderer
//!     previews both through the CMYK alternate; the half-tint resolves
//!     visibly lighter (the distinctive render effect this pack asserts).
//!   * **inks** — the spot swatches carry an `AlternateSpace="CMYK"` /
//!     `AlternateColorValue` fallback (the renderer has no spectral spot
//!     model, so the alternate is what actually paints).
//!   * **mixed-ink fallback presence** — a `Model="MixedInk"` swatch
//!     (`Color/Mixed`) that carries its own CMYK alternate. The renderer
//!     recognises `MixedInk` but resolves it as `Unknown`, falling back
//!     to the alternate — so the fixture proves the *fallback* is present
//!     and renders rather than dropping.
//!   * **colour groups** — a `<ColorGroup ColorGroupSwatches="…">`
//!     grouping the brand inks (round-trip for the editor's Color Groups
//!     panel).
//!   * **swatch overrides / based-on** — a `<Swatch>` that aliases a
//!     `<Color>` by reference (the named-swatch-over-colour layer), and
//!     an `ObjectStyle` BasedOn cascade whose derived style supplies the
//!     fill swatch for a frame that declares no inline fill.
//!
//! Three A4 pages, one swatch-filled rectangle each: full ink, half-tint
//! ink, and a frame filled through the `<Swatch>` alias (which resolves
//! one level of indirection to `Color/InkFull`). The parser round-trip
//! asserts the colour table, the tint, the group membership, the swatch
//! alias, the mixed-ink fallback, AND the ObjectStyle BasedOn cascade;
//! the render test asserts the half-tint pixel is lighter than the
//! full-ink pixel and the swatch-alias frame paints the wrapped colour.

use crate::builders::designmap::{write_designmap, DesignMap};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml_rich, preferences_xml, styles_xml_with_raw,
    ColorGroupSpec, RichColor, SwatchSpec,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "swatches";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;

/// Swatch self-ids the fixture defines. Exported so the tests can refer
/// to them without re-typing the literals.
pub const INK_FULL: &str = "Color/InkFull";
pub const INK_HALF: &str = "Color/InkHalf";
pub const INK_MIXED: &str = "Color/Mixed";
pub const COLOR_GROUP: &str = "ColorGroup/Brand";
pub const SWATCH_ALIAS: &str = "Swatch/BrandAlias";
/// The ObjectStyle whose BasedOn parent supplies the fill swatch.
pub const STYLE_BASE: &str = "ObjectStyle/Base";
pub const STYLE_DERIVED: &str = "ObjectStyle/Derived";

/// The brand ink's measured Lab value.
///
/// Its `AlternateColorValue` MIRRORS it, because that is the only shape
/// InDesign writes: all 15 spot swatches in the corpus packs are
/// `Space="LAB"` + `AlternateSpace="LAB"` with identical values, and
/// none has a primary and alternate that differ. This fixture used to
/// declare a CMYK alternate of `100 60 0 10` instead — a combination
/// InDesign never produces and, when it reads one, does not honour: it
/// painted the Lab primary, so the reference was `(91, 44, 159)` against
/// our `(0, 86, 157)` and the page measured ΔE 11.9 for a disagreement
/// the format does not actually contain.
const INK_LAB: &str = "30 40 -55";

/// Build the rich `Resources/Graphic.xml`: the spot full/half-tint inks,
/// the mixed-ink swatch (with its own alternate), the colour group, and
/// the swatch alias.
fn graphic() -> Vec<u8> {
    let colors = [
        RichColor {
            self_id: INK_FULL.to_string(),
            name: "Brand Ink".to_string(),
            model: "Spot",
            space: "LAB",
            value: INK_LAB.to_string(),
            alternate_space: Some("LAB"),
            alternate_value: Some(INK_LAB.to_string()),
            tint: None,
            base_color: None,
        },
        RichColor {
            self_id: INK_HALF.to_string(),
            name: "Brand Ink 50%".to_string(),
            model: "Spot",
            space: "LAB",
            value: INK_LAB.to_string(),
            alternate_space: Some("LAB"),
            alternate_value: Some(INK_LAB.to_string()),
            // A swatch-level `TintValue`. InDesign writes this on ZERO
            // of the 7,545 `<Color>` elements in the corpus packs and
            // ignores it on read — it carries a tint on the OBJECT
            // (`FillTint`, which it writes 2,949 times). Kept in the
            // palette so the reader's tolerance for it stays covered,
            // but nothing paints with it any more: page 2 tints at the
            // object level, the way the format actually spells it.
            tint: Some(50.0),
            base_color: None,
        },
        RichColor {
            self_id: INK_MIXED.to_string(),
            name: "Mixed Ink".to_string(),
            model: "MixedInk",
            space: "CMYK",
            value: "0 0 0 0".to_string(),
            // MixedInk resolves as Unknown in the renderer; the CMYK
            // alternate is the fallback that actually paints.
            alternate_space: Some("CMYK"),
            alternate_value: Some("20 80 40 0".to_string()),
            tint: None,
            base_color: None,
        },
    ];
    let groups = [ColorGroupSpec {
        self_id: COLOR_GROUP.to_string(),
        name: "Brand".to_string(),
        members: vec![
            INK_FULL.to_string(),
            INK_HALF.to_string(),
            INK_MIXED.to_string(),
        ],
    }];
    let swatches = [SwatchSpec {
        self_id: SWATCH_ALIAS.to_string(),
        name: "Brand Alias".to_string(),
        color_ref: INK_FULL.to_string(),
    }];
    graphic_xml_rich(&colors, &groups, &swatches)
}

/// `Resources/Styles.xml` carrying a two-step ObjectStyle BasedOn
/// cascade. `STYLE_BASE` supplies the fill swatch; `STYLE_DERIVED`
/// inherits it via `BasedOn` and adds nothing else. The BasedOn-styled
/// frame on page 3 declares no inline `FillColor`, so its paint comes
/// entirely from the resolved cascade.
///
/// Two things make InDesign honour a style's fill, and this fixture had
/// neither:
///
/// * `EnableFill="true"`. An object style is a set of CATEGORIES, each
///   with its own enable flag, and a category that is off contributes
///   nothing however the attributes read. All 834 styles in the corpus
///   packs that carry `EnableFill` set it `true`.
/// * the fill on the style the frame actually APPLIES. `BasedOn` on an
///   `<ObjectStyle>` is not an IDML thing: 0 of those 1,101 styles carry
///   it, and asking InDesign to author a based-on style (2026-09-08,
///   `ExportFormat.INDESIGN_MARKUP`) produces a child with no `BasedOn`
///   and its OWN resolved `FillColor="Swatch/None"` — the inheritance is
///   flattened at export and never re-derived on import.
///
/// So `STYLE_DERIVED` stays defined, with its `BasedOn`, as a case the
/// READER must tolerate; the page that has to paint applies
/// `STYLE_BASE` directly, which is the shape InDesign's own export
/// writes: `AppliedObjectStyle` on the frame, no `FillColor` anywhere on
/// it, and the colour on the style.
fn styles() -> Vec<u8> {
    let fragment = format!(
        "<RootObjectStyleGroup>\
<ObjectStyle Self=\"{STYLE_BASE}\" Name=\"Base\" EnableFill=\"true\" \
FillColor=\"{INK_FULL}\" EnableStroke=\"true\" \
StrokeColor=\"Swatch/None\" StrokeWeight=\"0\"/>\
<ObjectStyle Self=\"{STYLE_DERIVED}\" Name=\"Derived\" BasedOn=\"{STYLE_BASE}\" \
EnableFill=\"true\" EnableStroke=\"true\" \
StrokeColor=\"Swatch/None\" StrokeWeight=\"0\"/>\
</RootObjectStyleGroup>"
    );
    styles_xml_with_raw(&fragment)
}

/// One swatch-filled rectangle filling most of its page.
///
/// `AppliedObjectStyle` is pinned to `STYLE_DERIVED` on every frame, but
/// only the page that passes [`page_item::INHERIT`] as its fill actually
/// exercises the cascade — the others name a colour inline, and an
/// inline `FillColor` wins. That was the fixture's original mistake: it
/// documented a frame "that declares no inline fill" while writing one
/// on all three pages.
fn swatch_rect(seq: u32, fill: &str, extra: Vec<(String, String)>) -> Rect {
    let w = 480.0;
    let h = 600.0;
    Rect {
        self_id: self_id(SAMPLE, "Rectangle", seq),
        width_pt: w,
        height_pt: h,
        item_transform: translate((PAGE_W_PT - w) * 0.5, (PAGE_H_PT - h) * 0.5),
        fill_color: Some(fill.to_string()),
        stroke_color: None,
        stroke_weight_pt: Some(0.0),
        parent_story: None,
        next_text_frame: None,
        previous_text_frame: None,
        // Reference the derived ObjectStyle (which is BasedOn the base
        // style) so the cascade is present on every frame.
        extra_attrs: {
            // The frame that inherits its paint must apply the style
            // that HAS the paint; InDesign does not walk `BasedOn`.
            let style = if fill == crate::builders::page_item::INHERIT {
                STYLE_BASE
            } else {
                STYLE_DERIVED
            };
            let mut a = vec![("AppliedObjectStyle".to_string(), style.to_string())];
            a.extend(extra);
            a
        },
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    }
}

pub fn build() -> Sample {
    // Three pages, each spelled the way InDesign spells it:
    //   1. the spot ink at full strength;
    //   2. the same ink at 50% through the OBJECT's `FillTint` — the
    //      only tint mechanism InDesign writes;
    //   3. no inline `FillColor` at all, so the paint comes down the
    //      ObjectStyle BasedOn cascade (Derived → Base → the ink).
    //
    // Page 3 used to fill from `Swatch/BrandAlias`, a `<Swatch Color=…>`
    // aliasing element this generator invented: `<Swatch>` appears 271
    // times across the corpus packs and is `Swatch/None` every single
    // time, never with a `Color` attribute. InDesign read it as an
    // unknown swatch and painted NOTHING, so the page measured mean ΔE
    // 31.8 against a blank reference.
    let pages: [(&str, Vec<(String, String)>); 3] = [
        (INK_FULL, Vec::new()),
        (INK_FULL, vec![("FillTint".to_string(), "50".to_string())]),
        (crate::builders::page_item::INHERIT, Vec::new()),
    ];

    let mut master_spreads = Vec::with_capacity(3);
    let mut spreads = Vec::with_capacity(3);
    let mut master_refs = Vec::with_capacity(3);
    let mut spread_refs = Vec::with_capacity(3);

    for (i, (fill, extra)) in pages.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id,
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        let name = match i {
            0 => "swatches · full-ink",
            1 => "swatches · 50% FillTint",
            _ => "swatches · ObjectStyle cascade",
        };
        let rect = swatch_rect(seq, fill, extra.clone());
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                item_transform: None,
                page_self_id: page_id,
                page_name: name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![rect.into()],
                override_list: Vec::new(),
                margins: None,
            }),
        ));
        spread_refs.push(spread_id);
    }

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: master_refs,
        spreads: spread_refs,
        stories: Vec::new(),
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic(),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories: Vec::new(),
    }
}
