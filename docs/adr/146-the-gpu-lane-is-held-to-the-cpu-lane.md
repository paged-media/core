# ADR 146 — The GPU lane is held to the CPU lane

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-gpu` (`vello_rs.rs`), `paged-compose` (`mask.rs`, the shadow stamps both
  lanes draw), `corpus/generated/parity-diff.sh` and `parity-thresholds.json`, the parity step of
  `.github/workflows/gpu.yml`. No model, wire or protocol change.

## Context

The fidelity gate measures the CPU rasterizer against InDesign's PDFs. The editor and the
viewer draw with the other rasterizer, Vello on WebGPU, and nothing measured it: its unit tests
check one command at a time. Rendering every page of the generated corpus through both lanes and
diffing them (mean ΔE2000 per page, 144 dpi, Metal) found 13 of 74 fixtures above 0.2, and the
causes were defects, not rasterizer noise:

- **Drop shadows were skipped.** The `DropShadow` arm drew nothing; `PathShadow` stamped a
  multi-tap approximation that came out softer and wider than the CPU's Gaussian
  (`effects-attrs` 2.26, `rect-shadows` 1.47, `gradient-geometry` 1.40).
- **The paper was a backdrop.** Vello clears the target to the page colour, so a blend mode on
  blank paper blended with white: a Screen fill vanished, a Multiply group lost its colour
  (`transparency` 0.95, `transparency-targets` 0.97). In InDesign, and on the CPU lane, the page
  is a group whose backdrop is empty; the paper sits under it.
- **Dashes were dropped.** Every dashed stroke drew solid (`strokes-fills` 0.37, `line-ends`
  0.27); once dashes were taken, a dotted stroke (a zero-length dash with round caps) drew
  nothing, because kurbo's stroker caps no zero-length dash where tiny-skia does.
- **A gradient feather's layer was never closed**, so everything drawn after it on the page
  landed inside it.

## Decision

1. **One shadow builder for both lanes.** `paged_compose::mask::drop_shadow_stamps` turns a
   `DropShadow` or `PathShadow` into coverage stamps (the outer σ for a frame shadow, the blur
   radius for a path shadow; opacity, then spread as a choke on the blurred alpha, then noise).
   The CPU composites them; Vello draws the same rasters as images. A shadow is the CPU's
   shadow on the GPU, to the pixel grid.
2. **The page is one isolated group.** Vello encodes the page's content inside one `Normal`
   layer over the clear colour, so blend modes inside it read an empty backdrop, as the CPU
   lane's group buffer does. A presenter that draws a paper rectangle first gets the same
   result, because the page scene carries its own isolating layer.
3. **Dashes follow tiny-skia's rule.** A dash pattern is taken when tiny-skia would take it (an
   even count of two or more non-negative lengths with a positive sum); anything else strokes
   solid on both lanes. A zero-length dash gets a thousandth of a point, so its caps are drawn.
4. **A parity gate.** `corpus/generated/parity-diff.sh` renders every generated fixture through
   both backends from the same display list and holds each fixture's worst page to a budget in
   `parity-thresholds.json`: a default, and per-fixture entries above it, each with its
   measured cause. The never-loosen rule of `fidelity-thresholds.json` applies. The CPU side
   renders without `--snap-glyph-origins`, which imitates pdftoppm's glyph phase for the
   InDesign comparison and has no Vello counterpart. In CI the gate runs, hard, in the `gpu.yml`
   lavapipe job: the budgets were measured on Metal, and lavapipe puts all 77 fixtures inside
   them too.

## Known gaps (budgeted, not fixed)

- **Knockout and non-isolated groups.** A Vello layer is always isolated and composites its
  members over each other. A knockout group (`KnockoutGroup`) and a non-isolated group with an
  opacity below 100 % whose members use blend modes need the group's initial backdrop, which a
  Vello scene cannot read back. `transparency-targets` page 4 (cases 43–48) carries that
  difference. Both are rare in documents: a non-isolated group of `Normal` members, InDesign's
  default, is drawn exactly.
- **Dense text.** Pages of small justified text differ by 0.2–0.4 mean ΔE: the two lanes
  antialias glyph outlines differently (analytic coverage on tiny-skia, area coverage on Vello),
  and the difference sums over thousands of glyphs. It is not a layout difference: both lanes
  draw the same glyph paths at the same positions.

## The lane that tested nothing

Running the gate in CI showed every Vello page blank. Mesa now installs its lavapipe driver as
`lvp_icd.json`; `gpu.yml` pointed the Vulkan loader at `lvp_icd.x86_64.json`, so there was no
device, `init_gpu` failed, and every `vello_rs` test returned early on its empty buffer and
passed. The sanity step only warned. The lane had been green while testing nothing. The ICD is
now found by glob, a missing device fails the lane, and `paged-inspect --backend vello` prints
Vello's warnings and exits 3 on an all-empty render, so a parity run cannot measure a blank
page again.

## Consequences

- The editor and the viewer show drop shadows, dashed strokes and blend modes as the CPU lane
  and InDesign do.
- A change to the display list or to either rasterizer that moves the lanes apart shows up as a
  fixture over budget, with a heatmap, instead of in a user's document.
- `docs/status.md` no longer lists the shadow gap; it lists the two known gaps above.
