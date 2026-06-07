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

//! Placed-image emission for rectangle / polygon / oval hosts:
//! link resolution (URI, inline base64, decode-failure triage),
//! fit/crop transforms, clipping, and the per-page image cache.

use super::*;

/// Record a render diagnostic for a placed image that didn't resolve.
/// Shared by the rectangle / polygon / oval image emitters so the
/// "missing link" / "decode failed" signals are reported consistently.
/// A no-op for `Resolved`, for non-image frames, and for the inline-PDF
/// fall-through (which intentionally renders the frame fill instead of a
/// placeholder). Pushes onto `page.diagnostics`; the document-level
/// aggregator backfills `page_index`.
fn report_image_resolution(
    page: &mut BuiltPage,
    resolution: &ImageResolution,
    has_image_element: bool,
    has_inline_pdf: bool,
    uri: Option<&str>,
    frame_id: Option<&str>,
) {
    let (code, msg) = match resolution {
        ImageResolution::DecodeFailed if has_image_element => (
            DiagnosticCode::ImageDecodeFailed,
            "placed image could not be decoded; frame fill used instead",
        ),
        ImageResolution::LinkMissing if has_image_element && !has_inline_pdf => (
            DiagnosticCode::ImageLinkMissing,
            "placed image link could not be resolved; placeholder drawn",
        ),
        _ => return,
    };
    let mut d = Diagnostic::new(code, msg);
    if let Some(u) = uri {
        d = d.with_uri(u);
    }
    if let Some(f) = frame_id {
        d = d.with_frame(f);
    }
    page.diagnostics.push(d);
}

/// Resolve, decode, and emit a placed image for a rectangle. Skips
/// silently if `assets` is unset, the resolver returns `None`, or
/// decoding fails — IDMLs without their linked assets should still
/// produce a usable render of the surrounding geometry.
///
/// Two placement paths:
///   1. *Inner* `<Image ItemTransform="...">` present (the
///      InDesign-export shape). The image's pixel rect (0..w, 0..h)
///      maps through that transform into the frame's inner coord
///      space, then through the frame's ItemTransform into spread
///      coords. The image is then *clipped* to the frame's path so
///      cropping / partial-frame placements (a thin slice, a square
///      crop, etc.) match InDesign.
///   2. No inner transform (synthetic IDMLs that omit it). Fall
///      back to the legacy "stretch image to frame bounds — minus
///      `<FrameFittingOption>` crops" path. No clip is needed
///      because the image already covers exactly the frame's AABB.
pub(super) fn emit_rectangle_image(
    page: &mut BuiltPage,
    rect: &Rectangle,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, paged_compose::ImageId>,
    decoded_cache: &mut HashMap<String, paged_compose::DecodedImage>,
) {
    // Routing: a `<Image>`-bearing frame whose link is *missing*
    // gets InDesign's 50% grey + diagonal-X placeholder. A link that
    // resolves but whose payload our decoder can't handle (Q-14) is
    // a different case — InDesign still rasterises it, so we fall
    // through to the frame's intrinsic FillColor instead of stamping
    // a "missing image" badge over what should be real content.
    //
    // Q-03: inline base64 `<Image><Contents>` bytes take precedence
    // over `LinkResourceURI` — when the IDML embeds the JPEG directly
    // we decode straight from those bytes regardless of whether an
    // asset resolver is wired up.
    let resolved = if let Some(bytes) = rect.image_bytes.as_deref() {
        resolve_inline_image_bytes(bytes, &mut page.list, page_image_cache, decoded_cache)
    } else {
        match rect.image_link.as_deref() {
            Some(uri) => resolve_image_id(
                uri,
                options,
                &mut page.list,
                page_image_cache,
                decoded_cache,
            ),
            None => ImageResolution::LinkMissing,
        }
    };
    report_image_resolution(
        page,
        &resolved,
        rect.has_image_element,
        rect.has_inline_pdf,
        rect.image_link.as_deref(),
        rect.self_id.as_deref(),
    );
    let outer = frame_outer_transform(page, rect.item_transform);
    let (id, img_w, img_h) = match resolved {
        ImageResolution::Resolved(id, w, h) => (id, w, h),
        ImageResolution::DecodeFailed => return,
        ImageResolution::LinkMissing => {
            // Q-06: inline `<PDF>` content we can't decode → fall
            // through to the frame's intrinsic FillColor (already
            // emitted by the earlier shape-fill pass) rather than
            // stamping the grey-X missing-image placeholder over it.
            if rect.has_image_element && !rect.has_inline_pdf && options.missing_image_placeholder {
                emit_rectangle_missing_image_placeholder(page, rect, outer);
            }
            return;
        }
    };

    if let Some(image_t) = rect.image_item_transform {
        // Path 1: honour the inner Image[ItemTransform]. The image's
        // natural pixel rect (0..w, 0..h) — IDML treats placed-image
        // pixels as 1pt at 72ppi — maps through `image_t` into
        // frame-inner coords, then through `outer` (= page_origin ·
        // frame.ItemTransform) into spread → page coords.
        //
        // `Transform::for_rect_in(rect, t)` builds
        //   t · scale(rect.w, rect.h) · translate(rect.x, rect.y)
        // so passing rect=(0,0,w,h) plus a composed `outer ∘ image_t`
        // gives us exactly the placement we need.
        let composed = outer.compose(&Transform(image_t));
        let img_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: img_w,
            h: img_h,
        };
        // Clip to the frame's rectangular path (in inner coords).
        // We use the rectangle's `bounds` AABB: IDML rectangles are
        // axis-aligned in inner space by definition, so the AABB
        // equals the path. Any rotation/shear lives on `outer`,
        // which we share with the image emission below. Polygon-
        // hosted images (curved frames) aren't part of this slice.
        let clip_rect = paged_compose::Rect {
            x: rect.bounds.left,
            y: rect.bounds.top,
            w: rect.bounds.width(),
            h: rect.bounds.height(),
        };
        // W1.21: an optional detached clipping path, masked on top of
        // the frame box. Its anchors are in image-pixel space, so it
        // rides the same `composed` transform as the image itself.
        let extra_clip = resolve_image_clip(
            page,
            rect.image_clip.as_ref(),
            composed,
            img_w,
            img_h,
            rect.image_link.as_deref(),
            rect.self_id.as_deref(),
        );
        emit_clipped_image(
            &mut page.list,
            clip_rect,
            outer,
            img_rect,
            composed,
            id,
            extra_clip,
        );
    } else {
        // Path 2: legacy synthetic-IDML placement. No inner
        // transform ⇒ fit the image to the frame's bounds (minus
        // FrameFitting crops). No clip — the image already
        // occupies exactly the rect.
        let frame_left = rect.bounds.left;
        let frame_top = rect.bounds.top;
        let frame_w = rect.bounds.width();
        let frame_h = rect.bounds.height();
        let (left_crop, top_crop, right_crop, bottom_crop) = rect
            .frame_fitting
            .as_ref()
            .map(|f| {
                (
                    f.left_crop.unwrap_or(0.0),
                    f.top_crop.unwrap_or(0.0),
                    f.right_crop.unwrap_or(0.0),
                    f.bottom_crop.unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0, 0.0, 0.0));
        let r = Rect {
            x: frame_left + left_crop,
            y: frame_top + top_crop,
            w: (frame_w - left_crop - right_crop).max(0.0),
            h: (frame_h - top_crop - bottom_crop).max(0.0),
        };
        // W1.21: a clip path on a no-inner-transform placement would
        // need an image-pixel→frame mapping we don't synthesise here
        // (real InDesign exports always carry the inner ItemTransform).
        // Record the defer so the clip isn't silently dropped.
        report_clip_for_untransformed_image(
            page,
            rect.image_clip.as_ref(),
            rect.image_link.as_deref(),
            rect.self_id.as_deref(),
        );
        paged_compose::emit_image_at(r, outer, id, &mut page.list);
    }
}

