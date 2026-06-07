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

//! Aftercare-D mega-file: `links-broken.idml`.
//!
//! One A4 page of placed-image rectangles that drive the editor's Links
//! panel (`LinkSummary`, panels.md gap 2):
//!
//!   * **`links · broken · missing-tif`** + **`links · broken ·
//!     missing-png`**: rectangles whose `<Image LinkResourceURI>` points
//!     at files that exist nowhere — not in the package, not on disk.
//!     With no asset resolver wired, the build can't resolve them, stamps
//!     the missing-image placeholder, and fires
//!     `DiagnosticCode::ImageLinkMissing`, so the canvas model classifies
//!     them `status = "missing"`.
//!   * **`links · ok · embedded`**: the healthy control — a rectangle
//!     hosting an inline-embedded PNG (base64 `<Contents>` CDATA). The
//!     pipeline resolves it from the embedded bytes without any resolver,
//!     so it renders cleanly and reads `status = "ok"`.
//!   * **`links · ppi · low-res`**: a large frame holding a genuinely
//!     tiny (2×2 px) embedded PNG, declaring `ActualPpi="(300 300)"
//!     EffectivePpi="(96 96)"`. It resolves "ok" (the bytes are
//!     embedded) but its effective PPI is far below the 150-ppi preflight
//!     threshold — the low-resolution-image warning case. `effective_ppi`
//!     is read verbatim from the `<Image EffectivePpi>` attribute by
//!     `paged-parse` (`image_metadata`), so the value is honest fixture
//!     data, not derived.
//!
//! "missing" vs "ok" is a *build-time* classification (it depends on
//! whether the render resolved the asset), so the sample test builds the
//! document through `paged-renderer` and inspects its diagnostics rather
//! than asserting a parse-only proxy. The PPI assertion is parse-level
//! (`Spread::image_metadata`).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, PlacedImage, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "links-broken";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 18.0;
const BODY_FONT: &str = "Inter";

/// A deterministic 2×2 RGBA PNG (solid green). Embedded as the inline
/// `<Contents>` payload of the healthy + low-ppi frames so they resolve
/// without an external file. Generated once; kept as raw bytes so the
/// builder base64-encodes them at emit time (stable across runs).
const GREEN_2X2_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x50, 0x3a, 0x1a, 0xf7,
    0x1f, 0x84, 0x19, 0x60, 0x0c, 0x00, 0x4d, 0x42, 0x09, 0x11, 0x4f, 0x30, 0xb7, 0xb3, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Variant {
    name: &'static str,
    /// Frame extents in pt.
    frame_w_pt: f32,
    frame_h_pt: f32,
    /// `LinkResourceURI` written on the `<Image>` + `<Link>`.
    link_uri: &'static str,
    /// Native pixel dims of the placed image (for the inner
    /// PathGeometry). 2 pt for the embedded fixtures.
    image_native_pt: (f32, f32),
    /// `Space` colour-space string.
    color_space: Option<&'static str>,
    /// `ActualPpi` / `EffectivePpi` x/y pairs. The low-res variant sets
    /// a sub-150 effective PPI.
    actual_ppi: Option<(f32, f32)>,
    effective_ppi: Option<(f32, f32)>,
    /// Inline image bytes. `Some` ⇒ healthy (resolves "ok"); `None` ⇒
    /// link-only (resolves "missing" with no resolver).
    inline: bool,
}

