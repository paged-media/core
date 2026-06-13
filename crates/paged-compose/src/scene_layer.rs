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

//! Plugin scene-layer IR (C-1).
//!
//! A plugin renders *vector* content INSIDE a frame by submitting a
//! [`SceneLayer`] — a small, serializable subset of the display list
//! (filled / stroked bezier paths with solid paint) authored in
//! **frame-content coordinates**: origin `(0,0)` at the frame's
//! content-box top-left, x right, y down, in points.
//!
//! The plugin never compensates for the frame's transform (§8.5): core
//! applies the frame's `ItemTransform` and clips to the content box at
//! compose time, in [`emit_scene_layer`]. Because the layer lowers to
//! ordinary [`DisplayCommand`]s, it renders through the *same* Vello (GPU)
//! and tiny-skia (CPU) lanes as native content — so it is colour-managed,
//! print-correct, and unit-testable on the CPU lane without a GPU.
//!
//! This is the VECTOR path of the GPU-surface RFC (`rfc-gpu-surface.md`).
//! The raw-`GPUTexture` path (image viewport) and the GPU-device door are
//! separate, later stages.

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::display_list::{
    Color, DecodedImage, DisplayCommand, DisplayList, GradientStop, LinearGradient, Paint, PathData,
    PathSegment, RadialGradient, Rect, Stroke, Transform,
};

/// A plugin-submitted vector layer in frame-content coordinates. Keyed
/// (on the wire) by the host element id of the frame it renders into.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SceneLayer {
    pub items: Vec<SceneItem>,
}

/// One drawable in a [`SceneLayer`]. Coordinates are frame-content points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SceneItem {
    /// Fill a bezier path (non-zero winding) with a solid paint.
    FillPath {
        path: Vec<ScenePathSeg>,
        paint: ScenePaint,
    },
    /// Stroke a bezier path. `width` is in points, in content space
    /// (core applies the frame transform to the geometry, and the
    /// rasterizer scales the width with the page-to-pixel transform).
    StrokePath {
        path: Vec<ScenePathSeg>,
        paint: ScenePaint,
        width: f32,
    },
    /// Draw a single-line text run (C-1.1). Glyphs are shaped + emitted by
    /// the RENDERER (it owns the fonts); `paged-compose` routes this item
    /// to the caller's text emitter (see [`emit_scene_layer`]). v1 renders
    /// in the document's DEFAULT font (the `family`/`style` hints are
    /// reserved for per-run face selection) and positions glyphs at the
    /// transformed baseline; full per-glyph affine (rotated-frame text)
    /// is a follow-on.
    Text(SceneTextItem),
    /// Composite a pre-decoded RGBA8 image into the frame (C-1.2 — the
    /// GPU-texture door's bytes/CPU stage, "Stage A"). `rgba` is tightly
    /// packed RGBA8 (`width * height * 4` bytes, row-major); the image is
    /// placed at the `dest` rect (top-left `x`/`y`, size `w`/`h`) in
    /// frame-content points and clipped to the content box like every
    /// scene item. The RENDERER interns the pixels into the display-list
    /// image pool and emits the same [`DisplayCommand::Image`] lane placed
    /// assets use, so it rasterises through tiny-skia (CPU) / Vello (GPU)
    /// with no new path. The shared-`GPUDevice` zero-copy stage (Stage B)
    /// is a follow-on. A malformed buffer (length ≠ `w*h*4`, or a
    /// zero-area image/dest) is skipped, never panicked.
    Image {
        /// Tightly packed RGBA8, row-major. Length must be `width*height*4`.
        #[tsify(type = "number[]")]
        rgba: Vec<u8>,
        /// Pixel width of the buffer.
        width: u32,
        /// Pixel height of the buffer.
        height: u32,
        /// Destination top-left x in frame-content points.
        x: f32,
        /// Destination top-left y in frame-content points.
        y: f32,
        /// Destination width in frame-content points.
        w: f32,
        /// Destination height in frame-content points.
        h: f32,
    },
    /// Fill a bezier path with a linear or radial gradient (C-1.3).
    /// **Additive** to [`SceneItem::FillPath`] — existing solid-fill
    /// consumers are untouched. The gradient geometry is authored in
    /// frame-content points (the same space as the path) and is carried to
    /// page space by the frame transform exactly like the path, so a
    /// plugin lowering a CSS `linear-gradient`/`radial-gradient` (paged.web,
    /// ADR-011) tracks its box. Lowers to the **same** gradient fill lane
    /// IDML placed gradients use (`Paint::LinearGradient`/`RadialGradient`
    /// over the display-list gradient pool), so it rasterises through
    /// tiny-skia (CPU) / Vello (GPU) with no new render path and is
    /// CPU-testable. An empty path or a gradient with <2 stops is skipped,
    /// never panicked.
    FillPathGradient {
        path: Vec<ScenePathSeg>,
        gradient: SceneGradient,
    },
}

