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

//! Ink separations read out of the CPU rasterizer's plane state.
//!
//! The rasterizer already maintains, per page pixel, one 8-bit plane
//! per process ink (C/M/Y/K) plus one per named spot ink — that state
//! is what makes per-channel CMYK overprint work (`cpu.rs`, Stage
//! 4A/4B/4C). Those planes ARE a separation; until now they were
//! allocated, written, read by the overprint composite, and then
//! dropped at the end of `rasterize`. This module gives them a name, a
//! public shape, and the two prepress readings a print operator wants:
//! **total area coverage** and **plate isolation**.
//!
//! # What is measured and what is not — read this before trusting a number
//!
//! A pixel lands on a plate only when the draw that produced it carried
//! a [`Paint::Cmyk`](paged_compose::Paint::Cmyk). That is the case for
//! every CMYK and spot swatch in the document (the pipeline resolves
//! `Space="CMYK"` / `Model="Spot"` swatches to `Paint::Cmyk` and folds
//! the swatch tint into the channels). It is NOT the case for:
//!
//!  * RGB and Lab swatches — they resolve to `Paint::Solid`, which has
//!    no ink decomposition at all;
//!  * gradients — `Paint::LinearGradient` / `RadialGradient` /
//!    `SweepGradient` carry RGB stops;
//!  * placed images — the raster lane composites RGBA pixels, and the
//!    PDF exporter embeds them in their own colour space for the RIP to
//!    separate;
//!  * anything drawn inside a blend group / layer buffer, because the
//!    plane state is page-level only (see `cpu.rs`'s Stage B note).
//!
//! Those pixels are *unknown*, not *blank*. [`Separation::separated`]
//! marks which pixels carry plane truth, and every report here carries
//! [`InkCoverageReport::separated_pixels`] against
//! [`InkCoverageReport::total_pixels`] so a caller can never quote a
//! coverage figure without also knowing how much of the page it covers.
//!
//! Deliberately NOT done: estimating the unknown pixels by inverting
//! the rendered RGB. The renderer's own `rgb_to_naive_cmyk_8bit` is a
//! 100%-GCR map — it separates a rich black to K=100% / TAC 100%, where
//! a real coated profile yields ~300%. Folding that in would make the
//! headline TAC number confidently wrong in exactly the place operators
//! check it. An honest hole beats a plausible fabrication.

use crate::cpu::CmykPlanes;
use image::{Rgba, RgbaImage};
use paged_compose::DisplayList;

/// Which ink a plate carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InkChannel {
    Cyan,
    Magenta,
    Yellow,
    Black,
    /// A named ink, identified by its `paged_compose::SpotInkId`.
    Spot(u32),
}

impl InkChannel {
    pub fn is_spot(&self) -> bool {
        matches!(self, InkChannel::Spot(_))
    }
}

/// One ink plate: the per-pixel tint of a single ink across the page.
///
/// `tint[i]` is 0..=255 mapping to 0..=100% of this ink at pixel `i`
/// (row-major, `width * height` entries). `alternate` is the DeviceCMYK
/// this ink contributes at full strength — the unit basis vector for a
/// process ink, `SpotInk::cmyk_alternate` for a spot. Carrying it on
/// the plate lets [`Separation::plate_preview`] composite process and
/// spot plates through one uniform rule, the same one
/// `cpu::compose_spot_overprint_via_plane` uses for the live render.
#[derive(Debug, Clone)]
pub struct InkPlate {
    pub channel: InkChannel,
    /// For a process ink, `"Cyan"`/`"Magenta"`/`"Yellow"`/`"Black"`.
    /// For a spot, the display list's `SpotInk::name` — the IDML
    /// `<Color Self="…">` id (e.g. `"Color/Pantone286"`), NOT the
    /// human swatch name. The palette that maps id → display name
    /// lives above this crate; the caller with the palette in hand
    /// (`paged-canvas`) does that lookup.
    pub name: String,
    pub alternate: [u8; 4],
    pub tint: Vec<u8>,
}

impl InkPlate {
    /// True when no pixel on this plate carries ink — i.e. the plate
    /// would come off the press blank.
    pub fn is_blank(&self) -> bool {
        self.tint.iter().all(|&t| t == 0)
    }
}