fn variants() -> Vec<Variant> {
    vec![
        // ── two genuinely broken links ───────────────────────────
        Variant {
            name: "links · broken · missing-tif",
            frame_w_pt: 200.0,
            frame_h_pt: 150.0,
            link_uri: "file:///does-not-exist/photo.tif",
            image_native_pt: (200.0, 150.0),
            color_space: Some("$ID/CMYK"),
            actual_ppi: Some((300.0, 300.0)),
            effective_ppi: Some((300.0, 300.0)),
            inline: false,
        },
        Variant {
            name: "links · broken · missing-png",
            frame_w_pt: 200.0,
            frame_h_pt: 150.0,
            link_uri: "file:///does-not-exist/logo.png",
            image_native_pt: (200.0, 150.0),
            color_space: Some("$ID/RGB"),
            actual_ppi: None,
            effective_ppi: None,
            inline: false,
        },
        // ── one healthy embedded image (control) ─────────────────
        Variant {
            name: "links · ok · embedded",
            frame_w_pt: 200.0,
            frame_h_pt: 150.0,
            // The URI is descriptive; the inline bytes win at build
            // time so it never reaches the resolver.
            link_uri: "file:embedded-green.png",
            image_native_pt: (2.0, 2.0),
            color_space: Some("$ID/RGB"),
            // 2 px across a ~150 pt frame is high-ish nominal ppi only
            // because the asset is tiny; we still advertise a healthy
            // EffectivePpi so the control reads as a normal link.
            actual_ppi: Some((72.0, 72.0)),
            effective_ppi: Some((300.0, 300.0)),
            inline: true,
        },
        // ── one healthy-but-low-resolution embedded image ────────
        Variant {
            name: "links · ppi · low-res",
            // A large frame stretching the 2×2 px image → low effective
            // resolution. The declared EffectivePpi is the honest hint
            // the preflight check compares against 150.
            frame_w_pt: 360.0,
            frame_h_pt: 240.0,
            link_uri: "file:embedded-lowres.png",
            image_native_pt: (2.0, 2.0),
            color_space: Some("$ID/RGB"),
            actual_ppi: Some((300.0, 300.0)),
            effective_ppi: Some((96.0, 96.0)),
            inline: true,
        },
    ]
}

/// A label text frame + its backing story. Returns `(frame, story)`.
fn label(page_name: &str, story_id: &str, frame_id: String, y_pt: f32) -> (Rect, Story) {
    let story = Story {
        self_id: story_id.to_string(),
        paragraphs: vec![label_paragraph(page_name)],
    };
    let frame = Rect {
        self_id: frame_id,
        width_pt: LABEL_W_PT,
        height_pt: LABEL_H_PT,
        item_transform: translate(36.0, y_pt),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id.to_string()),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    };
    (frame, story)
}

fn label_paragraph(text: &str) -> Paragraph {
    Paragraph {
        justification: None,
        space_before: None,
        space_after: None,
        leading: None,
        first_line_indent: None,
        left_indent: None,
        right_indent: None,
        drop_cap_characters: None,
        drop_cap_lines: None,
        tab_list: Vec::new(),
        bullets_list_type: None,
        applied_numbering_list: None,
        bullet_character: None,
        table: None,
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
        runs: vec![Run {
            text: text.to_string(),
            point_size: Some(11.0),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
    }
}

pub fn build() -> Sample {
    let variants = variants();

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut story_refs: Vec<String> = Vec::new();
    let mut items: Vec<PageItem> = Vec::new();

    // Stack each (label + image frame) pair down the page.
    for (i, v) in variants.iter().enumerate() {
        let seq = i as u32;
        let label_story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let rect_id = self_id(SAMPLE, "Rectangle", seq);
        let image_id = self_id(SAMPLE, "Image", seq);

        let row_top = 36.0 + (seq as f32) * 180.0;
        let (label_frame, label_story) = label(v.name, &label_story_id, label_frame_id, row_top);
        stories.push((label_story_id.clone(), write_story(&label_story)));
        story_refs.push(label_story_id);
        items.push(label_frame.into());

        let placed = PlacedImage {
            link_resource_uri: v.link_uri.to_string(),
            fitting: "FitContentToFrame",
            left_crop: 0.0,
            top_crop: 0.0,
            right_crop: 0.0,
            bottom_crop: 0.0,
            image_self_id: image_id,
            image_w_pt: v.image_native_pt.0,
            image_h_pt: v.image_native_pt.1,
            image_item_transform: None,
            effective_ppi: v.effective_ppi,
            actual_ppi: v.actual_ppi,
            color_space: v.color_space,
            inline_bytes: if v.inline {
                Some(GREEN_2X2_PNG.to_vec())
            } else {
                None
            },
            clipping_path: None,
        };

        let rect = Rect {
            self_id: rect_id,
            width_pt: v.frame_w_pt,
            height_pt: v.frame_h_pt,
            item_transform: translate(36.0, row_top + 20.0),
            // Paper fill so a healthy frame isn't see-through; the
            // missing frames get the placeholder stamped over this.
            fill_color: Some("Color/Paper".to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: None,
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: Some(placed),
            text_wrap: None,
            anchored_setting: None,
            frame_effects: Vec::new(),
            text_frame_pref: None,
            custom_subpaths: None,
        };
        items.push(rect.into());
    }

    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: page_id,
        page_name: "links · broken+ok".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: vec![spread_id.clone()],
        stories: story_refs,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
