# ADR 007 — Rendering & save-back honesty: parse-don't-fake, carry-through

**2026-06-07 · decision record · status: ACCEPTED (records the engine's honesty
posture).**

**Sources:** `crates/paged-write/src/lib.rs:15-40` (carry-through save-back:
"copies the original package verbatim and patches only what the model can
faithfully express"); `crates/paged-renderer/src/diagnostics.rs:15-40`
(`RenderDiagnostics` on `BuiltDocument`; `Severity { Info, Warning, Error }`;
"correct-but-lossy" outcomes reported, not silently warned);
the internal renderer gap list (the engineering mirror of the public
`<SupportBadge>`s; "every `not-yet-parsed`/`parsed-not-rendered` row should have
a matching badge"); `docs: content/docs/edge-cases/graceful-degradation.mdx`
(`<SupportBadge>` in the public docs).

## The decision

**The renderer renders what the model holds and nothing more, and the save-back
writer patches only what the model can faithfully express — unsupported or lossy
constructs get structured diagnostics and honest support badges, never a silent
approximation.** Two faces of one principle: *parse-don't-fake on the way out to
pixels, carry-through on the way back to IDML.*

## The two mechanisms

- **Render side — diagnostics, not silent degradation.** Several render outcomes
  are correct-but-lossy: text overflowing the last frame in a chain is clipped
  to match InDesign's PDF; a missing image link renders a grey placeholder;
  page numbering falls back to computed section rules. These were historically
  `tracing::warn!`-only and invisible to callers. `RenderDiagnostics` now rides
  on `BuiltDocument` with a stable machine-matchable code + severity, so
  paged-inspect and the editor surface them. The renderer does not invent a
  plausible-looking result for a construct it does not handle.
- **Save-back side — carry-through, not regenerate.** `paged-write` does NOT
  regenerate the IDML package from the model (the parser keeps only a *subset*
  of each entry's attributes; most entries — fonts, preferences, tags, the XML
  backing store — are not modeled at all). Regenerating would silently drop
  everything the parser didn't read. Instead it copies the source package
  **byte-identical** and patches only the changed Spreads/Stories — the model's
  faithfully-expressible delta. Round-trip stays honest because un-modeled data
  is preserved, not re-synthesized.

## Why (reconstructed rationale)

The carry-through strategy is recorded only in the `paged-write` crate doc-
comment; the principle is **reconstructed** as: a *faithful* renderer's worst
failure mode is the convincing wrong answer. A grey placeholder you can see beats
a fabricated image; a diagnostic beats a silent clip; a byte-preserved unknown
entry beats a lossy regeneration. Honesty is a fidelity feature, not a fallback.

## Consequences

- **The internal renderer gap list ↔ `<SupportBadge>` must stay in sync** — every
  internal gap row has a public badge and vice-versa; the two are the same truth
  at two altitudes. This sync *is* the honesty contract surfaced to users.
- Save-back is the foundation of conformance **level 4 (round-trips)** —
  carry-through is what makes round-trip conformance-verifiable rather than
  best-effort.
- The cost: the model is intentionally a subset, so save-back cannot *add*
  un-modeled constructs (only patch what's modeled). Accepted — the alternative
  (model everything to regenerate) trades the silent-drop risk for a far larger
  surface and is not worth it for a renderer-first engine.

## Amendment — 2026-10-02

Checked against the code at `9f933f1`, with plugin-publish at `6994ad1` and docs at `2094ba4`.
The posture stands on both sides. The render-side mechanism is where the ADR says it is:
`RenderDiagnostics` is carried on `BuiltDocument`
(`crates/paged-renderer/src/diagnostics.rs:14-25`, `:185`;
`crates/paged-renderer/src/pipeline/mod.rs:914-928`). On the save side the text above no
longer matches the code in four places.

**1. The IDML save-back writer is no longer in this repository.** `crates/paged-write` does not exist.
The writer is the crate `idml-export` in plugin-publish, and this repository takes it as a git
dependency pinned to a revision.

- `plugin-publish: crates/idml-export/src/lib.rs:15-72` — the crate comment the ADR cites,
  with the same strategy: the writer "copies the original package verbatim and **patches only
  what the model can faithfully express**" (`:26-27`).
- `crates/paged-canvas/Cargo.toml:35` — the dependency; `Cargo.toml:71-82` — why the
  workspace patches the adapter's own dependencies back onto the local model crates.
- `crates/paged-canvas/src/model.rs:4371-4381`, `:4398-4408` — the canvas model calls
  `idml_export::write_idml_with` with the retained source bytes.
- The IDML reader moved the same way (`idml-import`, `crates/paged-canvas/Cargo.toml:22`).

The move is recorded in
[ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md),
the mutual pin in
[ADR 650](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/650-mutual-git-revision-pins.md),
and the writer's strategy in
[ADR 652](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/652-idml-save-back-patches.md).
This supersedes the `crates/paged-write/src/lib.rs:15-40` entry in **Sources** and the crate
name `paged-write` wherever it appears above.

**2. The writer patches more than Spreads and Stories.** "patches only the changed
Spreads/Stories" described the first version. The crate comment now lists:

- `plugin-publish: crates/idml-export/src/lib.rs:38-47` — `MasterSpreads/*.xml` is rewritten
  as well; an entry whose rewrite equals its source is still copied verbatim.
- `plugin-publish: crates/idml-export/src/lib.rs:63-67` — page-item inserts and removals within
  a spread, and new swatches, gradients and styles injected into the `Resources` entries.
- `plugin-publish: crates/idml-export/src/lib.rs:67-71` — a story or a spread created after
  load is written as a new part and referenced from `designmap.xml`.

Every other entry is still copied with its original compressed bytes (`:29-33`). The last
Consequences bullet still holds: the writer emits what the model holds and carries the rest
through.

**3. An IDML export reports what it could not carry.** The ADR gives the save side one
mechanism, carry-through. A second one was added: the loss list.

- `crates/paged-canvas/src/export_losses.rs:14-38` — the document is exported, the bytes are
  parsed again, and the result is compared with the live scene; each construct the re-parsed
  document lacks becomes one line naming the element.
- `crates/paged-canvas/src/model.rs:4491-4512` — `idml_export_losses`; the comment gives the
  reason the list is measured and not declared.
- `crates/paged-canvas/src/channel.rs:1790-1802` — the `IdmlExported` reply carries the list
  as `lost`; `crates/paged-canvas-wasm/src/dispatch.rs:1062-1068` fills it on every IDML
  export.

See [ADR 124](124-opacity-masks-native.md) for the construct that IDML has no element for.

**4. `.paged` is a second save target.** The decision speaks only of the way "back to IDML".
`export_paged` writes the IDML parts through the same writer and adds the model itself as a
native part, `paged/core/model/document.pgm`
(`crates/paged-canvas/src/model.rs:4662-4682`, `crates/paged-store/src/lib.rs:36`, `:72`). The
container is described in [ADR 118](118-paged-file-is-a-valid-idml-package.md) and the
direction in [ADR 021](021-paged-native-document-model-idml-as-format.md).

The docs page cited in **Sources** is now at
`docs: content/docs/idml/edge-cases/graceful-degradation.mdx`.
