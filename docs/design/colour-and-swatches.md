# Paged — Concept 2: Colours & Swatches

June 2026. Concept paper. Sections describe intent; where the implementation differs, `../status.md` and the ADRs in `../adr/` are authoritative.

Sections not relevant outside the original planning context have been removed; numbering is unchanged.

*Status note (2026-10-02): source comments in this repository cite this document as "Concept 2" and [pdf-export.md](pdf-export.md) as "Concept 3", with the decision ids (C1 to C8) and the numbered acceptance criteria used below. "Concept 1" is a toolbar concept for the editor and is not in this repository. The decisions that came out of this concept are recorded in [ADR 003](../adr/003-lcms2-color.md) (the colour-management engine) and [ADR 106](../adr/106-colour-resolved-at-build-time.md) (how colour is resolved).*

*Subtitle: an IDML-faithful colour model, a colour-management engine, and the freieFarbe open library.*

---

## Scope

This concept covers the colour substrate beneath three catalogued panels (`paged.swatches`, `paged.color`, `paged.gradient`) plus the Ink Manager and proof-setup surfaces they imply. The panels' *binding* behaviour is already settled by the panel catalogue (an internal design note); what is unspecified — and what this concept defines — is the **colour model** and the **colour-management engine (CMM)** that resolves colour to screen and to print.

In scope:

- The Rust colour model: `ColorValue`, `Swatch`, `ColorModel`, `Tint`, `Gradient`, `MixedInk`/`MixedInkGroup`, `ColorGroup`, and the reserved specials — as IDML-faithful, tsify-sourced types.
- The CMM: display resolution, gamut check, soft-proofing, and export conversion (the last consumed by Concept 3).
- Document colour settings: working spaces, rendering intents, black-point compensation, assign/convert policies.
- The freieFarbe HLC atlas as a bundled open library, with the `.ase` import path.
- The Ink Manager (spot↔process, aliasing, standard Lab values for spots) as the bridge to output.
- The `paged.swatches`/`paged.color`/`paged.gradient` panel shapes (recap) and the `"mixed"` sentinel for heterogeneous selection.

Out of scope:

- The PDF colour-space *encoding* (DeviceN/Separation/ICCBased/Lab objects) — Concept 3, which *consumes* this engine's export-convert mode.
- Image colour management for placed assets at export time — Concept 3 (the engine is shared; the policy lives with export).
- Transparency/blend (opacity, blend modes, effects) — that is `paged.effects`, a separate composition; colour and transparency are orthogonal axes.

## Position in the architecture

*Status note (2026-10-02): this section predates the implementation; `crates/paged-color` holds the colour-management engine (`cmm.rs`), the Lab conversion (`lab.rs`) and the `.ase` codec (`ase.rs`), but not the colour model, and there is no `crates/paged-color/src/model.rs`. The model stayed the IDML colour model (`ColorEntry`, `ColorSpace`, `ReservedSwatch` in `crates/paged-model/src/lib.rs`); the engine works on `WorkingColor`, an adapter over its resolved channels (`crates/paged-color/src/cmm.rs`).*

The colour model is Rust-side source of truth via tsify, like everything crossing the WASM boundary. It lives in a new `crates/paged-color` crate, consumed by the renderer (display resolution feeds Vello), by the panels (the model is read via `documentCollection:swatches` / `documentCollection:gradients` / `documentCollection:colorGroups`), and — in Concept 3 — by the PDF backend (export conversion). The CMM is a single crate behind a narrow trait so the implementation is swappable and never leaks into the panels or the exporter.

```
        crates/paged-color  (model + CMM, #[derive(Tsify)])
        ┌───────────────────────────────────────────────┐
        │  ColorValue · Swatch · Tint · Gradient ·        │
        │  MixedInk · ColorGroup · specials               │
        │  ─────────────────────────────────────────────  │
        │  CMM trait: resolve_display · check_gamut ·      │
        │             soft_proof · convert_for_export      │
        └───────────────────────────────────────────────┘
              │                 │                  │
        Vello display     swatch panels       PDF export
        (Concept 1's      (documentCollection) (Concept 3,
        fill/stroke too)                        export-convert)
```

## The Rust colour model = the IDML colour model

*Status note (2026-10-02): the types sketched below were not built as a new type system; see the note under "Position in the architecture".*

