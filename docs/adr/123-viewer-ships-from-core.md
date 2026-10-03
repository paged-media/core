# ADR 123 — The viewer ships from core

- **Status:** Accepted 2026-06-07. Recorded here on 2026-10-02, from the code at `9f933f1`.
- **Scope:** `web/idml-viewer`, `crates/paged-sdk`, `.github/workflows/publish-wasm.yml`

## Context

The viewer is a browser component that shows an IDML document without any editing surface. It has
two parts. `crates/paged-sdk` is the Rust side: a WebGPU session (`ViewerSession`) over the
renderer, published as `@paged-media/sdk` ([ADR 112](112-viewer-sdk-is-a-sibling.md)). The camera,
page navigation, input handling and events are TypeScript written against that session's typed
contract.

The SDK crate's design note was written with a different home for the TypeScript part in mind. It
names a separate `paged-media/viewer` repository as the wrapper of the SDK
(`crates/paged-sdk/WEBGPU.md:40-42`), built in the interim "from a sibling checkout" by
`viewer/build-wasm.sh`, with a typed stub standing in for the session until the crate shipped
(`crates/paged-sdk/WEBGPU.md:132-142`).

The wrapper was then added to this repository in commit `64da127` (2026-06-06), together with its
publish steps. The repository does not record why.

## Decision

The viewer package `@paged-media/idml-viewer` lives in `web/idml-viewer` in this repository and is
built, tested, versioned and published by the same tag-triggered workflow as the engine's three
wasm packages.

- The workflow builds the SDK wasm, then in `web/idml-viewer` runs `npm install`, `npm test` and
  `npm run build`, copies `paged_sdk.js`, `paged_sdk_bg.wasm` and `paged_sdk.d.ts` from the SDK
  output of the same run into `wasm/`, and sets the package version from the tag.
- The published package carries its own copy of the wasm: `files` lists `dist`, `wasm` and
  `README.md`, and the manifest declares no runtime dependency.
  `createSessionFromBundledWasm` loads `../wasm/paged_sdk.js` relative to the built module.
- `wasm/` and `dist/` are ignored by git. In a source checkout there is no wasm to load, and an
  embedder passes its own session object, which `createViewer` accepts through the
  `ViewerSessionLike` interface.
- The package's API catalogue, `web/idml-viewer/api-catalog.json`, sits beside the source, and a
  test fails when an export of `src/index.ts` is missing from it.

## Evidence

- `web/idml-viewer/package.json:2-10`, `:20-24`, `:30-33` — the package name, its repository
  directory, the published files, and development dependencies only
- `.github/workflows/publish-wasm.yml:298-307`, `:339-342` — the viewer build step; four packages
  published in sequence
- `.github/workflows/publish-wasm.yml:158-170` — the version comes from the tag
- `web/idml-viewer/src/bootstrap.ts:17-29`, `.gitignore:37-41` — the bundled wasm is placed by the
  workflow and absent in a checkout
- `web/idml-viewer/src/session.ts:15-22`, `web/idml-viewer/src/viewer.ts:70-74` — the session
  contract and the injectable session
- `web/idml-viewer/test/api-catalog.test.ts:15-19`, `:49-53` — the catalogue check
- `docs: package.json:29`, `docs: sources.pin:21-27` — the documentation site depends on the
  published package and reads the catalogue from this directory

## Alternatives considered

A separate viewer repository that wraps the SDK and builds it from a sibling checkout: the plan in
`crates/paged-sdk/WEBGPU.md` cited above. This repository contains no `viewer/` directory and no
`build-wasm.sh`.

## Consequences

The wrapper and the wasm inside it come from one commit and carry one version, the same as the
other three packages ([ADR 006](006-protocol-coupled-versioning.md)). The version in the committed
manifest (`0.1.0-canary.0`) is a placeholder that the workflow overwrites.

The viewer step runs before the publish step, so a failing viewer test stops the release of all
four packages. The packages are then published one after another; a failure part-way leaves the
earlier ones published (`.github/workflows/publish-wasm.yml:328-332`).

The viewer's tests run only in that workflow. No other workflow, the `Makefile` or `scripts/`
mentions `web/idml-viewer`, so a pull request that breaks the wrapper is not caught before a tag is
pushed. The tests drive a fake session (`web/idml-viewer/test/fake-session.ts`), not the wasm.
`package-lock.json` is ignored and the workflow runs `npm install`, so the development
dependencies are resolved anew at each release.

Other repositories depend on this location: the documentation site reads the catalogue at
`web/idml-viewer/api-catalog.json` on `main`, and the workflow comment notes that npm's trusted
publishing would be keyed on the workflow's file name (`.github/workflows/publish-wasm.yml:27-29`).

Comments still describe the earlier plan: `crates/paged-sdk/Cargo.toml:20` and
`web/idml-viewer/src/bootstrap.ts:20-21` refer to `viewer/build-wasm.sh`, and
`crates/paged-sdk/WEBGPU.md:136-142` to a stub in `viewer/src/session.ts`. Neither path exists here.

## Related

- [ADR 112](112-viewer-sdk-is-a-sibling.md) — the SDK's dependency boundary and the audit that enforces it
- [ADR 006](006-protocol-coupled-versioning.md) — the version scheme the four packages share
- [ADR 803](https://github.com/paged-media/docs/blob/main/docs/adr/803-live-preview-uses-viewer-package.md) — the documentation site as a consumer of the published package