/// A page's ink separation: one plate per ink, plus the mask saying
/// which pixels the separation actually knows about.
#[derive(Debug, Clone)]
pub struct Separation {
    pub width: u32,
    pub height: u32,
    /// Process plates first, in C, M, Y, K order, then one per spot
    /// ink in `SpotInkId` order. Always contains the four process
    /// plates even when blank — a job's plate list is a fixed frame of
    /// reference, and "Cyan: 0%" is itself a useful reading.
    pub plates: Vec<InkPlate>,
    /// Non-zero where the ink at this pixel came from plane state
    /// (a `Paint::Cmyk` draw). Zero where the pixel's colour has no
    /// ink decomposition in the raster lane — see the module docs.
    pub separated: Vec<u8>,
}

impl Separation {
    /// Build a separation from the rasterizer's plane state. `list`
    /// supplies the spot-ink table so plates can be named.
    ///
    /// `planes` is `None` when the page contained no CMYK draw at all;
    /// the result is then four blank process plates and an all-zero
    /// `separated` mask, which reads correctly as "nothing on this page
    /// is ink-separated".
    pub(crate) fn from_planes(
        planes: Option<&CmykPlanes>,
        list: &DisplayList,
        width: u32,
        height: u32,
    ) -> Self {
        let n = (width as usize) * (height as usize);
        const PROCESS: [(InkChannel, &str, [u8; 4]); 4] = [
            (InkChannel::Cyan, "Cyan", [255, 0, 0, 0]),
            (InkChannel::Magenta, "Magenta", [0, 255, 0, 0]),
            (InkChannel::Yellow, "Yellow", [0, 0, 255, 0]),
            (InkChannel::Black, "Black", [0, 0, 0, 255]),
        ];
        let mut plates = Vec::with_capacity(4);
        for (idx, (channel, name, alternate)) in PROCESS.into_iter().enumerate() {
            plates.push(InkPlate {
                channel,
                name: name.to_string(),
                alternate,
                tint: planes
                    .map(|p| p.process_plane(idx).to_vec())
                    .unwrap_or_else(|| vec![0u8; n]),
            });
        }
        let mut separated = planes
            .map(|p| p.coverage_mask().to_vec())
            .unwrap_or_else(|| vec![0u8; n]);
        if let Some(p) = planes {
            for (spot_idx, tint) in p.spot_planes().iter().enumerate() {
                let ink = list.spot_ink(paged_compose::SpotInkId(spot_idx as u32));
                plates.push(InkPlate {
                    channel: InkChannel::Spot(spot_idx as u32),
                    name: ink
                        .map(|i| i.name.clone())
                        .unwrap_or_else(|| format!("Spot {spot_idx}")),
                    alternate: p.spot_alternate(spot_idx),
                    tint: tint.clone(),
                });
                // A non-overprint spot draw writes its plane but not
                // the process coverage mask (`splat_spot_into_plane`
                // deliberately leaves `coverage` alone so a later
                // process overprint still sees virgin paper). For the
                // separation the mask means "we know this pixel's
                // ink", so spot-only pixels have to be folded in.
                for (i, &t) in tint.iter().enumerate() {
                    if t > 0 && separated[i] == 0 {
                        separated[i] = 255;
                    }
                }
            }
        }
        Self {
            width,
            height,
            plates,
            separated,
        }
    }

    fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// Total area coverage at one pixel, in percent: the sum of every
    /// plate's tint. Process-only content tops out at 400%; each spot
    /// plate adds up to another 100%, because a spot ink is a real
    /// extra pass on press — 100% PANTONE 286 over 100% K really is
    /// 200% TAC.
    fn tac_at(&self, idx: usize) -> f32 {
        let mut sum = 0.0f32;
        for plate in &self.plates {
            sum += plate.tint[idx] as f32 * (100.0 / 255.0);
        }
        sum
    }