IDML represents colour as `<Color>` elements carrying a colour `Space` (`CMYK` | `RGB` | `LAB`), a `ColorValue` (the components), and a `Model` (`Process` | `Spot` | `Registration` | `Mixed`), plus reserved swatches (`[None]`, `[Paper]`, `[Black]`, `[Registration]`). Tints reference a base colour with a tint percentage; gradients carry stops; mixed inks reference spot colourants via a mixed-ink group. To round-trip IDML losslessly, the model represents all of this natively. Converting everything to RGB on import destroys print intent — the canonical data-loss bug is a 100%-K text black silently becoming a four-colour rich black.

```rust
// crates/paged-color/src/model.rs  — Rust source of truth, #[derive(Tsify)]

#[derive(Tsify, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(tag = "space", content = "value", rename_all = "camelCase")]
pub enum ColorValue {
    Cmyk { c: f32, m: f32, y: f32, k: f32 },   // 0..1 each
    Rgb  { r: f32, g: f32, b: f32 },            // 0..1, relative to the doc RGB working space
    Lab  { l: f32, a: f32, b: f32 },            // CIELAB, D50
    Gray { k: f32 },                            // 0..1
}

#[derive(Tsify, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum ColorModel { Process, Spot, Registration }

#[derive(Tsify, Serialize, Deserialize, Clone, Debug)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct Swatch {
    pub id: SwatchId,
    pub name: String,                  // for spots this IS the colourant identity
    pub model: ColorModel,
    pub value: ColorValue,
    /// For spot colours: the alternate (process/Lab) used for on-screen preview
    /// and as the PDF Separation tint transform. Never the colourant identity.
    pub spot_alternate: Option<ColorValue>,
    pub group: Option<ColorGroupId>,
    /// Optional library provenance, e.g. "HLC H010_L20_C010" — preserved on round-trip.
    pub provenance: Option<String>,
}

#[derive(Tsify, Serialize, Deserialize, Clone, Debug)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct Tint {                      // a tint is a swatch in its own right in IDML
    pub id: SwatchId,
    pub base: SwatchId,
    pub tint: f32,                     // 0..1 — a percentage of ONE ink, not opacity
}

#[derive(Tsify, Serialize, Deserialize, Clone, Debug)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct GradientStop {
    pub color: SwatchRef,              // ref to a swatch/tint, NOT an inline colour
    pub location: f32,                 // 0..1 along the ramp
    pub midpoint: f32,                 // 0..1 — the 50% blend point to the next stop
}

#[derive(Tsify, Serialize, Deserialize, Clone, Debug)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Gradient {
    Linear { stops: Vec<GradientStop> },
    Radial { stops: Vec<GradientStop> },
}

#[derive(Tsify, Serialize, Deserialize, Clone, Debug)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct MixedInk {                  // a fixed combination of spot inks
    pub id: SwatchId,
    pub components: Vec<(SwatchId, f32)>,   // (spot swatch, ink percentage 0..1)
}
// MixedInkGroup generates a grid of MixedInk variants over 2+ spots; preserved as a group.

/// The reserved swatches. Never editable or deletable.
#[derive(Tsify, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum ReservedSwatch {
    None,          // no paint
    Paper,         // the substrate; editable colour for preview, but not "white ink"
    Black,         // 100% K process
    Registration,  // prints on ALL plates — for crop/registration marks
}
```

Two modelling subtleties worth pinning:

- **A gradient stop references a swatch, it does not inline a colour.** This is how IDML keeps a gradient consistent when its component swatch is edited, and how a gradient can contain spot stops that must export as `Separation`. Inlining colour into stops would break both.
- **`[Paper]` is not "white."** It is the substrate colour; users tint it for mock-ups on coloured stock, and it must never be emitted as an opaque white ink at export. It models "knockout to substrate."

### IDML round-trip table

| IDML construct | Model representation | Round-trip note |
| -------------- | -------------------- | --------------- |
| `Space=CMYK Model=Process` | `Cmyk` + `Process` | values preserved verbatim; never auto-converted on import |
| `Space=RGB` | `Rgb` (working-space tagged) | the document RGB working space matters for display |
| `Space=LAB` | `Lab` (D50) | device-independent; the clean import target (see freieFarbe) |
| `Model=Spot` | `Spot` + `spot_alternate` | spot name is the colourant identity; alternate is preview/export-only |
| `Tint` | `Tint` over a base swatch | 50% of a spot stays 50% of *one ink*, not 50% opacity |
| `Gradient` | `Linear`/`Radial` + stops (swatch refs + midpoints) | stops reference swatches; midpoints preserved |
| `MixedInk` / `MixedInkGroup` | `MixedInk{components}` + group | multi-spot; full fidelity can be v2 (see C5) |
| `ColorGroup` | `ColorGroup` membership on swatches | panel grouping; preserved |
| `[None]/[Paper]/[Black]/[Registration]` | `ReservedSwatch` | never editable/deletable; `Registration` prints on all plates |

