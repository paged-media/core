# ADR 135 — Transparency per target, group isolation and knockout, and the effect attributes InDesign writes

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-model` (`TargetTransparency`, the `fill_transparency` / `stroke_transparency` /
  `content_transparency` and `fill_drop_shadow` / `content_drop_shadow` fields of the frame kinds,
  `GraphicLine` `opacity` / `blend_mode`, `Group` `isolate_blending` / `knockout_group`,
  `DropShadowSetting` `blend_mode` / `spread_pct` / `noise_pct` / `knocked_out` /
  `honor_other_effects` / `use_global_light`, the glow `technique`, the bevel blend modes, the
  feather highlight and follow-shape fields, `TransparencyPreference`), `paged-renderer`
  (`module/{frame,group,drop_shadow,effects}.rs`, `pipeline/{shapes,blend_shadow}.rs`),
  `paged-gpu` (`cpu.rs`: knockout groups, spread and noise), `paged-export-pdf` (group `/I` and
  `/K`), `paged-mutate` and `paged-introspect` (13 element paths and two `doc` rows). The IDML
  spelling is plugin-publish's (`idml-import` `spread.rs`, `designmap.rs`; `idml-export`
  `transparency.rs`, `effects.rs`, `preferences.rs`). Every model addition is serde-default and
  every path is additive: no protocol bump.

## Context

InDesign gives a page item four transparency targets: the object as a whole
(`<TransparencySetting>`), its stroke (`<StrokeTransparencySetting>`), its fill
(`<FillTransparencySetting>`) and its content, meaning the placed picture or a text frame's text
(`<ContentTransparencySetting>`). Each has its own opacity, blend mode and effects. The importer
read the object target and folded the other three into it, so a fill at 50 % faded the stroke
and the text as well. A graphic line had no opacity or blend mode at all and printed solid. A
group's `IsolateBlending` and `KnockoutGroup` were not read, and the exporter rewrote both to
`false` on save.

Several effect attributes were not read either: the drop shadow's `BlendMode`, `Spread`, `Noise`,
`KnockedOut`, `HonorOtherEffects` and `UseGlobalLight`, the glows' `Technique`, the bevel's
highlight and shadow blend modes, the gradient feather's highlight, and the directional feather's
`FollowShapeMode`. The document's global light (`<TransparencyPreference GlobalLightAngle
GlobalLightAltitude>` in `Resources/Preferences.xml`) was not read, so a shadow that follows it was
cast at its own stored angle.

InDesign 20.0.1 was asked (ADR 120). Two fixtures were built through its DOM, exported as IDML
and PDF, and the IDML spelling was read off its own export:

- `transparency-targets`, 48 cases: one target at a time at 50 % in Normal, Multiply and Screen,
  on text frames and graphic frames; a drop shadow per target; graphic lines; groups of two
  overlapping Multiply squares under every combination of isolate and knockout.
- `effects-attrs`, 31 cases: each effect attribute against its default, over paper and over a
  colour.

## Decision

**Targets.** Fill, stroke and content each carry a `TargetTransparency` (opacity, blend mode,
effects). The object target stays where it was. Each target is its own transparency group: the
renderer pushes one layer per target that is not plain Normal at 100 %, inside the object's layer.
A graphic line paints only its stroke, so its object and stroke groups wrap the same paint.

**Groups.** InDesign's object group is NON-isolated: its members blend with what lies under the
group (cases 37 and 45). An isolated group's members see only each other. In a knockout group, a
later member replaces earlier members where it covers them, compositing onto the group's initial
backdrop: a copy of the parent region, or transparent when the group is also isolated (cases
41–48). The CPU rasterizer implements both. The PDF writer emits `/I true` and `/K true` on the
group's transparency dictionary, as InDesign's own PDF does.

**Shadow strength.** InDesign casts a drop shadow from the object's composited alpha. A fill target
at 40 % casts a shadow 40 % as strong, which shows through the fill (`effects-attrs` cases 9–10:
the shadow falls from 165 to 66 levels, with `KnockedOut` true or false alike). The renderer
approximates this per painted part: the fill's opacity for a filled frame, the stroke's for a
stroke-only frame, and the stronger of the two when both are painted.

**Spread and noise.** Spread chokes the blurred alpha: the alpha is scaled by `1 / (1 − spread)`
and clamped, so that share of the band is solid before it fades (InDesign's and Photoshop's
definition). Noise multiplies the alpha by grain hashed per page-space cell, so the pattern does
not change with the resolution.

**Global light.** An effect with `UseGlobalLight="true"` takes its angle (and, for bevels, its
altitude) from the document's `TransparencyPreference`, which defaults to 120° and 30°.

**Effect blend modes.** InDesign writes an effect's `BlendMode` only when it differs from that
effect's default (Multiply for shadows and satin, Screen for glows), so an explicit `Normal` is a
choice. The renderer used to read it as "absent" and substitute the default: a cyan glow in Normal
was screened, which prints white over white and over magenta (cases 17–20). The bevel's
`HighlightBlendMode` and `ShadowBlendMode` reach its two stamps the same way (cases 21–24).

**Default radial feather.** A radial gradient feather with no start of its own is drawn the way
InDesign's PDF writes it: a unit-circle shading under a matrix, read off six probe frames, a
rotated one, a stroked one and the fixture. The circle is centred on a box around the frame's
visible bounds (the stroke's outer edge), widened by 9 pt left and right, 8 pt above and 1 pt below
in the frame's own axes. Its radius is half that box's larger side. On a frame taller than it is
wide the circle becomes an ellipse whose vertical radius is stretched by the frame's height over
its width. The display list's `GradientFeather` carries that ratio (`radial_aspect`); the CPU
rasterizer and the PDF writer draw the ellipse, the latter as InDesign does.

**Surfaces.** Thirteen element paths are new: `frame{Fill,Stroke,Content}{Opacity,BlendMode}`,
`groupIsolateBlending`, `groupKnockout` and
`frameDropShadow{BlendMode,Spread,Noise,KnockedOut,UseGlobalLight}`. `documentGlobalLightAngle`
and `documentGlobalLightAltitude` are `doc` schema rows. A graphic line now reads and writes
`frameBlendMode` and `frameOpacity`; its fill tint keeps rejecting, because a line has no fill.

## Consequences

- Documents that fade a fill, a stroke or text on their own now print as InDesign prints them.
  Before, the whole object faded.
- Isolation and knockout survive an IDML round trip; the exporter no longer resets them.
- The Vello backend draws each target layer but ignores the new flags: its layers are always
  isolated, a knockout group composites as an ordinary one, and a path shadow's spread and noise
  are not applied. The CPU backend is the fidelity gate; the GPU parity work (P-0 to P-2) closes
  the gap.
- The per-part shadow strength is an approximation of InDesign's composited alpha. A frame whose
  fill and stroke differ in opacity casts the stronger part's shadow everywhere.
- Still open, and measured in `effects-attrs` page 2: `HiliteLength` / `HiliteAngle` move the
  radial shading's focal point to the circle's edge along the highlight angle (InDesign's PDF:
  `/Coords [cos θ −sin θ 0 0 0 1]`), which needs a focal radial in the display list; the bevels'
  Smooth technique shades softer than ours, which mitres the corners; the glows' Precise technique
  keeps a harder edge.
