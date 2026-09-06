// Copyright the paged-media authors.
// SPDX-License-Identifier: MPL-2.0 OR LicenseRef-PMEL
#![cfg(all(
    feature = "vello-backend",
    feature = "cpu",
    not(target_arch = "wasm32")
))]

//! Vello-lane effect parity. The CPU rasterizer is the path of record
//! for object effects; these tests pin the GPU lane against it on the
//! properties that matter visually, so a Vello approximation cannot
//! silently paint ink where InDesign takes ink away.

use paged_compose::{
    Color, DisplayCommand as Cmd, DisplayList, Feather, FeatherCornerType, Paint, PathData,
    PathSegment, Transform as XF,
};
use paged_gpu::{cpu::CpuRasterizer, vello_rs::VelloRasterizer, PathRasterizer, RasterOptions};

fn unit_rect(list: &mut DisplayList) -> paged_compose::PathId {
    let mut p = PathData::default();
    p.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
    p.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
    p.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
    p.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
    p.segments.push(PathSegment::Close);
    list.paths.push_anon(p)
}

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[test]
fn a_feather_takes_alpha_away_it_does_not_add_white_ink() {
    // The Vello lane used to approximate a feather by stamping WHITE
    // rings inside the path. On a tinted ground that paints a white
    // halo where InDesign shows the ground; on the annual's page 59 it
    // erased the peach panel under the cloud and the veil.
    let mut list = DisplayList::new();
    let rect = unit_rect(&mut list);
    let ground = XF([60.0, 0.0, 0.0, 30.0, 0.0, 0.0]);
    let obj = XF([24.0, 0.0, 0.0, 18.0, 6.0, 6.0]);
    list.commands.push(Cmd::FillPath {
        path_id: rect,
        paint: Paint::Solid(Color::rgba(0.5, 0.5, 0.5, 1.0)),
        transform: ground,
    });
    list.commands.push(Cmd::FillPath {
        path_id: rect,
        paint: Paint::Solid(Color::rgba(0.05, 0.05, 0.05, 1.0)),
        transform: obj,
    });
    list.commands.push(Cmd::Feather {
        path_id: rect,
        transform: obj,
        params: Feather {
            width: 6.0,
            corner_type: FeatherCornerType::Sharp,
            noise: 0.0,
            choke: 0.0,
        },
    });
    let mut opts = RasterOptions::new(60.0, 30.0);
    opts.dpi = 72.0;
    let cpu = CpuRasterizer.rasterize(&list, &opts);
    let vello = VelloRasterizer::new().rasterize(&list, &opts);
    if vello.iter().all(|b| *b == 0) {
        eprintln!("no GPU available; skipping");
        return;
    }
    let (w, _h) = opts.pixel_size();

    // Away from the object both lanes must show the untouched ground.
    for (x, y) in [(50u32, 25u32), (3, 3)] {
        let c = px(&cpu, w, x, y);
        let v = px(&vello, w, x, y);
        assert!(
            (c[0] as i32 - v[0] as i32).abs() <= 6,
            "ground at ({x},{y}) differs: cpu={c:?} vello={v:?}"
        );
    }
    // Inside the fading edge the object gives way to the GROUND, so
    // the pixel must never be LIGHTER than the ground.
    let ground_level = px(&cpu, w, 50, 25)[0] as i32;
    for x in 6..12u32 {
        let v = px(&vello, w, x, 15)[0] as i32;
        assert!(
            v <= ground_level + 8,
            "the feather lightens past the ground at x={x}: got {v}, ground {ground_level}"
        );
    }
}