/// Polygon-hosted placed image. Mirrors [`emit_rectangle_image`] but
/// uses the polygon's curved `PathPointType` anchors as the clip
/// shape so the image hugs the polygon's outline rather than its
/// bounding rectangle. When the polygon has no anchors (synthetic
/// IDMLs declaring a polygon via `GeometricBounds` only), the
/// rectangle path falls through to a flat AABB clip — visually
/// identical to the rectangle case.
pub(super) fn emit_polygon_image(
    page: &mut BuiltPage,
    poly: &Polygon,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, paged_compose::ImageId>,
    decoded_cache: &mut HashMap<String, paged_compose::DecodedImage>,
) {
    let resolved = if let Some(bytes) = poly.image_bytes.as_deref() {
        resolve_inline_image_bytes(bytes, &mut page.list, page_image_cache, decoded_cache)
    } else {
        match poly.image_link.as_deref() {
            Some(uri) => resolve_image_id(
                uri,
                options,
                &mut page.list,
                page_image_cache,
                decoded_cache,
            ),
            None => ImageResolution::LinkMissing,
        }
    };
    report_image_resolution(
        page,
        &resolved,
        poly.has_image_element,
        poly.has_inline_pdf,
        poly.image_link.as_deref(),
        poly.self_id.as_deref(),
    );
    let outer = frame_outer_transform(page, poly.item_transform);
    let (id, img_w, img_h) = match resolved {
        ImageResolution::Resolved(id, w, h) => (id, w, h),
        ImageResolution::DecodeFailed => return,
        ImageResolution::LinkMissing => {
            if poly.has_image_element && !poly.has_inline_pdf && options.missing_image_placeholder {
                emit_polygon_missing_image_placeholder(page, poly, outer);
            }
            return;
        }
    };

    // Build (or reuse) the polygon's clip path. Falls back to the
    // bounds AABB when the polygon carries no Bezier anchors. Honours
    // `subpath_open` so open contours don't get auto-closed when used
    // as an image clip (P-15).
    let clip_path_id = if !poly.anchors.is_empty() {
        let path = polygon_path_from_anchors_with_open(
            &poly.anchors,
            &poly.subpath_starts,
            &poly.subpath_open,
        );
        let cache_key = match poly.self_id.as_deref() {
            Some(sid) => fnv_1a_u64(sid.as_bytes()),
            None => path_signature(&poly.anchors),
        };
        let (id, _) = page.list.paths.intern(cache_key, path);
        id
    } else {
        // Anchor-less polygon: synthesise the AABB unit rect path
        // (same key as rectangles use, so both share one entry).
        const CLIP_UNIT_RECT_KEY: u64 = 0x1d_4c_69_70_5f_72_65_63;
        let path = paged_compose::PathData {
            segments: vec![
                paged_compose::PathSegment::MoveTo { x: 0.0, y: 0.0 },
                paged_compose::PathSegment::LineTo { x: 1.0, y: 0.0 },
                paged_compose::PathSegment::LineTo { x: 1.0, y: 1.0 },
                paged_compose::PathSegment::LineTo { x: 0.0, y: 1.0 },
                paged_compose::PathSegment::Close,
            ],
        };
        let (id, _) = page.list.paths.intern(CLIP_UNIT_RECT_KEY, path);
        id
    };

    // Pick a clip transform: anchor-bearing polygons keep their path
    // in inner coords (already in the right space) so `outer` maps
    // directly. Anchor-less polygons need the unit-rect-to-bounds
    // scale baked in — same shape as `emit_clipped_image`.
    let clip_transform = if !poly.anchors.is_empty() {
        outer
    } else {
        let clip_rect = paged_compose::Rect {
            x: poly.bounds.left,
            y: poly.bounds.top,
            w: poly.bounds.width(),
            h: poly.bounds.height(),
        };
        Transform::for_rect_in(clip_rect, outer)
    };

    let img_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: img_w,
        h: img_h,
    };
    let image_transform = if let Some(image_t) = poly.image_item_transform {
        outer.compose(&Transform(image_t))
    } else {
        // Stretch the image across the polygon's bounds (same fallback
        // as the synthetic rectangle case).
        let frame_rect = Rect {
            x: poly.bounds.left,
            y: poly.bounds.top,
            w: poly.bounds.width(),
            h: poly.bounds.height(),
        };
        // Compose the bounds-scale into outer so emit_image_under_clip
        // ends up with `outer ∘ scale(w,h) ∘ translate(left,top)`.
        Transform::for_rect_in(frame_rect, outer)
    };
    // When no inner image transform is present, `image_transform`
    // already encodes the for_rect_in math; for_rect_in below would
    // double-scale. Branch: pass a unit rect so emit_image_under_clip
    // doesn't multiply by img_w/img_h again.
    if poly.image_item_transform.is_some() {
        // W1.21: clip anchors are in image-pixel space → same
        // `image_transform` (= outer ∘ image_t) as the image.
        let extra_clip = resolve_image_clip(
            page,
            poly.image_clip.as_ref(),
            image_transform,
            img_w,
            img_h,
            poly.image_link.as_deref(),
            poly.self_id.as_deref(),
        );
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            img_rect,
            image_transform,
            id,
            extra_clip,
        );
    } else {
        report_clip_for_untransformed_image(
            page,
            poly.image_clip.as_ref(),
            poly.image_link.as_deref(),
            poly.self_id.as_deref(),
        );
        let unit = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            unit,
            image_transform,
            id,
            None,
        );
    }
}