/// A plugin gradient paint for [`SceneItem::FillPathGradient`] (C-1.3).
/// Coordinates are frame-content points (mapped by the frame transform
/// like the filled path). Colours are sRGB, linearised at lowering to
/// composite identically to document colours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SceneGradient {
    /// Linear gradient from `(x0,y0)` to `(x1,y1)` in content points.
    Linear {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        stops: Vec<SceneGradientStop>,
    },
    /// Radial gradient centred at `(cx,cy)` with `radius`, in content
    /// points.
    Radial {
        cx: f32,
        cy: f32,
        radius: f32,
        stops: Vec<SceneGradientStop>,
    },
}

/// One colour stop in a [`SceneGradient`]. `offset` is `0.0..=1.0` along
/// the gradient axis; the colour is sRGB (linearised at lowering).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SceneGradientStop {
    pub offset: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// A single-line text run in frame-content coordinates (C-1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SceneTextItem {
    /// Baseline origin x in frame-content points.
    pub x: f32,
    /// Baseline origin y in frame-content points (the text baseline).
    pub y: f32,
    /// The run's text (single line — newlines are not laid out).
    pub text: String,
    /// Point size.
    pub size: f32,
    pub paint: ScenePaint,
    /// Reserved face hints (v1 renders in the document default font).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// A bezier path segment in frame-content coordinates (points).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ScenePathSeg {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    CubicTo {
        cx1: f32,
        cy1: f32,
        cx2: f32,
        cy2: f32,
        x: f32,
        y: f32,
    },
    Close,
}

/// A solid paint in **sRGB** (0..=1 per channel; alpha is linear). Core
/// converts to the display list's linear-light [`Color`] so plugin
/// colours composite identically to document colours.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ScenePaint {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// sRGB → linear-light (the IEC 61966-2-1 transfer). The display list
/// composites in linear light, so plugin sRGB colours must be linearised
/// to match document colours.
fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert a plugin sRGB [`ScenePaint`] to the display list's linear-light
/// [`Color`]. Public so the renderer's text emitter (which lowers
/// [`SceneItem::Text`]) paints glyphs with the same colour treatment as
/// the path items.
pub fn scene_paint_to_color(p: ScenePaint) -> Color {
    Color::rgba(
        srgb_to_linear(p.r),
        srgb_to_linear(p.g),
        srgb_to_linear(p.b),
        p.a.clamp(0.0, 1.0),
    )
}

fn paint_to_color(p: ScenePaint) -> Color {
    scene_paint_to_color(p)
}

/// Convert plugin gradient stops (sRGB) to display-list stops
/// (linear-light), clamping offsets to `0.0..=1.0` and sorting by offset
/// so the rasterizer gets a monotone ramp.
fn scene_stops_to_display(stops: &[SceneGradientStop]) -> Vec<GradientStop> {
    let mut out: Vec<GradientStop> = stops
        .iter()
        .map(|s| GradientStop {
            offset: s.offset.clamp(0.0, 1.0),
            color: scene_paint_to_color(ScenePaint {
                r: s.r,
                g: s.g,
                b: s.b,
                a: s.a,
            }),
        })
        .collect();
    out.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    out
}

fn build_path(segs: &[ScenePathSeg]) -> PathData {
    let mut out = Vec::with_capacity(segs.len());
    for s in segs {
        out.push(match *s {
            ScenePathSeg::MoveTo { x, y } => PathSegment::MoveTo { x, y },
            ScenePathSeg::LineTo { x, y } => PathSegment::LineTo { x, y },
            ScenePathSeg::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            },
            ScenePathSeg::Close => PathSegment::Close,
        });
    }
    PathData { segments: out }
}

