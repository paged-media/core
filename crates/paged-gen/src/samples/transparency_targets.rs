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

//! `transparency-targets.idml` — how InDesign composites a page item's
//! four transparency targets, asked of InDesign (ADR 120).
//!
//! A page item carries one transparency setting per TARGET: the object
//! as a whole (`<TransparencySetting>`), its stroke
//! (`<StrokeTransparencySetting>`), its fill (`<FillTransparencySetting>`)
//! and its content — the placed picture, or a text frame's text
//! (`<ContentTransparencySetting>`). Spelling and order are InDesign
//! 20.0.1's own, read off a document built through its DOM.
//!
//! Every frame is a magenta fill with a 10 pt cyan stroke; its content is
//! green "Ag" (Inter 54 pt) or a 64 × 64 picture of four quadrants (red,
//! green, blue, black). A yellow band runs under the lower half of every
//! row, so each case shows what it does over paper and over a colour.
//! The cases are numbered on the page:
//!
//! * page 1, cases 1–12 — text frames; one target at a time at opacity
//!   50 % in `Normal`, `Multiply` and `Screen` (rows Object, Fill, Stroke,
//!   Content);
//! * page 2, cases 13–24 — the same grid on graphic frames, the content
//!   row on the picture's own `<TransparencySetting>`;
//! * page 3, cases 25–40 — a drop shadow per target (text frames, then
//!   graphic frames); `<GraphicLine>`s with object and stroke
//!   transparency; and combinations (an object at 50 % over a
//!   `Multiply` fill — InDesign's object is a non-isolated group — and a
//!   fill at 50 % that casts its own shadow);
//! * page 4, cases 41–48 — `<Group>`s of two overlapping `Multiply`
//!   squares: `IsolateBlending` × `KnockoutGroup`, then the same flags
//!   under a group opacity of 50 %.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{GraphicLine, Group, PageItem, PathPoint, PlacedImage, Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "transparency-targets";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const FONT: &str = "Inter";

/// The four transparency targets, in InDesign's element names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Object,
    Fill,
    Stroke,
    Content,
}

impl Target {
    pub fn element(self) -> &'static str {
        match self {
            Target::Object => "TransparencySetting",
            Target::Fill => "FillTransparencySetting",
            Target::Stroke => "StrokeTransparencySetting",
            Target::Content => "ContentTransparencySetting",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Target::Object => "object",
            Target::Fill => "fill",
            Target::Stroke => "stroke",
            Target::Content => "content",
        }
    }

    /// InDesign writes the blocks in this order after `<Properties>`.
    fn order(self) -> u8 {
        match self {
            Target::Object => 0,
            Target::Stroke => 1,
            Target::Fill => 2,
            Target::Content => 3,
        }
    }
}

/// One target's setting on a case: blending, and optionally the
/// standard shadow.
#[derive(Clone, Debug)]
pub struct TargetSetting {
    pub target: Target,
    pub opacity: Option<f32>,
    pub blend: Option<&'static str>,
    pub shadow: bool,
}

const fn blending(target: Target, opacity: Option<f32>, blend: &'static str) -> TargetSetting {
    TargetSetting {
        target,
        opacity,
        blend: Some(blend),
        shadow: false,
    }
}

const fn shadow(target: Target) -> TargetSetting {
    TargetSetting {
        target,
        opacity: None,
        blend: None,
        shadow: true,
    }
}

/// What a case paints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A text frame holding "Ag".
    Text,
    /// A graphic frame holding the quadrant picture.
    Image,
    /// A 12 pt cyan `<GraphicLine>`.
    Line,
    /// A `<Group>` of two overlapping `Multiply` squares.
    Group { isolate: bool, knockout: bool },
}

#[derive(Clone, Debug)]
pub struct Case {
    pub n: u32,
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub kind: Kind,
    pub label: String,
    pub settings: Vec<TargetSetting>,
    /// A graphic frame's content target written the way InDesign does
    /// NOT read it: as the frame's own `<ContentTransparencySetting>`
    /// (the picture's transparency is the `<Image>`'s own
    /// `<TransparencySetting>`; see the module docs).
    pub content_on_frame: bool,
}

