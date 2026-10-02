# ADR 104 — Object effects are modelled on InDesign's parameters over one shared mask pipeline

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-compose/src/mask.rs` and the effect variants of `DisplayCommand`; `crates/paged-renderer/src/module/effects.rs`; the effect arms of `crates/paged-gpu` (`cpu.rs`, `vello_rs.rs`) and `crates/paged-export-pdf/src/effects.rs`

## Context

A document carries InDesign's effect settings (`Size`, `Spread`, `Choke`, `Angle`,
`Distance`, `Depth` and so on). Three back ends draw them: the CPU rasteriser, the Vello
lane and the PDF exporter ([ADR 100](100-two-rasterisers-one-trait.md),
[ADR 101](101-display-list-single-intermediate.md)).

Commit `e5ea69a` (2026-09-06) records what was wrong before. `Size` was passed in as a
Gaussian σ on the canvas, "so every canvas shadow and glow was twice as soft and reached
twice as far — while the PDF lane used `Size/2` all along and the two silently disagreed".
`Choke` and `Spread` were fed in as points. The offset computed from `Angle` and `Distance`
had the wrong sign on both axes. The values were corrected by asking InDesign 20.0.1
directly, "by having it author the effect and export both the raster and its own IDML".

The same commit removed the CPU rasteriser's own effect renderers. Its stated reason: "all
three lanes now paint `paged_compose::mask`'s stamps, so a calibration that is right in one
is right in all of them by construction".

## Decision

Each object effect is its own `DisplayCommand` variant that carries InDesign's parameters,
and the coverage masks they are painted through are built once, in `paged_compose::mask`.

- **One construction.** The module doc: "take the object's coverage, move / grow / blur /
  invert it, combine two of them, and paint a colour through the result". `mask.rs` has "no
  rasterizer and no writer dependency", so every lane can call it.
- **Stamps.** Inner shadow, outer glow, inner glow, bevel and emboss, and satin are returned
  as `EffectStamp`s (a coverage raster, a colour, a blend mode). The CPU rasteriser, the
  Vello lane and the PDF exporter all paint these stamps.
- **`Size` is the width of the band**, not σ: σ = `Size` / 2 for effects that spill outside
  the object, 0.4 × `Size` for effects that stay inside.
- **`Choke` and `Spread` are fractions of `Size`**, not distances.
- **`Angle` names where the light comes from**, so the shadow falls the other way:
  `x = −distance · cos(angle)`, `y = +distance · sin(angle)` in y-down page coordinates.
- **A bevel is a facet exactly `Size` wide**, shaded by Lambert against a surface normal;
  `Depth` "steepens that facet rather than scaling the result".
- **Stacking order.** Outer glow is emitted before the fill; the inner effects and the
  feathers after it.

## Evidence

- `crates/paged-compose/src/mask.rs:15-26` — the one construction and its dependencies; `:620-632` — `EffectStamp`: "Every raster lane consumes these"
- `crates/paged-compose/src/mask.rs:634-669` — the measured σ mapping and the choke rule
- `crates/paged-compose/src/mask.rs:1009-1034` — the bevel model, read off InDesign exports
- `crates/paged-compose/src/display_list.rs:917-941`, `:1020-1087` — the effect variants: `DropShadow`, `PathShadow`, `InnerShadow`, `OuterGlow`, `InnerGlow`, `BevelEmboss`, `Satin`, `Feather`, `DirectionalFeather`, `GradientFeather`
- `crates/paged-renderer/src/module/effects.rs:24-29`, `:232-250` — stacking order; `polar_to_offset`
- `crates/paged-gpu/src/cpu.rs:1990-2079`, `:3354-3360`, `crates/paged-gpu/src/vello_rs.rs:1079-1099`, `crates/paged-export-pdf/src/effects.rs:93-147` — the CPU, Vello and PDF lanes paint the stamps

## Alternatives considered

All were earlier states of this code, replaced on 2026-09-06 (commits `72cd7b3` and `e5ea69a`):

- `Size` passed straight in as σ (`crates/paged-compose/src/mask.rs:645-647`).
- One effect renderer per lane; on Vello, concentric path stamps, with the bevel skipped
  (`crates/paged-gpu/src/vello_rs.rs:1084-1091`). The PDF and Vello lanes moved to the
  shared masks in `72cd7b3`, the CPU rasteriser in `e5ea69a`.
- A bevel from the gradient of blurred coverage: "the same bevel got weaker as the dpi rose".
- Growing and shrinking a shape by blur-and-threshold, replaced by thresholding the
  coverage's distance field.

## Consequences

A calibration of the five stamp effects applies to all three lanes at once. The fidelity
thresholds of the `effects` fixture were tightened in the same commit
([ADR 105](105-fidelity-gate.md)).

Not everything goes through the shared masks. The CPU rasteriser draws the three feathers
with its own functions (`crates/paged-gpu/src/cpu.rs:3422`, `:3509`, `:3680`). Vello takes
all three feather masks from `mask.rs` (`crates/paged-gpu/src/vello_rs.rs:480-500`). The PDF
exporter takes the plain and the directional feather mask from `mask.rs` and writes the
gradient feather as a vector soft mask (`crates/paged-export-pdf/src/transparency.rs:125-127`,
`:350-359`). `DropShadow` uses σ = `Size` / 2 on
the CPU and in the PDF; `PathShadow` uses 3.5 × `Size` on the CPU
(`crates/paged-gpu/src/cpu.rs:1498-1501`) and a multi-stamp approximation on Vello, where
`DropShadow` is skipped (`crates/paged-gpu/src/vello_rs.rs:1039-1078`).

`crates/paged-compose/src/extent.rs:63-84` repeats each effect's reach so that a
transparency group's bounds fit what the effect paints; a change to the mapping has to be
made there too.

The bevel has a recorded gap: where the band is wider than half the object, InDesign fades
the facet and this model does not (`crates/paged-compose/src/mask.rs:1032-1034`). The
module doc of the Vello backend still lists these effects as approximated and the bevel as
skipped (`crates/paged-gpu/src/vello_rs.rs:33-64`); the code no longer does that.

## Related

- [ADR 101](101-display-list-single-intermediate.md), [ADR 120](120-indesign-is-the-oracle.md) — the commands these effects are; the method of measuring InDesign
- [ADR 100](100-two-rasterisers-one-trait.md), [ADR 119](119-pdf-export-backend.md) — the three lanes