    /// Ink-coverage report against a total-area-coverage `limit_pct`
    /// (the press's ink limit — 300% is the common SWOP sheet-fed
    /// figure, 240–280% for uncoated/newsprint).
    ///
    /// Statistics are taken over the *separated* pixels only, and the
    /// count of those is returned alongside so the caller can present
    /// the denominator. A page whose art is entirely RGB reports
    /// `separated_pixels == 0` and all-zero statistics — which is the
    /// truthful answer ("this page's ink is not knowable from the
    /// raster lane"), not a claim that it carries no ink.
    pub fn ink_coverage(&self, limit_pct: f32) -> InkCoverageReport {
        let n = self.pixel_count();
        let mut max_tac = 0.0f32;
        let mut sum_tac = 0.0f64;
        let mut over = 0u64;
        let mut separated_pixels = 0u64;
        let mut histogram = vec![0u32; TAC_BUCKETS];
        for idx in 0..n {
            if self.separated[idx] == 0 {
                continue;
            }
            separated_pixels += 1;
            let tac = self.tac_at(idx);
            if tac > max_tac {
                max_tac = tac;
            }
            sum_tac += tac as f64;
            if tac > limit_pct {
                over += 1;
            }
            let bucket = ((tac / TAC_BUCKET_PCT) as usize).min(TAC_BUCKETS - 1);
            histogram[bucket] += 1;
        }
        let plates = self
            .plates
            .iter()
            .map(|plate| {
                let mut inked = 0u64;
                let mut max_tint = 0u8;
                let mut sum_tint = 0u64;
                for (idx, &t) in plate.tint.iter().enumerate() {
                    if self.separated[idx] == 0 || t == 0 {
                        continue;
                    }
                    inked += 1;
                    sum_tint += t as u64;
                    if t > max_tint {
                        max_tint = t;
                    }
                }
                PlateCoverage {
                    name: plate.name.clone(),
                    is_spot: plate.channel.is_spot(),
                    inked_pixels: inked,
                    area_pct: if n == 0 {
                        0.0
                    } else {
                        inked as f32 * 100.0 / n as f32
                    },
                    max_tint_pct: max_tint as f32 * (100.0 / 255.0),
                    mean_tint_pct: if inked == 0 {
                        0.0
                    } else {
                        (sum_tint as f64 / inked as f64) as f32 * (100.0 / 255.0)
                    },
                }
            })
            .collect();
        InkCoverageReport {
            width: self.width,
            height: self.height,
            limit_pct,
            total_pixels: n as u64,
            separated_pixels,
            max_tac_pct: max_tac,
            mean_tac_pct: if separated_pixels == 0 {
                0.0
            } else {
                (sum_tac / separated_pixels as f64) as f32
            },
            over_limit_pixels: over,
            plates,
            histogram,
        }
    }

    /// Separations preview: composite only the plates at the given
    /// indices, exactly the way the renderer composites overlapping
    /// inks (per-channel max of each ink's DeviceCMYK contribution,
    /// then the naive CMYK→RGB map that `cpu.rs` uses as its
    /// definition of "what these inks look like together").
    ///
    /// Unknown pixels — those outside [`Separation::separated`] —
    /// come back **fully transparent**, never as paper white and never
    /// as the original artwork. Transparent is the only encoding that
    /// cannot be misread: white would claim the plate is empty there,
    /// and showing the art would claim it is on the plate. The caller
    /// composites over whatever backdrop makes the hole visible.
    pub fn plate_preview(&self, visible: &[usize]) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(self.width, self.height, Rgba([0, 0, 0, 0]));
        let raw = img.as_mut();
        for idx in 0..self.pixel_count() {
            if self.separated[idx] == 0 {
                continue;
            }
            let (mut c, mut m, mut y, mut k) = (0u16, 0u16, 0u16, 0u16);
            for &p in visible {
                let Some(plate) = self.plates.get(p) else {
                    continue;
                };
                let tint = plate.tint[idx] as u16;
                if tint == 0 {
                    continue;
                }
                let contrib = |alt: u8| -> u16 { (alt as u16 * tint + 127) / 255 };
                c = c.max(contrib(plate.alternate[0]));
                m = m.max(contrib(plate.alternate[1]));
                y = y.max(contrib(plate.alternate[2]));
                k = k.max(contrib(plate.alternate[3]));
            }
            let (r, g, b) = crate::cpu::naive_cmyk_to_rgb_8bit(
                c.min(255) as u8,
                m.min(255) as u8,
                y.min(255) as u8,
                k.min(255) as u8,
            );
            let o = idx * 4;
            raw[o] = r;
            raw[o + 1] = g;
            raw[o + 2] = b;
            raw[o + 3] = 255;
        }
        img
    }

    /// Ink-limit overlay: opaque `flag` wherever separated ink exceeds
    /// `limit_pct`, transparent everywhere else (including over the
    /// unknown pixels — an overlay must not imply a verdict where
    /// there is no measurement).
    pub fn ink_limit_overlay(&self, limit_pct: f32, flag: [u8; 4]) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(self.width, self.height, Rgba([0, 0, 0, 0]));
        let raw = img.as_mut();
        for idx in 0..self.pixel_count() {
            if self.separated[idx] == 0 {
                continue;
            }
            if self.tac_at(idx) > limit_pct {
                let o = idx * 4;
                raw[o..o + 4].copy_from_slice(&flag);
            }
        }
        img
    }
}