/// Mirror of `emit_polygon_image` for `<Oval>` frames hosting placed
/// images. The clip path is the unit ellipse (interned at a stable
/// key so multiple ovals share the same path); the image fits the
/// oval's bounds unless the inner `<Image ItemTransform>` overrides
/// it (P-16).
pub(super) fn emit_oval_image(
    page: &mut BuiltPage,
    oval: &Oval,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, paged_compose::ImageId>,
    decoded_cache: &mut HashMap<String, paged_compose::DecodedImage>,
) {
    let resolved = if let Some(bytes) = oval.image_bytes.as_deref() {
        resolve_inline_image_bytes(bytes, &mut page.list, page_image_cache, decoded_cache)
    } else {
        match oval.image_link.as_deref() {
            Some(uri) => resolve_image_id(
                uri,
                options,
                &mut page.list,
                page_image_cache,
                decoded_cache,
            ),
            None => ImageResolution::LinkMissing,
        }
    };
    report_image_resolution(
        page,
        &resolved,
        oval.has_image_element,
        oval.has_inline_pdf,
        oval.image_link.as_deref(),
        oval.self_id.as_deref(),
    );
    let outer = frame_outer_transform(page, oval.item_transform);
    let (id, img_w, img_h) = match resolved {
        ImageResolution::Resolved(id, w, h) => (id, w, h),
        ImageResolution::DecodeFailed => return,
        ImageResolution::LinkMissing => {
            if oval.has_image_element && !oval.has_inline_pdf && options.missing_image_placeholder {
                emit_oval_missing_image_placeholder(page, oval, outer);
            }
            return;
        }
    };

    // Clip to the oval's parametric ellipse (unit-rect-scaled to the
    // frame's bounds via the outer affine). UNIT_ELLIPSE_KEY is the
    // same interner key the fill / stroke paths use, so the path is
    // shared across all ovals.
    let bounds = oval.bounds;
    let clip_rect = paged_compose::Rect {
        x: bounds.left,
        y: bounds.top,
        w: bounds.width(),
        h: bounds.height(),
    };
    let (clip_path_id, _) = page
        .list
        .paths
        .intern(paged_compose::UNIT_ELLIPSE_KEY, unit_ellipse_path());
    let clip_transform = Transform::for_rect_in(clip_rect, outer);

    let img_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: img_w,
        h: img_h,
    };
    let image_transform = if let Some(image_t) = oval.image_item_transform {
        outer.compose(&Transform(image_t))
    } else {
        Transform::for_rect_in(clip_rect, outer)
    };
    if oval.image_item_transform.is_some() {
        let extra_clip = resolve_image_clip(
            page,
            oval.image_clip.as_ref(),
            image_transform,
            img_w,
            img_h,
            oval.image_link.as_deref(),
            oval.self_id.as_deref(),
        );
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            img_rect,
            image_transform,
            id,
            extra_clip,
        );
    } else {
        report_clip_for_untransformed_image(
            page,
            oval.image_clip.as_ref(),
            oval.image_link.as_deref(),
            oval.self_id.as_deref(),
        );
        let unit = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            unit,
            image_transform,
            id,
            None,
        );
    }
}

