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

//! Frame fitting: where a placed picture lands inside its frame.
//!
//! InDesign's fitting commands (Object ▸ Fitting) and the
//! `<FrameFittingOption FittingOnEmptyFrame="…">` a frame applies when
//! content is placed into it resolve to ONE thing: the inner `<Image>`'s
//! `ItemTransform`, which maps the picture's pixel box (`0 0 w h`, one
//! pixel a point at 72 ppi) into the frame's inner space. The frame
//! clips whatever overflows. This module computes that transform, so the
//! renderer (drawing a frame that names a fit but carries no transform
//! yet) and the authoring doors (baking the fit on place, as InDesign
//! does) agree on the same numbers.
//!
//! The fits, for a frame box `W × H` and a picture `w × h`:
//!
//! * **Fill Frame Proportionally** (`FillProportionally`) — ONE scale,
//!   `max(W/w, H/h)`: the picture covers the frame, the overflow axis is
//!   clipped. Never a stretch.
//! * **Fit Content Proportionally** (`Proportionally`) — one scale,
//!   `min(W/w, H/h)`: the whole picture shows, letterboxed.
//! * **Fit Content to Frame** (`ContentToFrame`) — `W/w` and `H/h`
//!   independently: the only fit that distorts.
//! * **Center Content** (`CenterContent`) — the picture's own size,
//!   unscaled.
//!
//! The proportional fits and Center Content align the scaled picture to
//! the frame's fitting reference point (`FittingAlignment`; the default
//! is the centre), so the overflow is shared equally by default.

use crate::Bounds;

/// A resolved fitting mode. Parsed from the IDML / scripting vocabulary
/// with [`FitMode::from_idml`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitMode {
    /// Uniform scale, cover the frame (overflow clipped).
    FillProportionally,
    /// Uniform scale, contain the picture (letterbox).
    Proportionally,
    /// Independent x/y scale: stretch to the frame.
    ContentToFrame,
    /// Native size, aligned to the reference point.
    CenterContent,
}

impl FitMode {
    /// Every spelling the engine meets for a fit that places content:
    /// the IDML `FittingOnEmptyFrame` / `FitOptions` values plus the
    /// menu-command names scripts and the CLI pass (`FitContentToFrame`,
    /// `FitContentProportionally`, `FillFrameProportionally`).
    /// `ContentAwareFit` resolves to a fill — the same uniform cover,
    /// without Adobe's subject detection choosing the offset. `None`,
    /// `FrameToContent` (resizes the frame, not the content) and unknown
    /// strings return `None`: no fit to apply.
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "FillProportionally" | "FillFrameProportionally" | "ContentAwareFit" => {
                Some(FitMode::FillProportionally)
            }
            "Proportionally" | "FitContentProportionally" | "ContentProportionally" => {
                Some(FitMode::Proportionally)
            }
            "ContentToFrame" | "FitContentToFrame" => Some(FitMode::ContentToFrame),
            "CenterContent" => Some(FitMode::CenterContent),
            _ => None,
        }
    }
}

/// The `(x, y)` fraction of the frame's free space a fitting reference
/// point (`FittingAlignment`) pins the content to: `0` left/top, `0.5`
/// centre, `1` right/bottom. `None` and unknown values are the centre.
pub fn reference_point_fraction(point: Option<&str>) -> (f32, f32) {
    match point.unwrap_or("CenterPoint") {
        "TopLeftPoint" => (0.0, 0.0),
        "TopCenterPoint" => (0.5, 0.0),
        "TopRightPoint" => (1.0, 0.0),
        "LeftCenterPoint" => (0.0, 0.5),
        "RightCenterPoint" => (1.0, 0.5),
        "BottomLeftPoint" => (0.0, 1.0),
        "BottomCenterPoint" => (0.5, 1.0),
        "BottomRightPoint" => (1.0, 1.0),
        _ => (0.5, 0.5),
    }
}