const COL3: [f32; 3] = [50.0, 230.0, 410.0];
const ROW4: [f32; 4] = [90.0, 280.0, 470.0, 660.0];
const COL4: [f32; 4] = [40.0, 178.0, 316.0, 454.0];
/// Frame size on pages 1 and 2.
pub const BIG: (f32, f32) = (140.0, 96.0);
/// Frame size on pages 3 and 4.
pub const SMALL: (f32, f32) = (110.0, 80.0);
const STROKE_W: f32 = 10.0;
const IMG_PX: f32 = 64.0;
const BLENDS: [&str; 3] = ["Normal", "Multiply", "Screen"];
const TARGETS: [Target; 4] = [
    Target::Object,
    Target::Fill,
    Target::Stroke,
    Target::Content,
];

/// Every case, in number order.
pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let mut n = 0;
    let mut push =
        |page: usize, x: f32, y: f32, kind: Kind, label: String, s: Vec<TargetSetting>| {
            n += 1;
            out.push(Case {
                n,
                page,
                x,
                y,
                kind,
                label,
                settings: s,
                content_on_frame: false,
            });
        };
    // Pages 1 and 2: one target at a time, 50 % × three blend modes.
    for (page, kind) in [(0, Kind::Text), (1, Kind::Image)] {
        for (row, target) in TARGETS.iter().enumerate() {
            for (col, blend) in BLENDS.iter().enumerate() {
                push(
                    page,
                    COL3[col],
                    ROW4[row],
                    kind,
                    format!("{} 50 % {}", target.name(), blend),
                    vec![blending(*target, Some(50.0), blend)],
                );
            }
        }
    }
    // Page 3: a shadow per target, text frames then graphic frames.
    for (row, kind) in [(0, Kind::Text), (1, Kind::Image)] {
        for (col, target) in TARGETS.iter().enumerate() {
            push(
                2,
                COL4[col],
                ROW4[row],
                kind,
                format!("{} shadow", target.name()),
                vec![shadow(*target)],
            );
        }
    }
    // Lines.
    let line_cases: [(&str, Vec<TargetSetting>); 4] = [
        (
            "line object 50 % Normal",
            vec![blending(Target::Object, Some(50.0), "Normal")],
        ),
        (
            "line object Multiply",
            vec![blending(Target::Object, None, "Multiply")],
        ),
        (
            "line object 50 % Screen",
            vec![blending(Target::Object, Some(50.0), "Screen")],
        ),
        (
            "line stroke 50 % Multiply",
            vec![blending(Target::Stroke, Some(50.0), "Multiply")],
        ),
    ];
    for (col, (label, s)) in line_cases.into_iter().enumerate() {
        push(2, COL4[col], ROW4[2], Kind::Line, label.to_string(), s);
    }
    // Combinations.
    push(
        2,
        COL4[0],
        ROW4[3],
        Kind::Text,
        "object 50 % + fill Multiply".into(),
        vec![
            blending(Target::Object, Some(50.0), "Normal"),
            blending(Target::Fill, None, "Multiply"),
        ],
    );
    push(
        2,
        COL4[1],
        ROW4[3],
        Kind::Image,
        "object 50 % + frame content setting".into(),
        vec![
            blending(Target::Object, Some(50.0), "Normal"),
            blending(Target::Content, None, "Multiply"),
        ],
    );
    push(
        2,
        COL4[2],
        ROW4[3],
        Kind::Text,
        "object Multiply + stroke Screen".into(),
        vec![
            blending(Target::Object, None, "Multiply"),
            blending(Target::Stroke, None, "Screen"),
        ],
    );
    push(
        2,
        COL4[3],
        ROW4[3],
        Kind::Image,
        "fill 50 % + fill shadow".into(),
        vec![TargetSetting {
            target: Target::Fill,
            opacity: Some(50.0),
            blend: Some("Normal"),
            shadow: true,
        }],
    );
    // Page 4: groups.
    let flags = [(false, false), (true, false), (false, true), (true, true)];
    for (row, group_opacity) in [(0, None), (1, Some(50.0))] {
        for (col, (isolate, knockout)) in flags.iter().enumerate() {
            let label = format!(
                "group{} isolate {} knockout {}",
                if group_opacity.is_some() { " 50 %" } else { "" },
                if *isolate { "on" } else { "off" },
                if *knockout { "on" } else { "off" },
            );
            let settings = match group_opacity {
                Some(o) => vec![blending(Target::Object, Some(o), "Normal")],
                None => Vec::new(),
            };
            push(
                3,
                COL4[col],
                ROW4[row],
                Kind::Group {
                    isolate: *isolate,
                    knockout: *knockout,
                },
                label,
                settings,
            );
        }
    }
    // Case 38 writes its content target the way InDesign ignores it.
    for c in out.iter_mut().filter(|c| c.n == 38) {
        c.content_on_frame = true;
    }
    out
}

