# ADR 112 — The read-only viewer SDK is a sibling of the editor wasm, enforced by a dependency audit

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-sdk`, `web/idml-viewer`, the subset audit in `.github/workflows/publish-wasm.yml`, and the optional features in `paged-renderer` and `paged-wire` that keep the boundary

## Context

The repository builds two browser engines. `paged-canvas-wasm` is the editor's: it links
`paged-canvas` and `paged-script`, and through them the mutation engine. `paged-sdk` shows
a document and nothing more.

`crates/paged-sdk/WEBGPU.md:36-46` states the rule for the second one: it links "**nothing
from the editor** (no `paged-mutate`, `paged-canvas`, `paged-script`)". It names the
boundary "sibling, not a shrunk app" and calls keeping it "the whole point". The same file
gives the rendering rule: all forward rendering is WebGPU, and when `navigator.gpu` is
absent the consumer shows a message, because "the SDK does not carry a second renderer"
(`crates/paged-sdk/WEBGPU.md:50-53`). Why the viewer must not be a subset of the editor is
not argued there. The repository does not record why.

A manifest does not show such a boundary holding. Twice a dependency added elsewhere
reached `paged-sdk` transitively, and both times the release was stopped by a check on the
resolved tree: at v0.35.1 through `paged-renderer` (commit `ce77513`), at v0.63.0 through
`paged-wire` (commit `08fb4b1`).

## Decision

`paged-sdk` is built from its own dependency graph, not by removing parts from the editor
wasm, and the publish workflow fails when an editor crate appears in that graph.

- `paged-sdk` depends on `paged-renderer` with `default-features = false`, on
  `paged-compose`, on `paged-gpu` (optional, behind the `gpu` feature) and on the IDML
  importer. `default-features = false` leaves out the CPU rasteriser.
- Its wasm surface is one session class, `ViewerSession`, and the values it returns:
  register a font, load, page layout, present to a canvas, read pixels back. It has no
  method that changes a document. `ViewerSession::new` rejects when `navigator.gpu` is
  absent.
- Load and layout problems are returned as a `Diagnostics` value, not thrown.
- The viewer loads through `viewer_build`, a target-independent function, and a native test
  compares each page's display-list digest with a direct `pipeline::build_document`.
- The audit runs `cargo tree -p paged-sdk --target wasm32-unknown-unknown --features gpu
  -e normal` and fails on a match for `paged-(mutate|canvas|script)`.
- `@paged-media/idml-viewer` (`web/idml-viewer`) is TypeScript over that session. It
  programs against a hand-written interface, `ViewerSessionLike`, so that a test can pass
  a fake and an embedder can start the wasm itself. Camera arithmetic, input handling and
  events are TypeScript on the main thread; each frame calls
  `session.present(zoom, x, y, dpr, page)`.

## Evidence

- `crates/paged-sdk/src/lib.rs:15-27` — crate docs: nothing from the editor, WebGPU-only
- `crates/paged-sdk/Cargo.toml:16-32` — the `gpu` feature and the dependency list
- `.github/workflows/publish-wasm.yml:261-271` — "Subset audit — paged-sdk must not link editor crates"
- `crates/paged-renderer/Cargo.toml:13-19`, `:41-44` — `mutate-roundtrip` is optional, and `paged-wire` is taken without default features, both for this boundary
- `crates/paged-sdk/src/build.rs:15-38`, `crates/paged-sdk/tests/digest_equivalence.rs:15-23` — the single load path and its equivalence test
- `web/idml-viewer/src/session.ts:15-22`, `:59-83` — `ViewerSessionLike` and why it exists
- `web/idml-viewer/src/bootstrap.ts:17-40` — `createSessionFromBundledWasm` loads `../wasm/paged_sdk.js`, placed by the publish workflow
- `web/idml-viewer/src/viewer.ts:209-222`, `web/idml-viewer/src/camera.ts:15-21` — the frame loop and the camera as pure functions

## Alternatives considered

- A viewer made by trimming the editor binary: named and refused in the phrase "sibling,
  not a shrunk app" (`crates/paged-sdk/WEBGPU.md:44-46`).
- A CPU fallback inside the SDK: refused (quoted above). The earlier free functions
  `render_to_png`, `render_pages` and `parse_summary` "are gone"
  (`crates/paged-sdk/WEBGPU.md:9-14`).

## Consequences

Every crate under `paged-renderer` must keep editor crates optional. That is the purpose
of `mutate-roundtrip` there and of the `mutations` feature in `paged-wire`
([ADR 111](111-wire-vocabulary-leaf-crate.md)).

The audit is a step of the publish workflow only. `.github/workflows/ci.yml` does not run
`cargo tree`, so a breach is found when a release is cut, as both were. The TypeScript tests
of `web/idml-viewer` also run only there (`npm test` before the build).

`ViewerSessionLike` repeats the wasm exports by hand. Nothing in `web/idml-viewer` reads the
generated `paged_sdk.d.ts`, so the interface is edited by hand when `ViewerSession` changes.

`crates/paged-sdk/WEBGPU.md:28-32` records, as a known follow-up, that `tiny-skia` is still pulled in
transitively by `resvg`. The same file opens with a comment calling itself "not yet
implemented" and ends with a section about a stub in another repository; both predate the
code. `viewer/build-wasm.sh`, which the crate's manifest, that file and
`web/idml-viewer/src/bootstrap.ts` name, is not in this repository.

## Related

- [ADR 123](123-viewer-ships-from-core.md) — where the viewer package lives and how it is published
- [ADR 111](111-wire-vocabulary-leaf-crate.md) — the feature split that restored this boundary
- [ADR 100](100-two-rasterisers-one-trait.md), [ADR 101](101-display-list-single-intermediate.md) — the WebGPU surface, and the digest the equivalence test compares