/// The inner image `ItemTransform` (`[a, b, c, d, tx, ty]`, pixel box →
/// frame-inner space) that fits a `img_w × img_h` picture into `frame`
/// with `mode`, aligned to `reference_point`. `None` for a degenerate
/// picture or frame.
pub fn fit_transform(
    mode: FitMode,
    reference_point: Option<&str>,
    frame: Bounds,
    img_w: f32,
    img_h: f32,
) -> Option<[f32; 6]> {
    let (fw, fh) = (frame.width(), frame.height());
    if !(img_w > 0.0 && img_h > 0.0 && fw > 0.0 && fh > 0.0) {
        return None;
    }
    let (rx, ry) = (fw / img_w, fh / img_h);
    let (sx, sy) = match mode {
        FitMode::FillProportionally => {
            let s = rx.max(ry);
            (s, s)
        }
        FitMode::Proportionally => {
            let s = rx.min(ry);
            (s, s)
        }
        FitMode::ContentToFrame => (rx, ry),
        FitMode::CenterContent => (1.0, 1.0),
    };
    let (ax, ay) = reference_point_fraction(reference_point);
    let tx = frame.left + ax * (fw - sx * img_w);
    let ty = frame.top + ay * (fh - sy * img_h);
    Some([sx, 0.0, 0.0, sy, tx, ty])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported case: a portrait 918×1224 picture in a landscape
    /// ~249×187 pt frame.
    const FRAME: Bounds = Bounds {
        top: 10.0,
        left: 20.0,
        bottom: 10.0 + 186.9,
        right: 20.0 + 249.3,
    };
    const W: f32 = 918.0;
    const H: f32 = 1224.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn fill_proportionally_is_uniform_and_covers_the_frame() {
        let t = fit_transform(FitMode::FillProportionally, None, FRAME, W, H).unwrap();
        assert_eq!(t[0], t[3], "one scale on both axes, never a stretch");
        assert_eq!((t[1], t[2]), (0.0, 0.0));
        let s = (249.3f32 / W).max(186.9 / H);
        assert!(close(t[0], s));
        // Width fits exactly, height overflows and is centred.
        assert!(close(t[0] * W, 249.3));
        assert!(t[3] * H > 186.9);
        assert!(close(t[4], 20.0));
        let overflow = t[3] * H - 186.9;
        assert!(close(t[5], 10.0 - overflow / 2.0));
    }

    #[test]
    fn fit_content_proportionally_is_uniform_and_contained() {
        let t = fit_transform(FitMode::Proportionally, None, FRAME, W, H).unwrap();
        assert_eq!(t[0], t[3]);
        assert!(close(t[0], (249.3f32 / W).min(186.9 / H)));
        // Height fits exactly, width letterboxed and centred.
        assert!(close(t[3] * H, 186.9));
        assert!(t[0] * W < 249.3);
        assert!(close(t[5], 10.0));
        assert!(close(t[4], 20.0 + (249.3 - t[0] * W) / 2.0));
    }

    #[test]
    fn fit_content_to_frame_stretches() {
        let t = fit_transform(FitMode::ContentToFrame, None, FRAME, W, H).unwrap();
        assert!(close(t[0] * W, 249.3));
        assert!(close(t[3] * H, 186.9));
        assert!(close(t[4], 20.0) && close(t[5], 10.0));
    }

    #[test]
    fn center_content_keeps_native_size_centred() {
        let t = fit_transform(FitMode::CenterContent, None, FRAME, 100.0, 50.0).unwrap();
        assert_eq!((t[0], t[3]), (1.0, 1.0));
        assert!(close(t[4], 20.0 + (249.3 - 100.0) / 2.0));
        assert!(close(t[5], 10.0 + (186.9 - 50.0) / 2.0));
    }

    #[test]
    fn the_reference_point_pins_the_overflow() {
        let tl = fit_transform(
            FitMode::FillProportionally,
            Some("TopLeftPoint"),
            FRAME,
            W,
            H,
        )
        .unwrap();
        assert!(close(tl[4], 20.0) && close(tl[5], 10.0));
        let br = fit_transform(
            FitMode::FillProportionally,
            Some("BottomRightPoint"),
            FRAME,
            W,
            H,
        )
        .unwrap();
        assert!(close(br[5] + br[3] * H, 10.0 + 186.9));
    }

    #[test]
    fn the_vocabulary_resolves() {
        assert_eq!(
            FitMode::from_idml("FillProportionally"),
            Some(FitMode::FillProportionally)
        );
        assert_eq!(
            FitMode::from_idml("ContentAwareFit"),
            Some(FitMode::FillProportionally)
        );
        assert_eq!(
            FitMode::from_idml("FitContentProportionally"),
            Some(FitMode::Proportionally)
        );
        assert_eq!(
            FitMode::from_idml("Proportionally"),
            Some(FitMode::Proportionally)
        );
        assert_eq!(
            FitMode::from_idml("FitContentToFrame"),
            Some(FitMode::ContentToFrame)
        );
        assert_eq!(
            FitMode::from_idml("ContentToFrame"),
            Some(FitMode::ContentToFrame)
        );
        assert_eq!(
            FitMode::from_idml("CenterContent"),
            Some(FitMode::CenterContent)
        );
        assert_eq!(FitMode::from_idml("None"), None);
        assert_eq!(FitMode::from_idml("FrameToContent"), None);
    }

    #[test]
    fn degenerate_inputs_have_no_fit() {
        assert!(fit_transform(FitMode::FillProportionally, None, FRAME, 0.0, 10.0).is_none());
        assert!(
            fit_transform(FitMode::FillProportionally, None, Bounds::ZERO, 10.0, 10.0).is_none()
        );
    }
}