/// `clip_rect` is the frame's inner-coord AABB; `clip_outer` is the
/// frame's outer transform (page_origin · ItemTransform).
/// `image_rect` is `(0, 0, img_w, img_h)` and `image_transform` is
/// the composed `outer ∘ image_item_transform`. Sharing `outer` for
/// both keeps clip and image in lockstep when the frame rotates —
/// the unit-rect clip turns into the host's rotated quad under
/// `clip_outer`, which is the right behaviour for axis-aligned
/// rectangles regardless of the host's rotation.
fn emit_clipped_image(
    list: &mut paged_compose::DisplayList,
    clip_rect: paged_compose::Rect,
    clip_outer: Transform,
    image_rect: paged_compose::Rect,
    image_transform: Transform,
    image_id: paged_compose::ImageId,
    extra_clip: Option<(paged_compose::PathId, Transform)>,
) {
    use paged_compose::PathSegment;
    // Unit-rect path interned under a stable key so multiple clipped-
    // image emissions share the same entry in the path buffer.
    const CLIP_UNIT_RECT_KEY: u64 = 0x1d_4c_69_70_5f_72_65_63; // "idClip_rec"
    let path = paged_compose::PathData {
        segments: vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 1.0 },
            PathSegment::LineTo { x: 0.0, y: 1.0 },
            PathSegment::Close,
        ],
    };
    let (clip_path_id, _) = list.paths.intern(CLIP_UNIT_RECT_KEY, path);
    let clip_transform = Transform::for_rect_in(clip_rect, clip_outer);
    emit_image_under_clip(
        list,
        clip_path_id,
        clip_transform,
        image_rect,
        image_transform,
        image_id,
        extra_clip,
    );
}

/// Push an arbitrary clip path, emit an image, then pop. Splits the
/// PushClip / Image / PopClip emission off `emit_clipped_image` so
/// the polygon-hosted image variant (used when the host is a curved
/// `<Polygon>` frame) can supply its own pre-interned path.
///
/// W1.21: `extra_clip` is an optional *second* clip (the image's
/// detached clipping path) pushed inside the frame clip. The
/// rasterizer intersects clips, so the image ends up masked to
/// `frame ∩ clip-path`. Pushed after the frame clip and popped before
/// it so the stack stays balanced.
fn emit_image_under_clip(
    list: &mut paged_compose::DisplayList,
    clip_path_id: paged_compose::PathId,
    clip_transform: Transform,
    image_rect: paged_compose::Rect,
    image_transform: Transform,
    image_id: paged_compose::ImageId,
    extra_clip: Option<(paged_compose::PathId, Transform)>,
) {
    use paged_compose::DisplayCommand;
    list.push(DisplayCommand::PushClip {
        path_id: clip_path_id,
        transform: clip_transform,
    });
    if let Some((path_id, transform)) = extra_clip {
        list.push(DisplayCommand::PushClip { path_id, transform });
    }
    let img_transform = Transform::for_rect_in(image_rect, image_transform);
    list.push(DisplayCommand::Image {
        image_id,
        transform: img_transform,
    });
    if extra_clip.is_some() {
        list.push(DisplayCommand::PopClip(Transform::IDENTITY));
    }
    list.push(DisplayCommand::PopClip(Transform::IDENTITY));
}