/// TAC histogram bucket width, in percent.
pub const TAC_BUCKET_PCT: f32 = 10.0;
/// Histogram bucket count: 0..=800% in [`TAC_BUCKET_PCT`] steps. 400%
/// is the process ceiling; the extra range covers spot inks, which add
/// a further 100% per plate. Anything past the top clamps into the
/// last bucket.
pub const TAC_BUCKETS: usize = 81;

/// Per-plate coverage figures. `area_pct` is the share of the WHOLE
/// page this plate puts any ink on (the "does this plate earn its
/// press pass" number); `max_tint_pct` / `mean_tint_pct` describe how
/// heavily, over the inked pixels only.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateCoverage {
    pub name: String,
    pub is_spot: bool,
    pub inked_pixels: u64,
    pub area_pct: f32,
    pub max_tint_pct: f32,
    pub mean_tint_pct: f32,
}

/// A page's ink-coverage reading.
///
/// **Always present `separated_pixels` / `total_pixels` next to the
/// TAC figures.** Every statistic here is taken over the separated
/// pixels; quoting `max_tac_pct` without the coverage denominator
/// invites an operator to trust a number that may describe 3% of the
/// page. See the module docs for what falls outside.
#[derive(Debug, Clone, PartialEq)]
pub struct InkCoverageReport {
    pub width: u32,
    pub height: u32,
    /// The limit `over_limit_pixels` was counted against.
    pub limit_pct: f32,
    pub total_pixels: u64,
    pub separated_pixels: u64,
    pub max_tac_pct: f32,
    pub mean_tac_pct: f32,
    pub over_limit_pixels: u64,
    pub plates: Vec<PlateCoverage>,
    /// [`TAC_BUCKETS`] counts over the separated pixels, bucket `i`
    /// covering `[i·10%, (i+1)·10%)`. Lets a caller re-threshold the
    /// ink limit without re-rendering the page.
    pub histogram: Vec<u32>,
}

impl InkCoverageReport {
    /// Share of the page the separation actually measured, 0..=100.
    /// The honesty denominator — a report over 4% of the page is not
    /// a page-level verdict.
    pub fn separated_pct(&self) -> f32 {
        if self.total_pixels == 0 {
            0.0
        } else {
            self.separated_pixels as f32 * 100.0 / self.total_pixels as f32
        }
    }

    /// Share of the SEPARATED pixels that exceed the limit, 0..=100.
    pub fn over_limit_pct_of_separated(&self) -> f32 {
        if self.separated_pixels == 0 {
            0.0
        } else {
            self.over_limit_pixels as f32 * 100.0 / self.separated_pixels as f32
        }
    }

