// CMYK plane splat / overprint compute shader.
//
// Driven from the Vello backend's overprint composite path. The scene
// is segmented at every `*Overprint` command; consecutive overprints
// with identical ink masks are coalesced into one Vello scene rendered
// to a scratch RGBA texture, then copied to `scratch_rgba` as a storage
// buffer (one packed u32 per pixel, byte order = R, G, B, A). This
// shader walks the buffer and updates plane state:
//
//   * If `coverage[p] == 0` (paper, no prior CMYK draw at this pixel)
//     and `alpha > 0`: virgin-paper splat — write `ink_mask * alpha`
//     into the four process planes (or the selected spot channel) and
//     set `coverage[p] = alpha`.
//   * If `coverage[p] > 0` and `alpha > 0`: per-channel
//     `max(plane, ink_mask * alpha)` — adding more ink can only darken
//     a channel, never lighten it. Same for spot dispatches.
//   * Otherwise (alpha == 0): no update.
//
// Both branches update `coverage` (with `max(coverage, alpha)`) so the
// final `recomposite` pass knows the pixel was touched and the
// `coverage == 0` passthrough preserves untouched RGB / images.
//
// Why buffers rather than `r8unorm` storage textures: WebGPU forbids
// `r8unorm` storage entirely, and `rgba8unorm` read_write storage
// access is gated on `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`
// which is non-portable. Storage buffers with `read_write` work on
// every adapter — and the per-pixel u32 packing is a wash on memory
// (one byte per channel either way).
//
// Integer arithmetic vs. CPU: the CPU rasterizer's
// `compose_cmyk_overprint_via_planes` keeps everything in u16 with
// explicit `(prod + 127) / 255` rounding. We mirror that exactly using
// integer math on u32 lanes — converting `ink_mask` (f32 in 0..1) to
// 8-bit on the Rust side before pushing the uniform, and using
// `(a*b + 127) / 255` for the alpha-weighting. This makes the GPU
// result bit-stable with the CPU finisher on identical inputs.