/// Resolve a `LinkResourceURI` to a renderer `ImageId` plus its
/// natural pixel dimensions, threading through both the per-page
/// `ImageId` cache and the renderer-scoped decoded-bytes cache.
/// Returns `None` for any failure along the resolver / decode chain
/// (no resolver, resolver miss, undecodable bytes, zero-pixel image)
/// so callers can fall back to a missing-image placeholder.
/// Outcome of `resolve_image_id`. `LinkMissing` means the IDML
/// referenced a link the asset resolver couldn't find (typical Envato
/// template placeholder — InDesign stamps a grey-X placeholder over
/// these). `DecodeFailed` means the resolver returned bytes our
/// decoder can't handle (oversized JPEG, unsupported PSD layers,
/// streaming-only formats); in that case InDesign would still
/// rasterise the actual content, so falling back to the
/// missing-image placeholder is worse than emitting the frame's
/// intrinsic FillColor (Q-14).
enum ImageResolution {
    Resolved(paged_compose::ImageId, f32, f32),
    DecodeFailed,
    LinkMissing,
}

fn resolve_image_id(
    uri: &str,
    options: &PipelineOptions,
    list: &mut paged_compose::DisplayList,
    page_image_cache: &mut HashMap<String, paged_compose::ImageId>,
    decoded_cache: &mut HashMap<String, paged_compose::DecodedImage>,
) -> ImageResolution {
    let id = match page_image_cache.get(uri).copied() {
        Some(id) => id,
        None => {
            let decoded = if let Some(d) = decoded_cache.get(uri) {
                d.clone()
            } else {
                let Some(resolver) = options.assets else {
                    return ImageResolution::LinkMissing;
                };
                let Some(bytes) = resolver.resolve_image(uri) else {
                    tracing::warn!(uri, "image resolver returned no bytes; skipping");
                    return ImageResolution::LinkMissing;
                };
                let Some(d) = decode_image_bytes(bytes.as_ref()) else {
                    tracing::warn!(uri, "image decode failed; skipping");
                    return ImageResolution::DecodeFailed;
                };
                decoded_cache.insert(uri.to_string(), d.clone());
                d
            };
            let id = list.push_image(decoded);
            page_image_cache.insert(uri.to_string(), id);
            id
        }
    };
    let (img_w, img_h) = match list.image(id) {
        Some(d) => (d.width as f32, d.height as f32),
        None => return ImageResolution::DecodeFailed,
    };
    if img_w <= 0.0 || img_h <= 0.0 {
        return ImageResolution::DecodeFailed;
    }
    ImageResolution::Resolved(id, img_w, img_h)
}

/// Q-03: route inline base64 image bytes (the `<Contents>` payload
/// captured by the parser) through the same per-page + decoded
/// caches `resolve_image_id` uses. Cache key is the bytes' allocation
/// address — stable across reuses inside a single render pass, and
/// distinct per frame so two Rectangles with the same inline image
/// share the decoded result.
fn resolve_inline_image_bytes(
    bytes: &[u8],
    list: &mut paged_compose::DisplayList,
    page_image_cache: &mut HashMap<String, paged_compose::ImageId>,
    decoded_cache: &mut HashMap<String, paged_compose::DecodedImage>,
) -> ImageResolution {
    let key = format!("inline:{:p}:{}", bytes.as_ptr(), bytes.len());
    let id = match page_image_cache.get(&key).copied() {
        Some(id) => id,
        None => {
            let decoded = if let Some(d) = decoded_cache.get(&key) {
                d.clone()
            } else {
                let Some(d) = decode_image_bytes(bytes) else {
                    tracing::warn!(len = bytes.len(), "inline image decode failed; skipping");
                    return ImageResolution::DecodeFailed;
                };
                decoded_cache.insert(key.clone(), d.clone());
                d
            };
            let id = list.push_image(decoded);
            page_image_cache.insert(key, id);
            id
        }
    };
    let (img_w, img_h) = match list.image(id) {
        Some(d) => (d.width as f32, d.height as f32),
        None => return ImageResolution::DecodeFailed,
    };
    if img_w <= 0.0 || img_h <= 0.0 {
        return ImageResolution::DecodeFailed;
    }
    ImageResolution::Resolved(id, img_w, img_h)
}

// ── W1.21: image clipping paths ──────────────────────────────────────
//
// InDesign clips a placed image to a *detached* clipping path in
// addition to the frame outline. The IDML serialises this under the
// `<Image>` as `<ClippingPathSettings>`; for a `UserModifiedPath` the
// resolved geometry rides along as a `<PathGeometry>` (anchors in the
// image's pixel space — the same space the image's own PathGeometry and
// `ItemTransform` use). PhotoshopPath / AlphaChannel / DetectEdges keep
// the geometry in the image binary, so without 8BIM/raster analysis we
// defer (render frame-clipped only + a diagnostic).
//
// Composition: the renderer already pushes the *frame* clip around the
// image (PushClip frame → Image → PopClip). We push a *second* clip —
// the clip path — inside that, so the rasterizer's clip-stack
// intersection yields `frame ∩ clip-path` for free. The clip path's
// transform is exactly the image's placement transform (`composed`),
// since the anchors are in image-pixel coordinates.
//
// Holes & invert under NonZero: the clip mask is filled `NonZero`, so we
// normalise winding ourselves (nested contours alternate orientation →
// holes survive) and express `InvertPath` as a compound path —
// (image-pixel bounding box) minus (path) — which, intersected with the
// frame clip, keeps the area *outside* the path.

