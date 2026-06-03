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
            Some(uri) => resolve_image_id(uri, options, &mut page.list, page_image_cache, decoded_cache),
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
            if rect.has_image_element
                && !rect.has_inline_pdf
                && options.missing_image_placeholder
            {
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
        emit_clipped_image(&mut page.list, clip_rect, outer, img_rect, composed, id);
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
            Some(uri) => resolve_image_id(uri, options, &mut page.list, page_image_cache, decoded_cache),
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
            if poly.has_image_element
                && !poly.has_inline_pdf
                && options.missing_image_placeholder
            {
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
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            img_rect,
            image_transform,
            id,
        );
    } else {
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
            Some(uri) => resolve_image_id(uri, options, &mut page.list, page_image_cache, decoded_cache),
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
            if oval.has_image_element
                && !oval.has_inline_pdf
                && options.missing_image_placeholder
            {
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
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            img_rect,
            image_transform,
            id,
        );
    } else {
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
    );
}

/// Push an arbitrary clip path, emit an image, then pop. Splits the
/// PushClip / Image / PopClip emission off `emit_clipped_image` so
/// the polygon-hosted image variant (used when the host is a curved
/// `<Polygon>` frame) can supply its own pre-interned path.
fn emit_image_under_clip(
    list: &mut paged_compose::DisplayList,
    clip_path_id: paged_compose::PathId,
    clip_transform: Transform,
    image_rect: paged_compose::Rect,
    image_transform: Transform,
    image_id: paged_compose::ImageId,
) {
    use paged_compose::DisplayCommand;
    list.push(DisplayCommand::PushClip {
        path_id: clip_path_id,
        transform: clip_transform,
    });
    let img_transform = Transform::for_rect_in(image_rect, image_transform);
    list.push(DisplayCommand::Image {
        image_id,
        transform: img_transform,
    });
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
                    tracing::warn!(
                        len = bytes.len(),
                        "inline image decode failed; skipping"
                    );
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
