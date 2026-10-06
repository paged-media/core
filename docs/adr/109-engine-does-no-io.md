# ADR 109 — The engine does no I/O and ships no assets; the host registers fonts and profiles

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-renderer` (`asset.rs`, `PipelineOptions`), `crates/paged-canvas` (`CanvasOptions`, the registries), `crates/paged-canvas-wasm/src/dispatch.rs`, `crates/paged-color/src/profiles.rs`

## Context

Laying out a document needs bytes the document does not always carry: font files, linked
images and an ICC profile for CMYK. `crates/paged-renderer/src/asset.rs:15-29` defines the
seam: hosts pass font, image and profile lookups to the renderer through one trait, which
is synchronous in Rust and returns `bytes::Bytes` "so cloning is a refcount bump, not a
memcpy".

For profiles the repository states a reason: "No profile ships with the engine: they are
large and individually licensed by their issuers"
(`crates/paged-color/src/profiles.rs:26-28`). For fonts the README states the fact, "Core
ships **no fallback face**" (`README.md:161`). The repository does not record why.

## Decision

The build pipeline opens no file and no URL: it asks the host. No font and no colour
profile is compiled into any crate.

- Fonts reach the build as `PipelineOptions::font` (a default face) and through
  `AssetResolver::resolve_font`; linked images through `AssetResolver::resolve_image`; the
  CMYK profile as bytes in `PipelineOptions::cmyk_icc_profile`.
- In the editor's engine the host sends `RegisterFont {family, style, bytes}` and
  `RegisterColorProfile {name, bytes}`. Both registries live on `WorkerCore` and survive
  `LoadDocument`.
- A registered profile becomes the working space in one of two ways: at load, when the
  document's declared profile name matches a registered one; or later through the
  `SetColorSettings` mutation, which refuses a name that is not registered.
- What the host does not supply degrades in a defined way. With no default face and no
  resolver hit, "text is skipped". A substituted face is traced and reported as
  `FontSubstituted`. A link that resolves to no bytes draws the placeholder and reports
  `ImageLinkMissing`. With no profile, CMYK converts by naive arithmetic.
- File access sits in helpers that a native host or a test chooses to call, among them
  `BytesResolver::link_dirs`, `font_registry_from_paths` (not compiled for wasm) and
  `paged_color::profiles::load_installed`.

## Evidence

- `crates/paged-renderer/src/asset.rs:51-77` — the `AssetResolver` trait
- `crates/paged-renderer/src/pipeline/mod.rs:235-242`, `:263-267` — `font`, `assets` and `cmyk_icc_profile` on `PipelineOptions`
- `crates/paged-renderer/src/pipeline/images.rs:683-702` — a linked image is resolved through `options.assets`; a miss is `LinkMissing`
- `crates/paged-canvas-wasm/src/dispatch.rs:82-90`, `:561-608` — the registries on `WorkerCore`; `RegisterFont` and `RegisterColorProfile`
- `crates/paged-canvas/src/model.rs:61-83`, `:1928-1947` — `CanvasOptions`; `SetColorSettings` rejecting an unregistered name
- `crates/paged-cli/src/options.rs:15-34`, `:78-95` — the order a host must follow, and the warning when no run shaped a glyph
- `crates/paged-renderer/src/diagnostics.rs:47-51`, `:66-73` — `ImageLinkMissing`, `FontSubstituted`
- `crates/paged-color/src/profiles.rs:154-176`, `crates/paged-renderer/src/asset.rs:94-100`, `:198-213` — the opt-in file helpers

## Alternatives considered

- An asynchronous resolver: the trait doc says the Rust surface is synchronous and that a
  Promise wrapper belongs at the JavaScript boundary
  (`crates/paged-renderer/src/asset.rs:23-26`).
- Reading linked images from disk in every host: only the `paged-inspect` binary sets
  `link_dirs` (`crates/paged-renderer/src/bin/inspect.rs:476`). `README.md:167-170` says
  this is something "the canvas model, and therefore the editor, cannot" do.

## Consequences

Every host loads assets itself and in a required order. `crates/paged-cli/src/options.rs`
exists as a type to hold that order: fonts, then the load, then the working space. A host
that skips a step gets a page that renders without error and looks wrong: unshaped text
and naive CMYK have no diagnostic code, and the CLI prints its own warnings.

`CanvasModel` builds its resolver from the font registry alone
(`crates/paged-canvas/src/model.rs:9743-9758`), so on that path a linked image whose bytes
are not in the document cannot be resolved.

"No assets" means fonts and profiles. Hyphenation patterns are compiled in
(`crates/paged-text/src/hyphenate.rs:95`, [ADR 103](103-hyphenation-sources.md)); the fonts
under `corpus/fonts/` are test fixtures.

Statements in the repository that are behind the code:

- `crates/paged-cli/README.md:59-62` and `crates/paged-cli/src/options.rs:21-24` say a font
  registered after load changes nothing. `RegisterFont` now also reaches the live model,
  which lays out the affected stories again
  (`crates/paged-canvas-wasm/src/dispatch.rs:571-583`,
  `crates/paged-canvas/src/model.rs:9312-9327`).
- `AssetResolver::resolve_icc` is declared, and nothing in the pipeline calls it. The
  Promise wrapper the trait doc places in `paged-sdk` does not exist there; the viewer
  session takes fonts through `register_font` before `load`.

## Related

- [ADR 003](003-lcms2-color.md), [ADR 106](106-colour-resolved-at-build-time.md) — the colour engine the profile bytes feed, and where the profile is applied
- [ADR 007](007-carry-through-rendering-honesty.md), [ADR 113](113-one-typed-door.md) — reporting a degradation instead of hiding it; the dispatcher that holds the registries