/// Signed area (shoelace) of one closed contour, sampled at its anchor
/// on-curve points. Sign distinguishes orientation; magnitude ranks
/// outer-vs-inner contours. Bezier handles are ignored — for winding
/// classification the polygon of on-curve points is sufficient (the
/// clip paths InDesign writes are well-behaved nested loops).
fn clip_contour_signed_area(anchors: &[PathAnchor]) -> f32 {
    let n = anchors.len();
    if n < 3 {
        return 0.0;
    }
    let mut acc = 0.0f32;
    for i in 0..n {
        let (x0, y0) = anchors[i].anchor;
        let (x1, y1) = anchors[(i + 1) % n].anchor;
        acc += x0 * y1 - x1 * y0;
    }
    acc * 0.5
}

/// Split an anchor list into per-contour slices using `subpath_starts`
/// (one entry per `<GeometryPathType>`; empty/single-entry ⇒ one
/// contour). Mirrors `polygon_path_from_anchors_with_open`'s range
/// logic so the two stay consistent.
fn clip_contour_ranges(anchors: &[PathAnchor], subpath_starts: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    if subpath_starts.len() <= 1 {
        if !anchors.is_empty() {
            ranges.push((0, anchors.len()));
        }
        return ranges;
    }
    let mut starts: Vec<usize> = subpath_starts
        .iter()
        .copied()
        .filter(|&s| s < anchors.len())
        .collect();
    starts.sort_unstable();
    starts.dedup();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    for i in 0..starts.len() {
        let lo = starts[i];
        let hi = starts.get(i + 1).copied().unwrap_or(anchors.len());
        if hi > lo {
            ranges.push((lo, hi));
        }
    }
    ranges
}

/// Reverse a contour's anchor list (anchor stays put, but the left/right
/// Bezier handles swap) so its winding flips. Used to alternate nested-
/// contour orientation for NonZero hole rendering and to punch the path
/// out of the invert bounding box.
fn reversed_contour(sub: &[PathAnchor]) -> Vec<PathAnchor> {
    sub.iter()
        .rev()
        .map(|a| PathAnchor {
            anchor: a.anchor,
            // Swap handles: a point's outgoing tangent becomes its
            // incoming one when the path direction reverses.
            left: a.right,
            right: a.left,
        })
        .collect()
}

