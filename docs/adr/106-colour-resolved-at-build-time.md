# ADR 106 — Colour is resolved to linear RGB at build time; CMYK channels and spot inks ride along

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `Paint` in `crates/paged-compose/src/display_list.rs`; `crates/paged-renderer/src/pipeline/color_paint.rs`; the plane code in `crates/paged-gpu` (`cpu.rs`, `separations.rs`, `cmyk_compute/`)

## Context

A document names its colours as CMYK, spot, Lab or RGB swatches. A screen needs RGB, and
the conversion from CMYK depends on an ICC profile ([ADR 003](003-lcms2-color.md) records
the colour engine). Three consumers need more than RGB: overprint combines ink channels, an
ink-coverage reading needs the separation, and the PDF exporter writes native colour
spaces ([ADR 119](119-pdf-export-backend.md)).

The doc comment on `Paint` gives both halves of the reason for the shape chosen. The CMYK
channels are carried "all the way through to the rasterizer — necessary for true
per-channel CMYK overprint compositing", and the resolved colour is kept on the paint so
that ordinary draws render "without re-running ICC at raster time"
(`crates/paged-compose/src/display_list.rs:60-66`, `:91-97`).

`crates/paged-color/src/lib.rs:18-21` gives a reason for the linear working space: the
final conversion "happens in a fragment shader so blending remains physically meaningful".
The code does not do that; see Consequences.

## Decision

Every swatch is resolved to a linear-RGB colour when the display list is built. A CMYK or
spot swatch additionally keeps its ink channels on the paint, and only overprint,
separations and PDF export read them.

- **`Paint::Cmyk { c, m, y, k, rgb, spot }`.** `rgb` is the ICC-converted colour, computed
  at build time. Ordinary draws use `rgb` alone. `spot` names an entry in `spot_inks`.
- **The profile is an input.** The host passes profile bytes, rendering intent and
  black-point compensation in `PipelineOptions` ([ADR 109](109-engine-does-no-io.md)).
- **No profile, no ink channels.** Without a transform a CMYK swatch becomes a
  `Paint::Solid` computed by the naive formula. A profile that fails to load is logged and
  treated the same way; the build does not fail.
- **Overprint on the CPU rasteriser** uses 8-bit planes per page pixel: one per process ink
  and one per named spot ink. A CMYK paint drawn with overprint outside a group composes
  through the planes. Inside a group buffer, and for RGB paints and gradients, overprint
  falls back to a `Darken` blend.
- **Separations** are read from the same planes. Pixels with no ink decomposition are
  reported as unknown and are not estimated.

## Evidence

- `crates/paged-compose/src/display_list.rs:60-78`, `:91-112` — `Paint::Cmyk`, its channels, the pre-baked `rgb`, the `spot` routing
- `crates/paged-renderer/src/pipeline/color_paint.rs:271-304` — "ICC-resolve once at compose time and bake the result into the paint"; `:306-314` — the `Paint::Solid` path without a transform
- `crates/paged-renderer/src/pipeline/mod.rs:263-282` — `cmyk_icc_profile`, `cmyk_intent`, `cmyk_bpc`, `use_standard_lab_for_spots`; `crates/paged-renderer/src/pipeline/color_paint.rs:57-81` — `build_cmyk_transform`: "Failures are logged and swallowed"
- `crates/paged-gpu/src/cpu.rs:110-155` — `CmykPlanes`: process planes, coverage, spot planes; `:1315-1329` — plane overprint for CMYK paints outside a group, `Darken` fallback otherwise
- `crates/paged-gpu/src/separations.rs:26-55` — what a separation measures and what it leaves unknown
- `crates/paged-gpu/src/cpu.rs:3058-3065`, `crates/paged-gpu/src/vello_rs.rs:2853-2869` — each paint colour is encoded to sRGB before it is handed to the rasteriser

## Alternatives considered

- Recovering the destination's CMYK by inverting the rendered RGB: the first form of plane
  overprint, replaced by explicit planes so the composite works "regardless of how many
  (non-overprint) CMYK paints sit between it and the page background"
  (`crates/paged-gpu/src/cpu.rs:136-140`).
- Estimating unknown pixels in a separation from RGB: "Deliberately NOT done", because it
  "would make the headline TAC number confidently wrong"; "An honest hole beats a plausible
  fabrication" (`crates/paged-gpu/src/separations.rs:50-55`).
- A flag on `FillPath` in place of the separate overprint commands: rejected so that the
  common fill keeps one rasteriser arm (`crates/paged-compose/src/display_list.rs:1133-1136`).
- On the GPU, a `read_write` storage texture: avoided as a "non-portable" extension
  (`crates/paged-gpu/src/cmyk_compute/mod.rs:22-24`).

## Consequences

A host that registers no CMYK profile gets naive CMYK colour, no plane overprint and
nothing for a separation to read, because no `Paint::Cmyk` is emitted.

Plane overprint and separations exist on the CPU rasteriser. The Vello rasteriser handles
overprint in its offscreen `rasterize` path only. A list with one to eight overprint
commands is rendered by the CPU rasteriser when the `cpu` feature is compiled in; otherwise
two compute shaders mirror the plane arithmetic, and if they are unavailable overprint is
drawn as knockout (`crates/paged-gpu/src/vello_rs.rs:177-221`). The browser surface builds its scene with `skip_overprints = false`, which draws an overprint
command as an ordinary knockout fill (`crates/paged-gpu/src/vello_rs.rs:435-437`, `:955-971`);
`crates/paged-gpu/src/surface.rs` does not use `cmyk_compute`.

Some comments describe a design the code does not have. `crates/paged-color/src/lib.rs:18-21`
and `crates/paged-compose/src/display_list.rs:24-25` say compositing happens in linear
light with the gamma applied by the GPU; both rasterisers encode every paint to sRGB first.
`crates/paged-renderer/src/pipeline/mod.rs:263-266` says ICC applies only where lcms2 is
available, "i.e. not wasm32"; on wasm32 the transform is built with qcms
(`crates/paged-color/src/lib.rs:27-32`). The doc of `FillPathOverprint`
(`crates/paged-compose/src/display_list.rs:1122-1131`) describes only an RGB darken
approximation and calls the per-channel composite deferred; the CPU rasteriser has it.

## Related

- [ADR 003](003-lcms2-color.md), [ADR 109](109-engine-does-no-io.md) — the colour engine behind the transform; the host supplies the profile
- [ADR 100](100-two-rasterisers-one-trait.md), [ADR 119](119-pdf-export-backend.md) — the consumers of the paint