struct Params {
    // Per-channel ink amount, 0..255 packed into 4 bytes of the same
    // u32 (byte 0=C, byte 1=M, byte 2=Y, byte 3=K). Pushed pre-scaled
    // on the Rust side so the shader can do pure integer math.
    ink_mask_packed: u32,
    // Sentinel 0xFFFFFFFF means a process dispatch (writes into
    // plane_cmyk). Any other value identifies a spot ink — combined
    // with `spot_channel` to drive the bound spot plane buffer.
    spot_id: u32,
    // Which channel of the packed spot plane to write (0..=3).
    // Equals `spot_id % 4` on the Rust side.
    spot_channel: u32,
    // 0..255 tint factor for spot dispatches. Multiplied by the
    // scratch alpha to produce the per-pixel tint contribution;
    // ignored for process dispatches.
    spot_tint: u32,
    // Output pixel dimensions. We dispatch a 2-D grid over (width,
    // height) and bail in any thread outside this rect — matches the
    // tail-handling pattern of the rest of the wgpu work in this
    // crate.
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<storage, read_write> plane_cmyk: array<u32>;
@group(0) @binding(1) var<storage, read_write> coverage: array<u32>;
@group(0) @binding(2) var<storage, read> scratch_rgba: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;
// vello_target: the underlying RGBA framebuffer below the overprint.
// Used to recover the bottom CMYK on virgin-paper splats so we don't
// drop the colour of any prior non-overprint draw at this pixel.
// Mirrors the `rgb_to_naive_cmyk_8bit(target)` fallback the CPU
// rasterizer uses at `compose_cmyk_overprint_via_planes:331` when
// `coverage_prev == 0`.
@group(0) @binding(4) var<storage, read> vello_target: array<u32>;

// Spot target — only meaningful when `params.spot_id != 0xFFFFFFFFu`.
// Even on process dispatches the bind group supplies a 1-element
// sentinel so the layout matches; the shader's branch on `spot_id`
// guarantees we never read or write the sentinel.
@group(1) @binding(0) var<storage, read_write> spot_plane: array<u32>;

fn unpack_byte(packed: u32, channel: u32) -> u32 {
    return (packed >> (channel * 8u)) & 0xFFu;
}

fn pack_bytes(c: u32, m: u32, y: u32, k: u32) -> u32 {
    return (c & 0xFFu)
         | ((m & 0xFFu) << 8u)
         | ((y & 0xFFu) << 16u)
         | ((k & 0xFFu) << 24u);
}

// (a * b + 127) / 255, clamped to 255. Mirrors the CPU rasterizer's
// `(num * 255 + denom/2) / denom`-style fixed-point round in
// `splat_scratch_into_planes` and `compose_cmyk_overprint_via_planes`.
fn scale_8(a: u32, b: u32) -> u32 {
    let prod = a * b + 127u;
    let result = prod / 255u;
    return min(result, 255u);
}

// Naive RGB→CMYK at 8 bits — mirror of `rgb_to_naive_cmyk_8bit` in
// `crates/idml-gpu/src/cpu.rs`. Recovers the bottom-side CMYK from a
// previously-painted vello_target pixel so virgin-paper overprint
// composites don't drop the underlying ink. Inputs in [0..=255].
//
//   K = 255 - max(R, G, B)
//   if K == 255: C = M = Y = 0
//   else:        C = ((255 - R - K) * 255 + denom/2) / denom
//                M = ...
//                Y = ...
//                (denom = 255 - K)
//
// Round-trip with `naive_cmyk_to_rgb_8bit` is bit-stable for any CMYK
// produced by the forward map — that's the case Stage A was designed
// to handle and the case that actually hits this fallback (a prior
// Paint::Cmyk swatch painted via the cached `rgb`).
fn rgb_to_naive_cmyk_8bit(r: u32, g: u32, b: u32) -> vec4<u32> {
    let max_rgb = max(max(r, g), b);
    let k = 255u - max_rgb;
    if (k == 255u) {
        return vec4<u32>(0u, 0u, 0u, 255u);
    }
    let denom = 255u - k;
    let half = denom / 2u;
    let c_num = 255u - r - k;
    let m_num = 255u - g - k;
    let y_num = 255u - b - k;
    let c = min((c_num * 255u + half) / denom, 255u);
    let m = min((m_num * 255u + half) / denom, 255u);
    let y = min((y_num * 255u + half) / denom, 255u);
    return vec4<u32>(c, m, y, k);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let idx = gid.y * params.width + gid.x;

    // RGBA stored as little-endian RGBA8: byte 0=R, 1=G, 2=B, 3=A.
    // Vello + the buffer copy preserve that ordering.
    let scratch_word = scratch_rgba[idx];
    let alpha = (scratch_word >> 24u) & 0xFFu;
    if (alpha == 0u) {
        return;
    }

    let cov_prev = coverage[idx] & 0xFFu;
    let virgin = cov_prev == 0u;

    let mask_c = unpack_byte(params.ink_mask_packed, 0u);
    let mask_m = unpack_byte(params.ink_mask_packed, 1u);
    let mask_y = unpack_byte(params.ink_mask_packed, 2u);
    let mask_k = unpack_byte(params.ink_mask_packed, 3u);

    // Recover bottom-side CMYK on virgin pixels by inverting the
    // vello_target RGB through the same `rgb_to_naive_cmyk_8bit` the
    // CPU rasterizer uses (cpu.rs:331-356). Pixels that were paper-
    // white come back as (0, 0, 0, 0); pixels painted by a prior
    // Paint::Cmyk swatch round-trip back to their original CMYK with
    // bit stability so the per-channel max composite stays correct.
    var bot_c: u32 = 0u;
    var bot_m: u32 = 0u;
    var bot_y: u32 = 0u;
    var bot_k: u32 = 0u;
    if (virgin) {
        let target_word = vello_target[idx];
        let tr = target_word & 0xFFu;
        let tg = (target_word >> 8u) & 0xFFu;
        let tb = (target_word >> 16u) & 0xFFu;
        let ta = (target_word >> 24u) & 0xFFu;
        if (ta != 0u) {
            // Un-premultiply if needed. The Vello render target uses
            // straight (non-premultiplied) alpha so this is usually a
            // no-op except for partially-transparent pixels.
            var r: u32 = tr;
            var g: u32 = tg;
            var b: u32 = tb;
            if (ta != 255u) {
                let half = ta / 2u;
                r = min((tr * 255u + half) / ta, 255u);
                g = min((tg * 255u + half) / ta, 255u);
                b = min((tb * 255u + half) / ta, 255u);
            }
            let bot = rgb_to_naive_cmyk_8bit(r, g, b);
            bot_c = bot.x;
            bot_m = bot.y;
            bot_y = bot.z;
            bot_k = bot.w;
        }
    }

    if (params.spot_id == 0xFFFFFFFFu) {
        // Process dispatch: update the four packed CMYK planes.
        let cur = plane_cmyk[idx];
        // For non-virgin pixels the plane state is the truth. For
        // virgin pixels seed from the vello_target round-trip so the
        // per-channel max composite folds in any prior non-overprint
        // CMYK draw at this pixel.
        let cur_c = select(unpack_byte(cur, 0u), bot_c, virgin);
        let cur_m = select(unpack_byte(cur, 1u), bot_m, virgin);
        let cur_y = select(unpack_byte(cur, 2u), bot_y, virgin);
        let cur_k = select(unpack_byte(cur, 3u), bot_k, virgin);
        let new_c = scale_8(mask_c, alpha);
        let new_m = scale_8(mask_m, alpha);
        let new_y = scale_8(mask_y, alpha);
        let new_k = scale_8(mask_k, alpha);
        // Per-channel max: adding ink can only darken a channel.
        let out_c = max(cur_c, new_c);
        let out_m = max(cur_m, new_m);
        let out_y = max(cur_y, new_y);
        let out_k = max(cur_k, new_k);
        plane_cmyk[idx] = pack_bytes(out_c, out_m, out_y, out_k);
    } else {
        // Spot dispatch: read-modify-write the selected channel of
        // the bound packed spot plane (4 spots / u32 lane). Other
        // channels carry tints for the other three spots in this
        // texture group — preserve them unchanged.
        let cur = spot_plane[idx];
        let cur_chan = unpack_byte(cur, params.spot_channel);
        let new_chan_raw = scale_8(params.spot_tint, alpha);
        let new_chan = max(cur_chan, new_chan_raw);
        // Reconstruct the u32 with only the targeted byte updated.
        let mask = 0xFFu << (params.spot_channel * 8u);
        let cleared = cur & ~mask;
        spot_plane[idx] = cleared | ((new_chan & 0xFFu) << (params.spot_channel * 8u));

        // Seed the process planes from the vello_target on virgin
        // pixels so the recomposite's union-of-process-and-spot has
        // any prior non-overprint CMYK draw to mix with. Without
        // this, a spot overprint over an existing magenta fill would
        // discard the magenta on first touch.
        if (virgin && (bot_c != 0u || bot_m != 0u || bot_y != 0u || bot_k != 0u)) {
            plane_cmyk[idx] = pack_bytes(bot_c, bot_m, bot_y, bot_k);
        }
    }

    // Coverage: every CMYK draw (process or spot) marks the pixel.
    let new_cov = max(cov_prev, alpha);
    coverage[idx] = new_cov;
}