/// The standard shadow every shadow case carries.
const SHADOW_XML: &str = r#"<DropShadowSetting Mode="Drop" XOffset="7" YOffset="7" Size="4" Opacity="75" EffectColor="Color/Black"/>"#;

/// The transparency blocks of a case, in InDesign's order.
pub fn blocks(settings: &[TargetSetting], group: Option<(bool, bool)>) -> String {
    let mut s: Vec<&TargetSetting> = settings.iter().collect();
    s.sort_by_key(|t| t.target.order());
    let mut out = String::new();
    // A group's flags live on its object-level blending setting, which
    // it needs even when it carries no opacity of its own.
    let group_flags = group.filter(|(i, k)| *i || *k);
    let has_object = s.iter().any(|t| t.target == Target::Object);
    if let (Some((isolate, knockout)), false) = (group_flags, has_object) {
        out.push_str(&format!(
            r#"<TransparencySetting><BlendingSetting IsolateBlending="{isolate}" KnockoutGroup="{knockout}"/></TransparencySetting>"#
        ));
    }
    for t in s {
        let el = t.target.element();
        out.push_str(&format!("<{el}>"));
        if t.opacity.is_some() || t.blend.is_some() || group_flags.is_some() {
            out.push_str("<BlendingSetting");
            if let Some(b) = t.blend {
                out.push_str(&format!(r#" BlendMode="{b}""#));
            }
            if let Some(o) = t.opacity {
                out.push_str(&format!(r#" Opacity="{o}""#));
            }
            if let (Target::Object, Some((isolate, knockout))) = (t.target, group_flags) {
                out.push_str(&format!(
                    r#" IsolateBlending="{isolate}" KnockoutGroup="{knockout}""#
                ));
            }
            out.push_str("/>");
        }
        if t.shadow {
            out.push_str(SHADOW_XML);
        }
        out.push_str(&format!("</{el}>"));
    }
    out
}

fn colors() -> Vec<ExtraColor> {
    let c = |id: &str, name: &str, v: &str| ExtraColor {
        self_id: format!("Color/{id}"),
        name: name.to_string(),
        space: "CMYK",
        value: v.to_string(),
    };
    vec![
        c("Magenta", "Magenta", "0 100 0 0"),
        c("Cyan", "Cyan", "100 0 0 0"),
        c("Yellow", "Yellow", "0 0 100 0"),
        c("Green", "Green", "100 0 100 0"),
    ]
}

/// A 64 × 64 opaque RGB PNG in four quadrants — red, green / blue,
/// black — stored (uncompressed) so it needs no encoder.
pub fn quadrant_png() -> Vec<u8> {
    let n = IMG_PX as usize;
    let mut raw = Vec::with_capacity(n * (1 + 3 * n));
    for y in 0..n {
        raw.push(0); // filter: none
        for x in 0..n {
            let px: [u8; 3] = match (x < n / 2, y < n / 2) {
                (true, true) => [255, 0, 0],
                (false, true) => [0, 200, 0],
                (true, false) => [0, 0, 255],
                (false, false) => [0, 0, 0],
            };
            raw.extend_from_slice(&px);
        }
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(n as u32).to_be_bytes());
    ihdr.extend_from_slice(&(n as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = data.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

fn text_paragraph(text: &str, size: f32, color: &str, justification: &'static str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(size * 1.2),
        justification: Some(justification),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(size),
            fill_color: Some(color.to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

fn rect(id: String, w: f32, h: f32, x: f32, y: f32) -> Rect {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(x, y),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: None,
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
    }
}

/// Everything one page needs: its items, the stories its text frames
/// own, and the transparency blocks to splice in after each item's
/// `</Properties>` (or a group's start tag).
#[derive(Default)]
struct PageBuild {
    items: Vec<PageItem>,
    stories: Vec<(String, Vec<u8>)>,
    splices: Vec<(String, String)>,
    group_splices: Vec<(String, String)>,
    /// `<TransparencySetting>` blocks spliced in before an `<Image>`'s
    /// close tag, keyed by the image's `Self`.
    image_splices: Vec<(String, String)>,
}

struct Ids {
    next: u32,
}

impl Ids {
    fn id(&mut self, kind: &str) -> String {
        self.next += 1;
        self_id(SAMPLE, kind, self.next)
    }
}

fn add_story(pb: &mut PageBuild, ids: &mut Ids, paragraph: Paragraph) -> String {
    let id = ids.id("Story");
    pb.stories.push((
        id.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: id.clone(),
            paragraphs: vec![paragraph],
        }),
    ));
    id
}

fn add_label(pb: &mut PageBuild, ids: &mut Ids, c: &Case) {
    let story = add_story(
        pb,
        ids,
        text_paragraph(
            &format!("{} · {}", c.n, c.label),
            7.5,
            "Color/Black",
            "LeftAlign",
        ),
    );
    let mut r = rect(ids.id("TextFrame"), 150.0, 14.0, c.x, c.y - 24.0);
    r.parent_story = Some(story);
    r.text_frame_pref = Some(TextFramePref {
        inset_spacing: Some([0.0; 4]),
        ..Default::default()
    });
    pb.items.push(r.into());
}

fn add_band(pb: &mut PageBuild, ids: &mut Ids, y: f32, h: f32) {
    let mut r = rect(ids.id("Rectangle"), PAGE_W_PT - 40.0, h, 20.0, y);
    r.fill_color = Some("Color/Yellow".into());
    pb.items.push(r.into());
}

fn add_case(pb: &mut PageBuild, ids: &mut Ids, c: &Case) {
    add_label(pb, ids, c);
    let (w, h) = if c.page < 2 { BIG } else { SMALL };
    match c.kind {
        Kind::Text | Kind::Image => {
            let id = ids.id(if c.kind == Kind::Text {
                "TextFrame"
            } else {
                "Rectangle"
            });
            let mut r = rect(id.clone(), w, h, c.x, c.y);
            r.fill_color = Some("Color/Magenta".into());
            r.stroke_color = Some("Color/Cyan".into());
            r.stroke_weight_pt = Some(STROKE_W);
            if c.kind == Kind::Text {
                let size = if c.page < 2 { 54.0 } else { 44.0 };
                r.parent_story = Some(add_story(
                    pb,
                    ids,
                    text_paragraph("Ag", size, "Color/Green", "CenterAlign"),
                ));
                r.text_frame_pref = Some(TextFramePref {
                    inset_spacing: Some([0.0; 4]),
                    vertical_justification: Some("CenterAlign"),
                    ..Default::default()
                });
            } else {
                // Placed at its own size: InDesign 20.0.1 drew an
                // embedded picture whose `<Image ItemTransform>` scaled it
                // to 0.8 at full size, so the fixture does not scale.
                let (img, s) = (IMG_PX, 1.0);
                let image_id = ids.id("Image");
                r.placed_image = Some(PlacedImage {
                    link_resource_uri: "file:transparency-targets-quadrants.png".to_string(),
                    fitting: "None",
                    left_crop: 0.0,
                    top_crop: 0.0,
                    right_crop: 0.0,
                    bottom_crop: 0.0,
                    image_self_id: image_id.clone(),
                    image_w_pt: IMG_PX,
                    image_h_pt: IMG_PX,
                    image_item_transform: Some([s, 0.0, 0.0, s, (w - img) / 2.0, (h - img) / 2.0]),
                    effective_ppi: Some((72.0 / s, 72.0 / s)),
                    actual_ppi: Some((72.0, 72.0)),
                    color_space: Some("$ID/RGB"),
                    inline_bytes: Some(quadrant_png()),
                    clipping_path: None,
                    pdf: None,
                });
                if !c.content_on_frame {
                    let (content, frame): (Vec<_>, Vec<_>) = c
                        .settings
                        .iter()
                        .cloned()
                        .partition(|t| t.target == Target::Content);
                    let picture = content
                        .into_iter()
                        .map(|t| TargetSetting {
                            target: Target::Object,
                            ..t
                        })
                        .collect::<Vec<_>>();
                    pb.image_splices.push((image_id, blocks(&picture, None)));
                    pb.splices.push((id, blocks(&frame, None)));
                    pb.items.push(r.into());
                    return;
                }
            }
            pb.splices.push((id, blocks(&c.settings, None)));
            pb.items.push(r.into());
        }
        Kind::Line => {
            let id = ids.id("GraphicLine");
            pb.items.push(
                GraphicLine {
                    self_id: id.clone(),
                    item_transform: translate(c.x, c.y + h / 2.0),
                    stroke_color: Some("Color/Cyan".into()),
                    stroke_weight_pt: Some(12.0),
                    extra_attrs: Vec::new(),
                    points: vec![PathPoint::corner((0.0, 0.0)), PathPoint::corner((w, 0.0))],
                }
                .into(),
            );
            pb.splices.push((id, blocks(&c.settings, None)));
        }
        Kind::Group { isolate, knockout } => {
            let gid = ids.id("Group");
            let sq = 62.0;
            let mut children = Vec::new();
            for (dx, dy, color) in [(0.0, 0.0, "Color/Magenta"), (40.0, 18.0, "Color/Cyan")] {
                let id = ids.id("Rectangle");
                let mut r = rect(id.clone(), sq, sq, c.x + dx, c.y + dy);
                r.fill_color = Some(color.into());
                pb.splices.push((
                    id,
                    blocks(&[blending(Target::Object, None, "Multiply")], None),
                ));
                children.push(r.into());
            }
            pb.group_splices
                .push((gid.clone(), blocks(&c.settings, Some((isolate, knockout)))));
            pb.items.push(
                Group {
                    self_id: gid,
                    item_transform: IDENTITY,
                    children,
                }
                .into(),
            );
        }
    }
}

/// Splice each item's transparency blocks into the written spread:
/// after the item's own `</Properties>` (InDesign writes them as
/// siblings of it), or right after a `<Group>`'s start tag.
fn splice(xml: Vec<u8>, pb: &PageBuild) -> Vec<u8> {
    let mut s = String::from_utf8(xml).expect("utf-8");
    for (id, block) in &pb.splices {
        if block.is_empty() {
            continue;
        }
        let at = s
            .find(&format!(r#"Self="{id}""#))
            .unwrap_or_else(|| panic!("item {id} written"));
        let close = at + s[at..].find("</Properties>").expect("properties") + "</Properties>".len();
        s.insert_str(close, block);
    }
    for (id, block) in &pb.image_splices {
        if block.is_empty() {
            continue;
        }
        let at = s
            .find(&format!(r#"<Image Self="{id}""#))
            .unwrap_or_else(|| panic!("image {id} written"));
        let close = at + s[at..].find("</Image>").expect("image close");
        s.insert_str(close, block);
    }
    for (id, block) in &pb.group_splices {
        if block.is_empty() {
            continue;
        }
        let at = s
            .find(&format!(r#"<Group Self="{id}""#))
            .unwrap_or_else(|| panic!("group {id} written"));
        let close = at + s[at..].find('>').expect("start tag") + 1;
        s.insert_str(close, block);
    }
    s.into_bytes()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let mut ids = Ids { next: 0 };
    let mut pages: Vec<PageBuild> = (0..4).map(|_| PageBuild::default()).collect();
    let all = cases();
    // The yellow band under the lower half of every row.
    for (p, pb) in pages.iter_mut().enumerate() {
        let h = if p < 2 { BIG.1 } else { SMALL.1 };
        let rows = if p == 3 { 2 } else { 4 };
        for y in ROW4.iter().take(rows) {
            add_band(pb, &mut ids, y + h / 2.0, h / 2.0 + 24.0);
        }
    }
    for c in &all {
        add_case(&mut pages[c.page], &mut ids, c);
    }

    let mut spreads = Vec::new();
    let mut spread_refs = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let titles = [
        "transparency-targets · text frames, one target at 50 %",
        "transparency-targets · graphic frames, one target at 50 %",
        "transparency-targets · shadows per target, lines, combinations",
        "transparency-targets · groups: isolate and knockout",
    ];
    for (p, mut pb) in pages.into_iter().enumerate() {
        let spread_id = self_id(SAMPLE, "Spread", p as u32);
        let xml = write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: self_id(SAMPLE, "Page", p as u32),
            page_name: titles[p].to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: std::mem::take(&mut pb.items),
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        });
        spreads.push((spread_id.clone(), splice(xml, &pb)));
        spread_refs.push(spread_id);
        for (id, bytes) in pb.stories {
            story_refs.push(id.clone());
            stories.push((id, bytes));
        }
    }

    let master_spreads = vec![(
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    )];

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id],
            spreads: spread_refs,
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras(&colors()),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader every target lands on its own
    /// slot, the picture's on the graphic frame's content target, and the
    /// group flags on the groups.
    #[test]
    fn every_case_reads_back_on_its_target() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        assert_eq!(cases().len(), 48);
        let page = |i: usize| &doc.spreads[i].spread;

        let frames: Vec<_> = page(0)
            .text_frames
            .iter()
            .filter(|f| f.fill_color.as_deref() == Some("Color/Magenta"))
            .collect();
        assert_eq!(frames.len(), 12);
        let n =
            |f: &dyn Fn(&&idml_import::TextFrame) -> bool| frames.iter().filter(|x| f(x)).count();
        assert_eq!(n(&|f| f.opacity == Some(50.0)), 3);
        assert_eq!(n(&|f| f.fill_transparency.is_some()), 3);
        assert_eq!(n(&|f| f.stroke_transparency.is_some()), 3);
        assert_eq!(n(&|f| f.content_transparency.is_some()), 3);

        let pictures: Vec<_> = page(1)
            .rectangles
            .iter()
            .filter(|r| r.has_image_element)
            .collect();
        assert_eq!(pictures.len(), 12);
        let content: Vec<_> = pictures
            .iter()
            .filter_map(|r| r.content_transparency.as_ref())
            .collect();
        assert_eq!(content.len(), 3, "the content row, on the pictures");
        assert!(content.iter().all(|t| t.opacity == Some(50.0)));

        let lines = &page(2).graphic_lines;
        assert_eq!(lines.len(), 4);
        assert_eq!(lines.iter().filter(|l| l.blend_mode.is_some()).count(), 3);
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.stroke_transparency.is_some())
                .count(),
            1
        );
        // Case 38 spells its content target as the frame's own
        // `<ContentTransparencySetting>`, which InDesign ignores — and so
        // does the reader.
        let shadows = page(2)
            .rectangles
            .iter()
            .filter(|r| r.content_drop_shadow.is_some())
            .count();
        assert_eq!(shadows, 1, "case 32's picture shadow");
        assert!(page(2)
            .rectangles
            .iter()
            .filter(|r| r.has_image_element && r.opacity == Some(50.0))
            .all(|r| r.content_transparency.is_none()));

        let groups = &page(3).groups;
        assert_eq!(groups.len(), 8);
        let flags = |iso: bool, ko: bool| {
            groups
                .iter()
                .filter(|g| {
                    g.transparency.isolate_blending == iso && g.transparency.knockout_group == ko
                })
                .count()
        };
        assert_eq!(
            (
                flags(false, false),
                flags(true, false),
                flags(false, true),
                flags(true, true)
            ),
            (2, 2, 2, 2)
        );
    }
}