## The colour engine (the shared spine with Concept 3)

*Status note (2026-10-02): this section predates the implementation; the `Cmm` trait in `crates/paged-color/src/cmm.rs` has `resolve_display`, `check_gamut` and `convert_for_export`, and takes rendering intent and black-point compensation as constructor state, not per call. Soft-proofing and the document colour settings are the wire operations `SetProofSetup` and `SetColorSettings` (`crates/paged-wire/src/lib.rs`). See [ADR 003](../adr/003-lcms2-color.md) and [ADR 106](../adr/106-colour-resolved-at-build-time.md).*

This is the part that does not exist yet and that both this concept and PDF export need. The CMM in `paged-color` does four jobs behind a narrow trait:

```rust
pub trait Cmm {
    /// For the canvas: any ColorValue → the surface display space (sRGB / Display-P3),
    /// premultiplied as Vello expects. CMYK and Lab MUST go through an ICC transform.
    fn resolve_display(&self, c: ColorValue, intent: Intent) -> DisplayRgba;

    /// Out-of-gamut flag for the mixer warning (a derived read, not a binding kind).
    fn check_gamut(&self, c: ColorValue, dest: &Profile) -> GamutStatus;

    /// Soft-proofing: simulate the output-intent device on screen (InDesign "Proof Colors").
    fn soft_proof(&self, c: ColorValue, dest: &Profile, intent: Intent, paper_white: bool) -> DisplayRgba;

    /// For export (Concept 3): convert PRESERVING native spaces where the policy says so.
    fn convert_for_export(&self, c: ColorValue, policy: ExportColorPolicy) -> ColorValue;
}
```