/// Lower a [`SceneLayer`] into `list`, clipped to the frame's content box
/// and transformed into page space.
///
/// `content_outer` maps frame-content coordinates (origin = content-box
/// top-left) to page space — i.e. it already folds in the frame's
/// `ItemTransform`, the spread transform, and the content-box offset. The
/// renderer builds it from `frame_outer_transform ∘ translate(content_left,
/// content_top)`. `content_size` is the content box `(w, h)` in points,
/// used for the clip; a non-positive size skips the clip but still emits
/// the items (an unclipped layer, the honest degenerate case).
///
/// Items render in submission order, ON TOP of the frame's own content
/// (the plugin layer is the frontmost content of its frame). A `PushClip`
/// / `PopClip` pair brackets them so nothing escapes the (possibly
/// rotated) content box.
///
/// `emit_text` lowers a [`SceneItem::Text`] (C-1.1): `paged-compose` has no
/// fonts, so the RENDERER passes a closure that resolves a face, shapes the
/// run, and emits glyph `FillPath`s into `list` — within the same clip
/// bracket. Callers with no text (or no fonts) pass a no-op
/// `|_, _, _| {}`; the converter's path/fill items need no font.
pub fn emit_scene_layer<T>(
    list: &mut DisplayList,
    layer: &SceneLayer,
    content_outer: Transform,
    content_size: (f32, f32),
    mut emit_text: T,
) where
    T: FnMut(&mut DisplayList, &SceneTextItem, Transform),
{
    if layer.items.is_empty() {
        return;
    }

    let (cw, ch) = content_size;
    let clipped = cw > 0.0 && ch > 0.0;
    if clipped {
        // Clip path = the content box [0,0,cw,ch] in content coords,
        // carried into page space by `content_outer`.
        let clip = list.paths.push_anon(PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: cw, y: 0.0 },
                PathSegment::LineTo { x: cw, y: ch },
                PathSegment::LineTo { x: 0.0, y: ch },
                PathSegment::Close,
            ],
        });
        list.push(DisplayCommand::PushClip {
            path_id: clip,
            transform: content_outer,
        });
    }

    for item in &layer.items {
        match item {
            SceneItem::FillPath { path, paint } => {
                if path.is_empty() {
                    continue;
                }
                let id = list.paths.push_anon(build_path(path));
                list.push(DisplayCommand::FillPath {
                    path_id: id,
                    paint: Paint::Solid(paint_to_color(*paint)),
                    transform: content_outer,
                });
            }
            SceneItem::StrokePath { path, paint, width } => {
                if path.is_empty() {
                    continue;
                }
                let id = list.paths.push_anon(build_path(path));
                list.push(DisplayCommand::StrokePath {
                    path_id: id,
                    paint: Paint::Solid(paint_to_color(*paint)),
                    stroke: Stroke::new((*width).max(0.0)),
                    transform: content_outer,
                });
            }
            SceneItem::Text(t) => {
                if t.text.is_empty() {
                    continue;
                }
                emit_text(list, t, content_outer);
            }
            SceneItem::Image {
                rgba,
                width,
                height,
                x,
                y,
                w,
                h,
            } => {
                // Skip a malformed buffer or a zero-area image/dest rather
                // than panic the whole render (a plugin owns these bytes).
                if *width == 0
                    || *height == 0
                    || *w <= 0.0
                    || *h <= 0.0
                    || rgba.len() != (*width as usize) * (*height as usize) * 4
                {
                    continue;
                }
                let image_id = list.push_image(DecodedImage {
                    width: *width,
                    height: *height,
                    encoded: bytes::Bytes::new(),
                    rgba: bytes::Bytes::from(rgba.clone()),
                    icc: None,
                });
                // `for_rect_in` maps the image's unit square into `dest`
                // (content coords) then `content_outer` carries it to page
                // space — the same transform placed assets use.
                let dest = Rect {
                    x: *x,
                    y: *y,
                    w: *w,
                    h: *h,
                };
                list.push(DisplayCommand::Image {
                    image_id,
                    transform: Transform::for_rect_in(dest, content_outer),
                });
            }
            SceneItem::FillPathGradient { path, gradient } => {
                // A gradient needs a path to fill and >=2 stops to ramp;
                // skip degenerate input rather than emit an empty fill.
                if path.is_empty() {
                    continue;
                }
                let paint = match gradient {
                    SceneGradient::Linear {
                        x0,
                        y0,
                        x1,
                        y1,
                        stops,
                    } => {
                        if stops.len() < 2 {
                            continue;
                        }
                        let id = list.push_linear_gradient(LinearGradient {
                            start: (*x0, *y0),
                            end: (*x1, *y1),
                            stops: scene_stops_to_display(stops),
                        });
                        Paint::LinearGradient(id)
                    }
                    SceneGradient::Radial {
                        cx,
                        cy,
                        radius,
                        stops,
                    } => {
                        if stops.len() < 2 || *radius <= 0.0 {
                            continue;
                        }
                        let id = list.push_radial_gradient(RadialGradient {
                            center: (*cx, *cy),
                            radius: *radius,
                            stops: scene_stops_to_display(stops),
                        });
                        Paint::RadialGradient(id)
                    }
                };
                let path_id = list.paths.push_anon(build_path(path));
                // Same transform as the path (`content_outer`): the
                // rasterizer maps the gradient's content-point endpoints
                // through it exactly as it maps the path geometry, so the
                // gradient tracks the box (cpu.rs build_*_gradient_shader).
                list.push(DisplayCommand::FillPath {
                    path_id,
                    paint,
                    transform: content_outer,
                });
            }
        }
    }

    if clipped {
        list.push(DisplayCommand::PopClip(Transform::IDENTITY));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<ScenePathSeg> {
        vec![
            ScenePathSeg::MoveTo { x: x0, y: y0 },
            ScenePathSeg::LineTo { x: x1, y: y1 },
        ]
    }

    fn black() -> ScenePaint {
        ScenePaint {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }

    #[test]
    fn empty_layer_emits_nothing() {
        let mut list = DisplayList::new();
        emit_scene_layer(
            &mut list,
            &SceneLayer::default(),
            Transform::IDENTITY,
            (10.0, 10.0),
            |_, _, _| {},
        );
        assert!(list.commands.is_empty());
        assert_eq!(list.paths.len(), 0);
    }

    #[test]
    fn layer_brackets_items_in_a_content_box_clip() {
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![
                SceneItem::StrokePath {
                    path: line(0.0, 0.0, 100.0, 0.0),
                    paint: black(),
                    width: 1.0,
                },
                SceneItem::FillPath {
                    path: vec![
                        ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
                        ScenePathSeg::LineTo { x: 10.0, y: 0.0 },
                        ScenePathSeg::LineTo { x: 10.0, y: 10.0 },
                        ScenePathSeg::Close,
                    ],
                    paint: black(),
                },
            ],
        };
        emit_scene_layer(
            &mut list,
            &layer,
            Transform::IDENTITY,
            (200.0, 100.0),
            |_, _, _| {},
        );
        // PushClip, StrokePath, FillPath, PopClip.
        assert_eq!(list.commands.len(), 4);
        assert!(matches!(list.commands[0], DisplayCommand::PushClip { .. }));
        assert!(matches!(
            list.commands[1],
            DisplayCommand::StrokePath { .. }
        ));
        assert!(matches!(list.commands[2], DisplayCommand::FillPath { .. }));
        assert!(matches!(list.commands[3], DisplayCommand::PopClip(_)));
        // clip rect + 2 item paths.
        assert_eq!(list.paths.len(), 3);
    }

    #[test]
    fn linear_gradient_fill_lowers_to_a_pooled_gradient_paint() {
        // FillPathGradient lowers to a DisplayCommand::FillPath whose paint
        // references a pooled LinearGradient — the SAME lane IDML placed
        // gradients use, additive to the solid FillPath path. Out-of-order
        // stops are normalised to a monotone ramp.
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::FillPathGradient {
                path: vec![
                    ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
                    ScenePathSeg::LineTo { x: 50.0, y: 0.0 },
                    ScenePathSeg::LineTo { x: 50.0, y: 50.0 },
                    ScenePathSeg::Close,
                ],
                gradient: SceneGradient::Linear {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 50.0,
                    y1: 0.0,
                    stops: vec![
                        SceneGradientStop {
                            offset: 1.0,
                            r: 1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        SceneGradientStop {
                            offset: 0.0,
                            r: 0.0,
                            g: 0.0,
                            b: 1.0,
                            a: 1.0,
                        },
                    ],
                },
            }],
        };
        emit_scene_layer(
            &mut list,
            &layer,
            Transform::IDENTITY,
            (200.0, 100.0),
            |_, _, _| {},
        );
        // PushClip, FillPath(gradient), PopClip.
        assert_eq!(list.commands.len(), 3);
        let Some(DisplayCommand::FillPath { paint, .. }) = list.commands.get(1) else {
            panic!("expected a gradient FillPath at [1]");
        };
        let Paint::LinearGradient(gid) = paint else {
            panic!("expected a LinearGradient paint, got {paint:?}");
        };
        let grad = list.linear_gradient(*gid).expect("gradient pooled");
        assert_eq!(grad.stops.len(), 2);
        assert!(
            grad.stops[0].offset < grad.stops[1].offset,
            "stops sorted by offset"
        );
        assert_eq!(grad.start, (0.0, 0.0));
        assert_eq!(grad.end, (50.0, 0.0));
    }

    #[test]
    fn degenerate_gradient_is_skipped_not_panicked() {
        // <2 stops (and, for radial, a non-positive radius) emit no fill.
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::FillPathGradient {
                path: line(0.0, 0.0, 10.0, 10.0),
                gradient: SceneGradient::Linear {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 10.0,
                    y1: 0.0,
                    stops: vec![SceneGradientStop {
                        offset: 0.0,
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }],
                },
            }],
        };
        emit_scene_layer(
            &mut list,
            &layer,
            Transform::IDENTITY,
            (100.0, 100.0),
            |_, _, _| {},
        );
        assert!(
            !list
                .commands
                .iter()
                .any(|c| matches!(c, DisplayCommand::FillPath { .. })),
            "a 1-stop gradient is skipped"
        );
    }

    #[test]
    fn image_item_interns_pixels_and_emits_clipped_image() {
        // A 2×2 red block placed at content rect (10,20,40,30) inside a
        // 200×100 content box, the frame translated to page (50,80).
        let outer = Transform::translate(50.0, 80.0);
        let mut list = DisplayList::new();
        #[rustfmt::skip]
        let red2x2: Vec<u8> = vec![
            255, 0, 0, 255,  255, 0, 0, 255,
            255, 0, 0, 255,  255, 0, 0, 255,
        ];
        let layer = SceneLayer {
            items: vec![SceneItem::Image {
                rgba: red2x2.clone(),
                width: 2,
                height: 2,
                x: 10.0,
                y: 20.0,
                w: 40.0,
                h: 30.0,
            }],
        };
        emit_scene_layer(&mut list, &layer, outer, (200.0, 100.0), |_, _, _| {});
        // PushClip, Image, PopClip — bracketed in the content-box clip.
        assert_eq!(list.commands.len(), 3);
        assert!(matches!(list.commands[0], DisplayCommand::PushClip { .. }));
        assert!(matches!(list.commands[2], DisplayCommand::PopClip(_)));
        // Pixels interned into the display-list image pool, intact.
        assert_eq!(list.images.len(), 1);
        assert_eq!(list.images[0].width, 2);
        assert_eq!(list.images[0].height, 2);
        assert_eq!(&list.images[0].rgba[..], &red2x2[..]);
        // The Image command maps the image's unit square onto the dest
        // rect AND carries the frame's page translation: (0,0) -> dest
        // top-left (10,20) -> page (60,100); (1,1) -> (50,50) -> (100,130).
        let DisplayCommand::Image { transform, .. } = list.commands[1] else {
            panic!("expected an Image command");
        };
        let (tlx, tly) = transform.apply(0.0, 0.0);
        let (brx, bry) = transform.apply(1.0, 1.0);
        assert!((tlx - 60.0).abs() < 1e-4 && (tly - 100.0).abs() < 1e-4, "tl=({tlx},{tly})");
        assert!((brx - 100.0).abs() < 1e-4 && (bry - 130.0).abs() < 1e-4, "br=({brx},{bry})");
    }

    #[test]
    fn image_item_with_a_malformed_buffer_is_skipped_not_panicked() {
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::Image {
                rgba: vec![255, 0, 0], // 3 bytes, not 2*2*4 = 16
                width: 2,
                height: 2,
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            }],
        };
        emit_scene_layer(&mut list, &layer, Transform::IDENTITY, (50.0, 50.0), |_, _, _| {});
        // Nothing interned, no Image command (the malformed item is skipped).
        assert!(list.images.is_empty());
        assert!(!list
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::Image { .. })));
    }

    #[test]
    fn content_outer_transforms_item_geometry_into_page_space() {
        // A frame translated to page (50, 80): a content-space point
        // (10, 20) must carry that translation in the command transform.
        let outer = Transform::translate(50.0, 80.0);
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::FillPath {
                path: vec![
                    ScenePathSeg::MoveTo { x: 10.0, y: 20.0 },
                    ScenePathSeg::LineTo { x: 30.0, y: 20.0 },
                    ScenePathSeg::LineTo { x: 30.0, y: 40.0 },
                    ScenePathSeg::Close,
                ],
                paint: black(),
            }],
        };
        emit_scene_layer(&mut list, &layer, outer, (0.0, 0.0), |_, _, _| {}); // no clip
                                                                              // No clip (size 0) → exactly one FillPath, transform == content_outer.
        assert_eq!(list.commands.len(), 1);
        let DisplayCommand::FillPath { transform, .. } = &list.commands[0] else {
            panic!("expected FillPath");
        };
        // Content point (10,20) lands at page (60,100).
        assert_eq!(transform.apply(10.0, 20.0), (60.0, 100.0));
    }

    #[test]
    fn rotated_frame_clip_is_a_rotated_box_not_axis_aligned() {
        // §8.5: a rotated frame clips to a rotated content box (the plugin
        // does NOT compensate). With a 90°-rotation content_outer, the
        // clip path's first corner (0,0) maps through the rotation.
        let outer = Transform::rotate_deg(90.0).compose(&Transform::translate(0.0, 0.0));
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::FillPath {
                path: line(0.0, 0.0, 10.0, 0.0)
                    .into_iter()
                    .chain([ScenePathSeg::Close])
                    .collect(),
                paint: black(),
            }],
        };
        emit_scene_layer(&mut list, &layer, outer, (40.0, 20.0), |_, _, _| {});
        let DisplayCommand::PushClip { transform, .. } = &list.commands[0] else {
            panic!("expected PushClip first");
        };
        // The content-box corner (40,0) rotates 90° CW about the origin to
        // roughly (0,40) — i.e. NOT left where an axis-aligned clip would.
        let (px, py) = transform.apply(40.0, 0.0);
        assert!(px.abs() < 1e-3, "x≈0, got {px}");
        assert!((py - 40.0).abs() < 1e-3, "y≈40, got {py}");
    }

    #[test]
    fn srgb_paint_is_linearised() {
        // Mid-grey sRGB 0.5 → ~0.214 linear (not 0.5).
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::FillPath {
                path: vec![
                    ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
                    ScenePathSeg::LineTo { x: 1.0, y: 0.0 },
                    ScenePathSeg::Close,
                ],
                paint: ScenePaint {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                },
            }],
        };
        emit_scene_layer(
            &mut list,
            &layer,
            Transform::IDENTITY,
            (0.0, 0.0),
            |_, _, _| {},
        );
        let DisplayCommand::FillPath {
            paint: Paint::Solid(c),
            ..
        } = &list.commands[0]
        else {
            panic!("expected solid fill");
        };
        assert!((c.r - 0.214).abs() < 0.01, "linearised, got {}", c.r);
        assert_eq!(c.a, 1.0, "alpha stays linear");
    }

    #[test]
    fn text_item_routes_to_the_emit_text_callback_with_content_transform() {
        // A Text item is NOT lowered by paged-compose (no fonts) — it is
        // handed to the renderer's text emitter, inside the clip bracket,
        // carrying the content transform so the renderer positions glyphs
        // at the transformed baseline.
        let outer = Transform::translate(50.0, 80.0);
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::Text(SceneTextItem {
                x: 10.0,
                y: 20.0,
                text: "42".to_string(),
                size: 12.0,
                paint: black(),
                family: None,
                style: None,
            })],
        };
        let mut seen: Vec<(String, (f32, f32))> = Vec::new();
        emit_scene_layer(&mut list, &layer, outer, (0.0, 0.0), |_list, t, xf| {
            seen.push((t.text.clone(), xf.apply(t.x, t.y)));
        });
        // The callback saw the run + the transformed baseline (60, 100).
        assert_eq!(seen, vec![("42".to_string(), (60.0, 100.0))]);
        // paged-compose emitted no glyph commands itself (renderer's job).
        assert!(list.commands.is_empty());
    }

    #[test]
    fn empty_text_run_is_skipped() {
        let mut list = DisplayList::new();
        let layer = SceneLayer {
            items: vec![SceneItem::Text(SceneTextItem {
                x: 0.0,
                y: 0.0,
                text: String::new(),
                size: 12.0,
                paint: black(),
                family: None,
                style: None,
            })],
        };
        let mut called = false;
        emit_scene_layer(
            &mut list,
            &layer,
            Transform::IDENTITY,
            (0.0, 0.0),
            |_, _, _| {
                called = true;
            },
        );
        assert!(!called, "an empty text run does not call the emitter");
    }
}
