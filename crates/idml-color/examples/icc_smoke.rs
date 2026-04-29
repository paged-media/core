//! Quick CMYK → RGB sanity check for ICC profiles. The official
//! per-conversion math is in `IccTransform`; this example also tries
//! a direct lcms2 path so anomalies between the two can be diagnosed
//! at a glance.
use idml_color::{Cmyk, IccTransform, LinearRgb};
use lcms2::{CIExyY, CIExyYTRIPLE, Flags, Intent, PixelFormat, Profile, ToneCurve, Transform};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: icc_smoke <profile.icc>");
    let bytes = std::fs::read(&path).unwrap();
    let xform = IccTransform::cmyk_to_linear_rgb(&bytes).unwrap();
    for (label, c, m, y, k) in [
        ("100/0/0/0 cyan", 100.0, 0.0, 0.0, 0.0),
        ("50/0/0/0 mid-cyan", 50.0, 0.0, 0.0, 0.0),
        ("0/0/0/100 black", 0.0, 0.0, 0.0, 100.0),
        ("0/0/0/0 paper", 0.0, 0.0, 0.0, 0.0),
    ] {
        let LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(Cmyk { c, m, y, k });
        println!("via_idml  {label}: lin-RGB {r:.3} {g:.3} {b:.3}");
    }
    let src = Profile::new_icc(&bytes).unwrap();
    let _primaries = CIExyYTRIPLE {
        Red: CIExyY {
            x: 0.6400,
            y: 0.3300,
            Y: 1.0,
        },
        Green: CIExyY {
            x: 0.3000,
            y: 0.6000,
            Y: 1.0,
        },
        Blue: CIExyY {
            x: 0.1500,
            y: 0.0600,
            Y: 1.0,
        },
    };
    let _white = CIExyY {
        x: 0.3127,
        y: 0.3290,
        Y: 1.0,
    };
    let _srgb_curve = ToneCurve::new(2.4);
    // Use lcms's built-in sRGB profile as the destination — known good.
    let dst = Profile::new_srgb();
    let t: Transform<[f32; 4], [f32; 3]> = Transform::new_flags(
        &src,
        PixelFormat::CMYK_FLT,
        &dst,
        PixelFormat::RGB_FLT,
        Intent::RelativeColorimetric,
        Flags::BLACKPOINT_COMPENSATION,
    )
    .unwrap();
    for (label, c, m, y, k) in [
        ("100/0/0/0 cyan", 1.0, 0.0, 0.0, 0.0),
        ("0/0/0/100 black", 0.0, 0.0, 0.0, 1.0),
        ("0/0/0/0 paper", 0.0, 0.0, 0.0, 0.0),
    ] {
        let mut out = [[0.0f32; 3]];
        t.transform_pixels(&[[c, m, y, k]], &mut out);
        println!(
            "direct    {label}: gamma-RGB {:.3} {:.3} {:.3}",
            out[0][0], out[0][1], out[0][2]
        );
    }
    // Try percentages on the [0..100] scale.
    for (label, c, m, y, k) in [
        ("100/0/0/0 cyan", 100.0, 0.0, 0.0, 0.0),
        ("0/0/0/100 black", 0.0, 0.0, 0.0, 100.0),
        ("0/0/0/0 paper", 0.0, 0.0, 0.0, 0.0),
    ] {
        let mut out = [[0.0f32; 3]];
        t.transform_pixels(&[[c, m, y, k]], &mut out);
        println!(
            "pct-100   {label}: gamma-RGB {:.3} {:.3} {:.3}",
            out[0][0], out[0][1], out[0][2]
        );
    }
    // Try the reversed convention (1 = no ink, 0 = full ink). This is
    // what some older lcms ICC profiles document.
    for (label, c, m, y, k) in [
        ("100/0/0/0 cyan", 0.0, 1.0, 1.0, 1.0),
        ("0/0/0/100 black", 1.0, 1.0, 1.0, 0.0),
        ("0/0/0/0 paper", 1.0, 1.0, 1.0, 1.0),
    ] {
        let mut out = [[0.0f32; 3]];
        t.transform_pixels(&[[c, m, y, k]], &mut out);
        println!(
            "reversed  {label}: gamma-RGB {:.3} {:.3} {:.3}",
            out[0][0], out[0][1], out[0][2]
        );
    }
}