1. **Display resolution.** Convert any `ColorValue` to the surface display space for Vello. The trap to avoid: treating CMYK as a naive `1 − x` RGB approximation. CMYK and Lab must go through a real ICC transform via the document's working-space profiles. Be explicit about the linear-vs-gamma boundary: Vello expects a defined space; resolve to sRGB (or Display-P3 if the surface advertises it) and hand premultiplied values, so blending happens in the space the renderer assumes rather than silently mixing encodings.
2. **Gamut check.** Flag out-of-gamut colours for the `paged.color` mixer's warning triangle — a derived read against the destination profile, never a binding kind.
3. **Soft-proofing.** Simulate the output condition on screen (InDesign's *Proof Colors* / *Proof Setup*), optionally simulating paper white and black ink. This is what lets a designer see, on a screen, roughly what FOGRA-coated stock will do to a vivid RGB. It reuses the same transforms as export but renders to the display.
4. **Export conversion.** The same machinery, but *preserving* native spaces per policy rather than collapsing to display. Concept 3 consumes this.

### Document colour settings

The document carries colour management state mirroring InDesign's *Color Settings*, read declaratively as `documentMeta`:

- **RGB working space** (e.g. sRGB, Adobe RGB, Display-P3) and **CMYK working space** (e.g. ISO Coated v2 / FOGRA39, PSO Uncoated v3, GRACoL 2006).
- **Default rendering intent** — one of *perceptual*, *relative colorimetric*, *saturation*, *absolute colorimetric*. Display typically uses relative-colorimetric + black-point compensation; images often perceptual; proofing absolute. Carry it per-document with per-object override room later.
- **Black-point compensation** on/off.
- **Assign/convert policy on open** — preserve embedded numbers, convert to working space, or colour-management-off; and how to treat missing/mismatched profiles. This is the policy the importer applies when an IDML (or placed asset) arrives tagged or untagged.

### CMM library choice — the WASM caveat, stated honestly

*Status note (2026-10-02): decided: lcms2 natively and qcms on wasm32; see [ADR 003](../adr/003-lcms2-color.md). The measurement is `crates/paged-color/tests/parity.rs`.*

- **`lcms2`** (the `rust-lcms2` bindings over Little CMS) is the mature, production-proven engine — stable for years, full ICC v2/v4 support, the de-facto reference CMM. The wrinkle: it is a C library via FFI, and compiling Little CMS for `wasm32-unknown-unknown` (the renderer target) is not free — you either build the C to WASM separately or accept an Emscripten-flavoured toolchain. Worth a spike before committing.
- **A pure-Rust CMM** (e.g. `moxcms` and peers emerging in the imaging ecosystem) sidesteps the C-in-WASM build entirely. Newer; verify ICC v4 coverage, all four rendering intents, and accuracy against known measurement patches before trusting it for print. Lower build friction for a `wasm32` target.

Recommend a short **bake-off** behind the `Cmm` trait: lcms2 (proven, build-heavy in WASM) vs a pure-Rust CMM (lighter build, accuracy to be proven). Because both sit behind the trait, the choice is swappable and never leaks into the panels or the exporter. This is the one genuine unknown in the three concepts and the reason to do this concept first (retire the risk where it is cheapest).

### Profile sourcing

*Status note (2026-10-02): this section predates the implementation; no profile ships with the engine, and the host registers profile bytes. See [ADR 109](../adr/109-engine-does-no-io.md).*

Ship a small set of common, redistributable ICC profiles (sRGB, and the freely redistributable ECI/FOGRA and IDEAlliance/GRACoL profiles where licensing permits), and allow the user to add their own. Profiles are versioned alongside the document's colour settings so a re-open is deterministic. Do not bake profiles into the WASM binary; load them as assets.

## freieFarbe / HLC as a bundled open swatch library

*Status note (2026-10-02): this section predates the implementation; the `.ase` parser and writer are `crates/paged-color/src/ase.rs`, reached through the wire operation `ImportSwatchLibrary`. This repository bundles no swatch library ([ADR 109](../adr/109-engine-does-no-io.md)).*

The freieFarbe e.V. **CIELAB HLC Colour Atlas** is a strong fit and a genuine differentiator, for a precise technical reason: it is **defined in CIELAB**, and IDML has native `Space=LAB` swatches. An HLC colour imports as a `Lab` swatch and **round-trips to InDesign losslessly** — no gamut-dependent conversion, no "which CMYK profile did you assume" ambiguity. It is the cleanest possible import target in the model above, and it pairs naturally with soft-proofing: a device-independent colour the CMM can render into any output condition.

What freieFarbe publishes (free, under Creative Commons): the atlas at **2,040 colours (Standard)** and **13,000+ (XL)**, as **`.ase` (Adobe Swatch Exchange) libraries** for Creative Cloud, plus **CxF3 spectral data** and CMYK/sRGB value tables. The HLC naming encodes the colour itself — `H{hue}_L{lightness}_C{chroma}` — so the name is meaningful and should be preserved as `Swatch.provenance` on round-trip.

### The `.ase` import path

ASE is a documented (community-reverse-engineered, stable) binary format: a `ASEF` signature, a version, a block count, then blocks of three kinds — *group start* (`0xC001`), *colour entry* (`0x0001`), *group end* (`0xC002`). Each colour entry carries a UTF-16 name, a four-byte colour-model tag (`"RGB "`, `"CMYK"`, `"LAB "`, `"Gray"`), big-endian float components, and a colour type (`0` global, `1` spot, `2` process/normal). The mapping to the model is direct: model tag → `ColorValue` variant, type → `ColorModel`, group blocks → `ColorGroup`. A `~200-line` parser covers it. For HLC specifically, every entry is `"LAB "` + `Lab` + (global/process), so it lands as `Lab`/`Process` swatches in an "HLC" colour group, names preserved.

Support the same parser for arbitrary user `.ase` libraries (the format is the lingua franca of swatch exchange), and offer **load/save `.ase`** from the Swatches panel so Paged interoperates with the wider design toolchain. Importing directly from another IDML's swatch list is the other interop path and falls out of the model for free.

The **CxF spectral data** is out of v1 scope but worth noting for the roadmap: spectral definitions enable *real* spot-colour proofing (predicting how an HLC spot prints on a given stock under a given illuminant), which is a high-end differentiator the device-independent model is already positioned for.

## The Ink Manager (the bridge to output)

*Status note (2026-10-02): the output-time ink settings are the wire operations `SetInkSetting` and `SetUseStandardLabForSpots` (`crates/paged-wire/src/lib.rs`); the panel is in the editor repository.*

The Ink Manager is the production surface that sits between swatches and export, and it deserves to exist in this concept because it operates on the colour model, not the document geometry:

- **Spot → process conversion** (per-ink or "all spots to process"): does not edit the swatch's identity, it sets an output-time substitution. Modelled as ink settings in `documentMeta`, consumed by Concept 3.
- **Ink aliasing**: map one spot to another (two near-identical spots collapse to one plate). Again an output-time mapping, not a swatch edit.
- **Use standard Lab values for spots**: prefer the spot's Lab definition over its CMYK alternate when converting — exactly the reason the `Lab` import target matters. InDesign exposes this toggle; Paged should too.
- **Ink density / sequence / type** (opaque, transparent, OPI) for trapping and overprint preview — far-future, but the data hangs off the same ink model.

The Ink Manager is a hybrid panel (composition chrome over the ink list, with the conversion/aliasing controls as `documentMeta` writes). It is the natural home for the spot-handling decisions that Concept 3's `Separation`/`DeviceN` encoding then honours.

## Panel shapes (recap + the `"mixed"` sentinel)

*Status note (2026-10-02): the panels described here belong to the editor repository, not to this one.*

- **`paged.swatches`** (hybrid): composition chrome + an expert swatch-grid child + a new-swatch popover (the CMYK/RGB/Lab mixer, which is `paged.color` reused). Apply-on-click is the `selectionProperty` write of the swatch ref; create/edit/delete/rename/merge/group is `paged.mutate(Operation::CreateSwatch{…})` etc. from the expert child. The grid rows carry type/mode badges (CMYK/RGB/Lab, process/spot, none/paper/registration) and a tint slider.
- **`paged.color`** (composition): a live mixer of bound sliders (CMYK/RGB/Lab/HSB), a tint slider, a hex input, the out-of-gamut warning (CMM-derived read), and add-to-swatches. The mixer is the new-swatch popover reused.
- **`paged.gradient`** (expert leaf): a draggable multi-stop ramp with midpoints, type (linear/radial), angle, reverse — writing the whole gradient as one `selectionProperty` property (the interaction is expert; the write is one path).

The **`"mixed"` sentinel** handles heterogeneous selection: when two selected frames have different fills, the well/mixer shows a "mixed" state rather than a value, and a write replaces it across the selection. This is a resolution refinement, not a binding kind — it lives in how the property resolver reports a multi-object selection, and the colour controls render the sentinel.

## What not to do

- **Don't auto-convert CMYK/Lab to RGB on import.** Preserve native spaces; resolve to display only for the canvas. The rich-black bug is the cautionary tale.
- **Don't inline colours into gradient stops.** Stops reference swatches so edits propagate and spot stops survive to `Separation` at export.
- **Don't treat `[Paper]` as white ink or `[Registration]` as black.** Paper is substrate (knockout); Registration prints on all plates.
- **Don't conflate tint with opacity.** A 50% tint is 50% of one ink; opacity is transparency (`paged.effects`). Different axes, different model fields, different PDF encodings.
- **Don't approximate CMYK→RGB without an ICC transform.** It is the single most visible "this is a toy" failure in a DTP tool.
- **Don't bake ICC profiles into the WASM binary.** Load them as versioned assets so re-opens are deterministic and profiles are swappable.
- **Don't fork the HLC data.** Ship/parse the originals with attribution.
- **Don't ship proprietary colour libraries.** The open HLC atlas is the deliberate alternative.
- **Don't let the CMM choice leak past the trait.** Panels and the exporter see `Cmm`, never lcms2 or a specific pure-Rust crate.

## Acceptance criteria

1. An IDML with process CMYK, RGB, Lab, spot, tint, gradient, and (where present) mixed-ink swatches imports, displays, and re-exports to IDML with byte-faithful colour data (no space coercion).
2. A pure 100%-K swatch survives import → display → IDML export as 100% K, not rich black.
3. CMYK and Lab colours render on the canvas through an ICC transform, not a naive approximation; switching the CMYK working space visibly changes on-screen rendering.
4. The `paged.color` mixer shows an out-of-gamut warning against the current CMYK working space, derived from the CMM.
5. Soft-proofing toggles the canvas to simulate the output condition (and optionally paper white) coherently.
6. The freieFarbe HLC `.ase` library imports as a Lab/Process colour group with HLC names preserved as provenance; attribution appears in the panel and `NOTICE`; the shipped files are the unmodified originals.
7. Arbitrary user `.ase` libraries import; the Swatches panel can save a `.ase` back out.
8. The Ink Manager converts a chosen spot to process at output time without altering the swatch identity, and can prefer the spot's Lab value.
9. Applying a swatch is a single `selectionProperty` write; creating/editing one is `paged.mutate` from the expert child — no new binding kind exists.
10. The CMM implementation is reachable only through the `Cmm` trait; no panel or the exporter imports the underlying library directly.

## Decision triggers

1. **After the CMM bake-off, before the panels.** Pick lcms2 or a pure-Rust CMM on measured accuracy + WASM build cost. If neither is comfortable, that is a real architectural finding to surface now, not after the panels depend on it.
2. **When the first real IDML round-trips.** Diff the exported colour table against the source. Any non-identity transform that isn't a deliberate policy is a model bug to fix before building on it.
3. **When Concept 3's export encoding lands.** Re-examine `convert_for_export` against `Separation`/`DeviceN`/`ICCBased` needs; the export side is the real test of the engine's preserve-native-spaces contract.
4. **When MixedInk is exercised (C5).** Decide v1 vs v2 fidelity based on whether real source documents use mixed inks; approximate-via-alternate while preserving data is the safe v1.

## Decisions register

*Status note (2026-10-02): the statuses in the table are those of the draft. C3 was decided in [ADR 003](../adr/003-lcms2-color.md). C6 ended differently: no profile ships with the engine ([ADR 109](../adr/109-engine-does-no-io.md)). C1 was built differently, see the note under "Position in the architecture".*

| # | Decision | Status |
| - | -------- | ------ |
| C1 | `ColorValue`/`Swatch`/`Tint`/`Gradient`/`MixedInk`/`ColorGroup`/specials live in a `paged-color` Rust crate, tsify'd. IDML round-trips natively; never auto-convert on import. | Proposed |
| C2 | Stand up a CMM behind a `Cmm` trait with display-resolve, gamut-check, soft-proof, and export-convert. Document carries RGB+CMYK working spaces, intent, and BPC as `documentMeta`. | Proposed |
| C3 | CMM library bake-off: `lcms2` (proven, heavy WASM build) vs a pure-Rust CMM (lighter build, verify accuracy). Decide behind the trait. **The one genuine unknown — do it first.** | Open |
| C4 | Bundle the freieFarbe HLC atlas as a built-in Lab swatch group via an `.ase` parser. Ship the original files with attribution. | Proposed |
| C5 | MixedInk fidelity is v2; v1 may approximate via the resolved alternate while preserving the data on round-trip. | Open |
| C6 | Ship redistributable ICC profiles as versioned assets, not baked into the WASM binary; users may add their own. | Proposed |
| C7 | Gradient stops reference swatches (not inline colours); `[Paper]`/`[Registration]` modelled as knockout/all-plates, never white/black ink. | Proposed |
| C8 | The Ink Manager (spot→process, aliasing, standard-Lab-for-spots) is a hybrid panel writing `documentMeta` ink settings, consumed by Concept 3 at export. | Proposed |

## Prior art / reference implementations

- **Typst's `color` module** (oklab/oklch/linear-rgb/srgb/cmyk/luma enum + conversions) is a clean shape reference for `ColorValue` and its conversion surface — useful for validating the enum shape, though Typst's colour goals are screen-leaning and do not cover spot/ICC print needs.
- **Shared text stack.** Typst and krilla (Concept 3) build on `rustybuzz`, `ttf-parser`, `unicode-bidi`. Glyph fill/stroke colour resolution runs through this engine, so aligning the font types across renderer, layout, and export keeps one vocabulary.
- **Little CMS** is the reference CMM behind most of the industry; `rust-lcms2` exposes it. A pure-Rust CMM is the WASM-friendly alternative under evaluation (C3).

## How this fits with the other two

- **This is the shared spine.** The CMM resolves colour for the canvas, warns on gamut, soft-proofs, and converts for export. Build it once, behind a trait. **Concept 3 (PDF export) is dramatically smaller if this engine already exists**, and **Concept 1's fill/stroke wells consume this colour model directly.**
- **Suggested position in the build sequence: first.** It unblocks the most and carries the one genuine unknown (the CMM bake-off, C3); retiring that early means getting the load-bearing dependency wrong where it is cheapest to fix.