    /// Plates that would actually go on press (any ink anywhere).
    pub fn active_plates(&self) -> impl Iterator<Item = &PlateCoverage> {
        self.plates.iter().filter(|p| p.inked_pixels > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_compose::{emit_rect, Color, DisplayCommand, Paint, Rect, SpotInk};

    fn rect_list(w: f32, h: f32, paint: Paint) -> DisplayList {
        let mut list = DisplayList::new();
        emit_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            paint,
            &mut list,
        );
        list
    }

    fn cmyk(c: f32, m: f32, y: f32, k: f32) -> Paint {
        Paint::Cmyk {
            c,
            m,
            y,
            k,
            rgb: Color::rgba(0.1, 0.1, 0.1, 1.0),
            spot: None,
        }
    }

    fn separate(list: &DisplayList, w: f32, h: f32) -> Separation {
        let opts = crate::RasterOptions::new(w, h);
        crate::cpu::rasterize_with_separation(list, &opts).1
    }

    #[test]
    fn rich_black_rectangle_reports_its_own_tac() {
        // 60/40/40/100 — the classic rich black, 240% TAC.
        let list = rect_list(20.0, 20.0, cmyk(0.6, 0.4, 0.4, 1.0));
        let sep = separate(&list, 20.0, 20.0);
        let report = sep.ink_coverage(300.0);
        assert!(
            (report.max_tac_pct - 240.0).abs() < 1.5,
            "max TAC = {}",
            report.max_tac_pct
        );
        assert_eq!(report.over_limit_pixels, 0, "240% is under a 300% limit");
        // The same page against a newsprint limit trips.
        let tight = sep.ink_coverage(200.0);
        assert!(tight.over_limit_pixels > 0, "240% must exceed a 200% limit");
        // Four process plates, all four inked.
        assert_eq!(report.plates.len(), 4);
        assert_eq!(report.active_plates().count(), 4);
    }

    #[test]
    fn an_rgb_page_reports_zero_separated_not_zero_ink() {
        // A Paint::Solid page has no ink decomposition. The report must
        // say "nothing measured", never "no ink".
        let list = rect_list(10.0, 10.0, Paint::Solid(Color::rgba(0.0, 0.0, 0.0, 1.0)));
        let sep = separate(&list, 10.0, 10.0);
        let report = sep.ink_coverage(300.0);
        assert_eq!(report.separated_pixels, 0);
        assert_eq!(report.separated_pct(), 0.0);
        assert_eq!(report.max_tac_pct, 0.0);
        assert_eq!(report.active_plates().count(), 0);
        assert!(report.total_pixels > 0, "the page still has pixels");
    }

    #[test]
    fn partial_coverage_is_reported_against_the_page() {
        // Fill a quarter of the page with CMYK; the rest stays paper.
        // separated_pct must reflect the quarter, so a caller cannot
        // read max_tac_pct as a whole-page verdict.
        let list = rect_list(10.0, 10.0, cmyk(1.0, 0.0, 0.0, 0.0));
        let sep = separate(&list, 20.0, 20.0);
        let report = sep.ink_coverage(300.0);
        let pct = report.separated_pct();
        assert!(
            (20.0..=30.0).contains(&pct),
            "quarter-page fill separated_pct = {pct}"
        );
    }

    #[test]
    fn a_spot_plate_is_its_own_press_pass() {
        // 100% spot over 100% K is 200% TAC — the spot is an extra
        // pass, not a re-mix of the process inks.
        let mut list = DisplayList::new();
        let spot_id = list.push_spot_ink(SpotInk {
            name: "Color/Pantone286".into(),
            cmyk_alternate: [255, 191, 0, 0],
        });
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        emit_rect(rect, cmyk(0.0, 0.0, 0.0, 1.0), &mut list);
        emit_rect(
            rect,
            Paint::Cmyk {
                c: 1.0,
                m: 0.75,
                y: 0.0,
                k: 0.0,
                rgb: Color::rgba(0.0, 0.2, 0.6, 1.0),
                spot: Some(spot_id),
            },
            &mut list,
        );
        // emit_rect always produces FillPath; upgrade the spot draw to
        // an overprint so it lands on its own plate over the black.
        let last = list.commands.len() - 1;
        if let DisplayCommand::FillPath {
            paint,
            path_id,
            transform,
        } = list.commands[last]
        {
            list.commands[last] = DisplayCommand::FillPathOverprint {
                paint,
                path_id,
                transform,
            };
        }
        let sep = separate(&list, 10.0, 10.0);
        assert_eq!(sep.plates.len(), 5, "4 process + 1 spot");
        let spot = sep
            .plates
            .iter()
            .find(|p| p.channel.is_spot())
            .expect("spot plate");
        assert_eq!(spot.name, "Color/Pantone286");
        let report = sep.ink_coverage(300.0);
        assert!(
            report.max_tac_pct > 190.0,
            "spot-over-K TAC = {} (expected ~200%)",
            report.max_tac_pct
        );
        assert!(report.over_limit_pixels == 0, "200% is under 300%");
    }

    #[test]
    fn plate_preview_leaves_unmeasured_pixels_transparent() {
        // A CMYK square on a larger page: inside is opaque, outside is
        // transparent — never white, which would claim "no ink here".
        let list = rect_list(10.0, 10.0, cmyk(1.0, 0.0, 0.0, 0.0));
        let sep = separate(&list, 20.0, 20.0);
        let cyan = sep
            .plates
            .iter()
            .position(|p| p.channel == InkChannel::Cyan)
            .unwrap();
        let img = sep.plate_preview(&[cyan]);
        assert_eq!(img.get_pixel(19, 19).0[3], 0, "unmeasured pixel is a hole");
        assert_eq!(img.get_pixel(2, 2).0[3], 255, "measured pixel is opaque");
        // Isolating cyan shows cyan, not the composite.
        let px = img.get_pixel(2, 2).0;
        assert!(
            px[0] < 80 && px[1] > 150 && px[2] > 150,
            "cyan plate: {px:?}"
        );
        // Isolating black instead shows paper (no black ink present).
        let black = sep
            .plates
            .iter()
            .position(|p| p.channel == InkChannel::Black)
            .unwrap();
        let k_img = sep.plate_preview(&[black]);
        let k_px = k_img.get_pixel(2, 2).0;
        assert_eq!([k_px[0], k_px[1], k_px[2]], [255, 255, 255], "blank plate");
    }

    #[test]
    fn ink_limit_overlay_flags_only_measured_violations() {
        let list = rect_list(10.0, 10.0, cmyk(1.0, 1.0, 1.0, 1.0));
        let sep = separate(&list, 20.0, 20.0);
        let overlay = sep.ink_limit_overlay(300.0, [255, 0, 0, 255]);
        assert_eq!(overlay.get_pixel(2, 2).0, [255, 0, 0, 255], "400% > 300%");
        assert_eq!(
            overlay.get_pixel(19, 19).0[3],
            0,
            "no verdict where nothing was measured"
        );
    }

    /// The whole separation feature rests on `rasterize` and
    /// `rasterize_with_separation` sharing one body, with the plane
    /// readout happening strictly after the last framebuffer write.
    /// If that ever stops holding, asking for a separation would
    /// silently change what the page LOOKS like — a fidelity event
    /// smuggled in behind a prepress feature. Pin it byte-for-byte,
    /// including on a page whose overprint composites write RGB
    /// through the naive map.
    #[test]
    fn asking_for_a_separation_does_not_change_a_single_pixel() {
        let mut list = DisplayList::new();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 30.0,
        };
        emit_rect(rect, cmyk(0.0, 1.0, 0.0, 0.0), &mut list);
        emit_rect(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 30.0,
                h: 30.0,
            },
            cmyk(1.0, 0.0, 0.0, 0.0),
            &mut list,
        );
        let last = list.commands.len() - 1;
        if let DisplayCommand::FillPath {
            paint,
            path_id,
            transform,
        } = list.commands[last]
        {
            list.commands[last] = DisplayCommand::FillPathOverprint {
                paint,
                path_id,
                transform,
            };
        }
        emit_rect(
            Rect {
                x: 20.0,
                y: 0.0,
                w: 20.0,
                h: 20.0,
            },
            Paint::Solid(Color::rgba(0.2, 0.4, 0.8, 1.0)),
            &mut list,
        );
        let opts = crate::RasterOptions::new(50.0, 50.0);
        let plain = crate::cpu::rasterize(&list, &opts);
        let (with_sep, _) = crate::cpu::rasterize_with_separation(&list, &opts);
        assert_eq!(plain.as_raw(), with_sep.as_raw());
    }

    #[test]
    fn histogram_re_thresholds_without_a_re_render() {
        let list = rect_list(20.0, 20.0, cmyk(0.6, 0.4, 0.4, 1.0));
        let sep = separate(&list, 20.0, 20.0);
        let report = sep.ink_coverage(300.0);
        assert_eq!(report.histogram.len(), TAC_BUCKETS);
        let counted: u32 = report.histogram.iter().sum();
        assert_eq!(counted as u64, report.separated_pixels);
        // 240% lands in the [240, 250) bucket.
        assert!(report.histogram[24] > 0, "240% TAC bucket is populated");
    }
}