/// Build the clip-path [`PathData`] (image-pixel space) for a placed
/// image, ready to push as a NonZero clip mask. Returns `None` when the
/// settings carry no renderable geometry.
///
/// * Holes: contours are classified by enclosed area. The largest is the
///   outer boundary; every other contour is re-wound to the *opposite*
///   orientation so NonZero leaves it as a hole (a doughnut / star-with-
///   a-punched-centre clip). This is the standard even-odd→nonzero
///   normalisation for non-self-intersecting nested loops.
/// * Invert (`InvertPath="true"`): the result is `bbox − path` — an
///   outer rectangle covering the image pixel box, with every path
///   contour wound opposite so NonZero subtracts it. Intersected with
///   the frame clip downstream, this keeps the area *outside* the path.
///   `pixel_w` / `pixel_h` size that bounding rect; the path is assumed
///   to live within `[0, pixel_w] × [0, pixel_h]` (the image's own
///   PathGeometry extent).
fn build_image_clip_path(
    clip: &paged_parse::ClippingPathSettings,
    pixel_w: f32,
    pixel_h: f32,
) -> Option<PathData> {
    let anchors = &clip.clip_anchors;
    if anchors.is_empty() {
        return None;
    }
    let ranges = clip_contour_ranges(anchors, &clip.clip_subpath_starts);
    if ranges.is_empty() {
        return None;
    }

    // Rank contours by |area| to find the outer boundary. Nested loops
    // become holes for a normal clip; with `InvertPath` every contour is
    // subtracted from the bounding box instead.
    let areas: Vec<f32> = ranges
        .iter()
        .map(|&(lo, hi)| clip_contour_signed_area(&anchors[lo..hi]))
        .collect();
    let outer_idx = areas
        .iter()
        .enumerate()
        .max_by(|a, b| {
            a.1.abs()
                .partial_cmp(&b.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    let outer_sign = areas.get(outer_idx).copied().unwrap_or(1.0).signum();

    let mut out_anchors: Vec<PathAnchor> = Vec::with_capacity(anchors.len() + 4);
    let mut starts: Vec<usize> = Vec::with_capacity(ranges.len() + 1);
    let mut opens: Vec<bool> = Vec::with_capacity(ranges.len() + 1);

    // Invert: prepend a bounding rectangle covering the image pixel box.
    // Every real path contour is then forced to the OPPOSITE winding so
    // NonZero subtracts it from the box.
    let mut bbox_sign = 0.0f32;
    if clip.invert_path {
        let bbox = [
            PathAnchor {
                anchor: (0.0, 0.0),
                left: (0.0, 0.0),
                right: (0.0, 0.0),
            },
            PathAnchor {
                anchor: (pixel_w, 0.0),
                left: (pixel_w, 0.0),
                right: (pixel_w, 0.0),
            },
            PathAnchor {
                anchor: (pixel_w, pixel_h),
                left: (pixel_w, pixel_h),
                right: (pixel_w, pixel_h),
            },
            PathAnchor {
                anchor: (0.0, pixel_h),
                left: (0.0, pixel_h),
                right: (0.0, pixel_h),
            },
        ];
        bbox_sign = clip_contour_signed_area(&bbox).signum();
        starts.push(out_anchors.len());
        opens.push(false);
        out_anchors.extend_from_slice(&bbox);
    }

    for (ci, &(lo, hi)) in ranges.iter().enumerate() {
        let sub = &anchors[lo..hi];
        let area_sign = areas[ci].signum();
        let want_flip = if clip.invert_path {
            // Force every path contour opposite the bbox so it punches a
            // hole. (Degenerate/zero-area contours keep orientation —
            // they contribute nothing either way.)
            area_sign != 0.0 && area_sign == bbox_sign
        } else {
            // Inner contours (not the largest) flip to oppose the outer
            // so NonZero leaves them as holes.
            ci != outer_idx && area_sign != 0.0 && area_sign == outer_sign
        };
        let materialised = if want_flip {
            reversed_contour(sub)
        } else {
            sub.to_vec()
        };
        starts.push(out_anchors.len());
        opens.push(clip.clip_subpath_open.get(ci).copied().unwrap_or(false));
        out_anchors.extend(materialised);
    }

    // Collapse to the canonical single-contour encoding when there's
    // exactly one closed subpath (matches the frame convention).
    let (starts, opens): (Vec<usize>, Vec<bool>) = if starts.len() <= 1 {
        (Vec::new(), Vec::new())
    } else {
        (starts, opens)
    };

    let path = polygon_path_from_anchors_with_open(&out_anchors, &starts, &opens);
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Record the defer diagnostic for a clipping path the renderer can't
/// honour from the XML (Photoshop path / alpha / detect-edges / named-
/// path-without-geometry). The image still renders, clipped to the frame
/// outline only.
fn report_clip_deferred(
    page: &mut BuiltPage,
    clip: &paged_parse::ClippingPathSettings,
    uri: Option<&str>,
    frame_id: Option<&str>,
) {
    use paged_parse::ClippingType;
    let detail = match clip.clipping_type {
        Some(ClippingType::PhotoshopPath) => "Photoshop 8BIM path",
        Some(ClippingType::AlphaChannel) => "alpha channel",
        Some(ClippingType::DetectEdges) => "detect-edges",
        Some(ClippingType::UserModifiedPath) => "user-modified path with no inline geometry",
        _ => "clipping path",
    };
    let msg = match &clip.applied_path_name {
        Some(name) => format!(
            "image clipping path ({detail}, \"{name}\") not resolvable from IDML; \
             image clipped to frame outline only"
        ),
        None => format!(
            "image clipping path ({detail}) not resolvable from IDML; \
             image clipped to frame outline only"
        ),
    };
    let mut d = Diagnostic::new(DiagnosticCode::ImageClippingPathDeferred, msg);
    if let Some(u) = uri {
        d = d.with_uri(u);
    }
    if let Some(f) = frame_id {
        d = d.with_frame(f);
    }
    page.diagnostics.push(d);
}

/// Resolve a placed image's `<ClippingPathSettings>` into an optional
/// extra clip `(PathId, Transform)` to push around the image, emitting a
/// defer diagnostic when the clip can't be honoured from the XML.
///
/// `image_transform` is the image's placement transform (`composed` —
/// the same affine the `Image` command rides), since clip anchors are in
/// image-pixel space. `pixel_w/h` are the image's native pixel extents,
/// used to size the invert bounding box. Returns `None` for "no clip"
/// (either truly absent, or deferred — in which case a diagnostic is
/// already recorded).
fn resolve_image_clip(
    page: &mut BuiltPage,
    clip: Option<&paged_parse::ClippingPathSettings>,
    image_transform: Transform,
    pixel_w: f32,
    pixel_h: f32,
    uri: Option<&str>,
    frame_id: Option<&str>,
) -> Option<(paged_compose::PathId, Transform)> {
    let clip = clip?;
    if clip.is_deferred_clip() {
        report_clip_deferred(page, clip, uri, frame_id);
        return None;
    }
    if !clip.has_renderable_geometry() {
        return None;
    }
    let path = build_image_clip_path(clip, pixel_w, pixel_h)?;
    // Key on the clip anchors + invert flag so identical clips share one
    // interned path; salt with invert so the same anchors used both ways
    // don't collide.
    let mut key = path_signature(&clip.clip_anchors);
    if clip.invert_path {
        key ^= 0x9e37_79b9_7f4a_7c15;
    }
    let (id, _) = page.list.paths.intern(key, path);
    Some((id, image_transform))
}

/// Defer-diagnostic for a clip on an image placed *without* an inner
/// `<Image ItemTransform>` (the legacy stretch-to-bounds path). We don't
/// synthesise the image-pixel→frame mapping there, so any clip — inline
/// geometry or a deferred type — is recorded as deferred and the image
/// renders frame-clipped only. A no-op when there's no clip.
fn report_clip_for_untransformed_image(
    page: &mut BuiltPage,
    clip: Option<&paged_parse::ClippingPathSettings>,
    uri: Option<&str>,
    frame_id: Option<&str>,
) {
    let Some(clip) = clip else { return };
    if clip.is_deferred_clip() || clip.has_renderable_geometry() {
        report_clip_deferred(page, clip, uri, frame_id);
    }
}

#[cfg(test)]
mod clip_geometry_tests {
    use super::*;
    use paged_parse::{ClippingPathSettings, ClippingType};

    fn corner(x: f32, y: f32) -> PathAnchor {
        PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        }
    }

    /// A square clip (CW) with a smaller square hole authored in the
    /// SAME winding gets the inner contour re-wound so NonZero keeps it
    /// as a hole — verifiable by the inner contour's anchors appearing
    /// in reversed order.
    #[test]
    fn build_clip_path_flips_inner_contour_for_hole() {
        // Outer square 0..100 (CW in screen space), inner square 40..60
        // authored CW too (so it would NOT be a hole without flipping).
        let clip = ClippingPathSettings {
            clipping_type: Some(ClippingType::UserModifiedPath),
            invert_path: false,
            include_inside_edges: true,
            applied_path_name: None,
            threshold: None,
            tolerance: None,
            clip_anchors: vec![
                corner(0.0, 0.0),
                corner(100.0, 0.0),
                corner(100.0, 100.0),
                corner(0.0, 100.0),
                // inner
                corner(40.0, 40.0),
                corner(60.0, 40.0),
                corner(60.0, 60.0),
                corner(40.0, 60.0),
            ],
            clip_subpath_starts: vec![0, 4],
            clip_subpath_open: vec![false, false],
        };
        let path = build_image_clip_path(&clip, 100.0, 100.0).expect("path built");
        // Two MoveTo contours survive.
        let move_tos: Vec<_> = path
            .segments
            .iter()
            .filter_map(|s| match s {
                PathSegment::MoveTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        assert_eq!(move_tos.len(), 2, "outer + hole contour");
        // The inner contour's first MoveTo is its first authored anchor
        // when NOT flipped, or its last anchor when flipped. Since both
        // squares are authored CW, the inner one is flipped, so its
        // MoveTo is the LAST authored inner anchor (40,60).
        assert_eq!(
            move_tos[1],
            (40.0, 60.0),
            "inner contour re-wound (reversed) so it punches a hole"
        );
    }

    /// Invert prepends a bounding-box contour and reverses the path so
    /// NonZero subtracts it — yielding two MoveTo contours where the
    /// FIRST is the bbox corner (0,0).
    #[test]
    fn build_clip_path_invert_prepends_bbox() {
        let clip = ClippingPathSettings {
            clipping_type: Some(ClippingType::UserModifiedPath),
            invert_path: true,
            include_inside_edges: false,
            applied_path_name: None,
            threshold: None,
            tolerance: None,
            clip_anchors: vec![
                corner(30.0, 30.0),
                corner(70.0, 30.0),
                corner(70.0, 70.0),
                corner(30.0, 70.0),
            ],
            clip_subpath_starts: Vec::new(),
            clip_subpath_open: Vec::new(),
        };
        let path = build_image_clip_path(&clip, 100.0, 100.0).expect("path built");
        let move_tos: Vec<_> = path
            .segments
            .iter()
            .filter_map(|s| match s {
                PathSegment::MoveTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        assert_eq!(move_tos.len(), 2, "bbox + punched rectangle");
        assert_eq!(move_tos[0], (0.0, 0.0), "bbox contour first");
    }

    /// No anchors ⇒ no path (the defer path handles the diagnostic).
    #[test]
    fn build_clip_path_none_without_anchors() {
        let clip = ClippingPathSettings {
            clipping_type: Some(ClippingType::PhotoshopPath),
            invert_path: false,
            include_inside_edges: false,
            applied_path_name: Some("Path 1".to_string()),
            threshold: None,
            tolerance: None,
            clip_anchors: Vec::new(),
            clip_subpath_starts: Vec::new(),
            clip_subpath_open: Vec::new(),
        };
        assert!(build_image_clip_path(&clip, 100.0, 100.0).is_none());
        assert!(clip.is_deferred_clip());
        assert!(!clip.has_renderable_geometry());
    }
}
