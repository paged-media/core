// Final-pass recomposite: union the process CMYK planes + all spot
// planes into a single visible pixel, with `coverage == 0` falling
// back to the underlying Vello target. Mirrors the CPU rasterizer's
// final flush logic (`flush_cmyk_planes_into_rgb` +
// `compose_spot_overprint_via_plane`'s union path) so visual output
// is equivalent.
//
// Inputs:
//   * `plane_cmyk`: per-pixel packed CMYK (process planes, byte 0=C,
//     1=M, 2=Y, 3=K).
//   * `coverage`: per-pixel coverage (low byte). > 0 means "use plane
//     state at this pixel"; 0 means "pass the Vello target through".
//   * `spot_planes`: stacked packed spot planes (one u32 per pixel per
//     group of 4 spots). Total size = `num_spot_groups * num_pixels`.
//   * `spot_alts`: per-spot CMYK alternate (one u32 per spot, byte
//     order = C, M, Y, K). Indexed by `spot_id`, with valid range
//     `[0, num_spots)`.
//   * `vello_target`: the original Vello render, RGBA stored as little
//     endian RGBA8 packed into u32 (byte 0=R, 1=G, 2=B, 3=A).
//
// Output:
//   * `final_target`: an `rgba8unorm, write` storage texture that the
//     surrounding Rust code copies into the user's destination buffer
//     via `copy_texture_to_buffer`. Write-only is portable on every
//     adapter; the alternative (`read_write` on `rgba8unorm`) requires
//     `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` which not
//     every device exposes.
//
// Pixels with `coverage == 0` write the corresponding Vello pixel
// straight through — the "images and non-CMYK fills survive" invariant.
//
// `naive_cmyk_to_rgb_8bit` is ported verbatim from
// `crates/idml-gpu/src/cpu.rs` (search `naive_cmyk_to_rgb_8bit`). The
// integer math matches at 8-bit precision; the round-trip with the
// CPU's `rgb_to_naive_cmyk_8bit` is bit-stable so any pixel painted by
// a CMYK swatch and re-decoded by this shader returns its exact
// original RGB. Banding from the 8-bit round-trip on gradient
// overprint is the documented Vello-only artifact in the plan.

struct Params {
    width: u32,
    height: u32,
    num_spot_groups: u32,
    num_spots: u32,
};

@group(0) @binding(0) var<storage, read> plane_cmyk: array<u32>;
@group(0) @binding(1) var<storage, read> coverage: array<u32>;
@group(0) @binding(2) var<storage, read> spot_planes: array<u32>;
@group(0) @binding(3) var<storage, read> spot_alts: array<u32>;
@group(0) @binding(4) var<storage, read> vello_target: array<u32>;
@group(0) @binding(5) var<uniform> params: Params;
@group(0) @binding(6) var final_target: texture_storage_2d<rgba8unorm, write>;

fn unpack_byte(packed: u32, channel: u32) -> u32 {
    return (packed >> (channel * 8u)) & 0xFFu;
}

// (a * b + 127) / 255, clamped to 255. The `((255 - v) * (255 - K) + 127) / 255`
// pattern below uses it on the per-channel inner term.
fn scale_8(a: u32, b: u32) -> u32 {
    let prod = a * b + 127u;
    let result = prod / 255u;
    return min(result, 255u);
}

// Naive CMYK→RGB at 8 bits — mirror of `naive_cmyk_to_rgb_8bit` in
// cpu.rs. Inputs in [0..=255], output in [0..=255].
fn naive_cmyk_to_rgb_8bit(c: u32, m: u32, y: u32, k: u32) -> vec3<u32> {
    let kp = 255u - k;
    let r = scale_8(255u - c, kp);
    let g = scale_8(255u - m, kp);
    let b = scale_8(255u - y, kp);
    return vec3<u32>(r, g, b);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let idx = gid.y * params.width + gid.x;
    let coord = vec2<i32>(i32(gid.x), i32(gid.y));

    let cov = coverage[idx] & 0xFFu;
    if (cov == 0u) {
        // Pass-through: write the Vello target pixel unchanged.
        let v = vello_target[idx];
        let r = f32(v & 0xFFu) / 255.0;
        let g = f32((v >> 8u) & 0xFFu) / 255.0;
        let b = f32((v >> 16u) & 0xFFu) / 255.0;
        let a = f32((v >> 24u) & 0xFFu) / 255.0;
        textureStore(final_target, coord, vec4<f32>(r, g, b, a));
        return;
    }

    // Process CMYK at this pixel.
    let proc = plane_cmyk[idx];
    var acc_c = unpack_byte(proc, 0u);
    var acc_m = unpack_byte(proc, 1u);
    var acc_y = unpack_byte(proc, 2u);
    var acc_k = unpack_byte(proc, 3u);

    // Union every active spot ink's CMYK contribution. Each spot
    // carries a tint in its plane channel and a CMYK alternate in
    // `spot_alts[id]`. Contribution per channel is
    // `(alt_channel * tint + 127) / 255`; we max across all spots.
    let n_spots = params.num_spots;
    let n_pixels = params.width * params.height;
    for (var sid: u32 = 0u; sid < n_spots; sid = sid + 1u) {
        let group = sid / 4u;
        let channel = sid % 4u;
        let plane = spot_planes[group * n_pixels + idx];
        let tint = unpack_byte(plane, channel);
        if (tint == 0u) {
            continue;
        }
        let alt = spot_alts[sid];
        let alt_c = unpack_byte(alt, 0u);
        let alt_m = unpack_byte(alt, 1u);
        let alt_y = unpack_byte(alt, 2u);
        let alt_k = unpack_byte(alt, 3u);
        acc_c = max(acc_c, scale_8(alt_c, tint));
        acc_m = max(acc_m, scale_8(alt_m, tint));
        acc_y = max(acc_y, scale_8(alt_y, tint));
        acc_k = max(acc_k, scale_8(alt_k, tint));
    }

    let rgb = naive_cmyk_to_rgb_8bit(
        min(acc_c, 255u),
        min(acc_m, 255u),
        min(acc_y, 255u),
        min(acc_k, 255u),
    );
    // Final pixel alpha follows the Vello target's alpha at this
    // pixel — CMYK overprint draws are opaque on the framebuffer side
    // (the underlying RGB has been replaced by the CMYK composite,
    // but the alpha channel of the Vello target was set to 1.0 by
    // whatever fills painted there). For consistency with the CPU
    // rasterizer, preserve the Vello target's alpha as the output
    // alpha so a transparent page background stays transparent.
    let va = f32((vello_target[idx] >> 24u) & 0xFFu) / 255.0;
    let r_f = f32(rgb.x) / 255.0;
    let g_f = f32(rgb.y) / 255.0;
    let b_f = f32(rgb.z) / 255.0;
    textureStore(final_target, coord, vec4<f32>(r_f, g_f, b_f, va));
}
