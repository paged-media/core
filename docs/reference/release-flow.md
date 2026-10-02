# Release flow — a core engine release, end to end

**2026-06-07 · process record · sources:** `.github/workflows/publish-wasm.yml`;
`crates/paged-canvas/src/channel.rs` (`PROTOCOL_VERSION = ProtocolVersion(35)` on that day);
[protocol-governance.md](protocol-governance.md); an internal runbook for the package-boundary
migration; [ADR 006](../adr/006-protocol-coupled-versioning.md);
editor `1d38f05`, plugin-sdk `b78f430`, core tag `v0.35.0` (`9e78005`). Worked example: today's v35.

The wire protocol number IS the package minor (`0.<protocol>.<patch>`, ADR 006). A
release moves four npm packages in lockstep from one core tag.

## The seven steps

*Status note (2026-10-02): the steps were checked against the workflows of this repository at
`9f933f1`, of `editor` at `28dc764` and of `plugin-sdk` at `d90f727`. Steps 1 and 3 match. The
others differ as follows; the step text below is unchanged.*

- *Step 2: the workflow also accepts a manual dispatch, but its version step fails for any ref
  that is not a tag of the form `vMAJOR.MINOR.PATCH[-prerelease]`
  (`.github/workflows/publish-wasm.yml:158`).*
- *Step 4: a run without `NPM_TOKEN` is no longer a no-op. A preflight step fails the run before
  any build when the token is absent or does not authenticate
  (`.github/workflows/publish-wasm.yml:61`).*
- *Step 5: the editor now reads `PROTOCOL_VERSION` from the version of the installed
  `@paged-media/canvas-wasm` package (`editor: packages/client/src/protocol.ts`), so a bump on that
  side is the pin move and the lockfile. `check-protocol-version.sh` is retired;
  `editor: .github/workflows/protocol-version.yml` now runs only the plugin-pin check.*
- *Step 6: the same workflow also runs `sync-catalog.mjs --check` against the published
  `@paged-media/introspect-wasm` of the same version
  (`plugin-sdk: .github/workflows/publish.yml:69`).*
- *Step 7: `plugin-sdk: .github/workflows/publish.yml` publishes through npm trusted publishing
  under `--tag canary` and contains neither an `npm dist-tag add … latest` step nor a
  `template-canary-smoke` job. `plugin-sdk: scripts/assert-dist-tags.mjs` states the policy as
  canary-only with `latest` intentionally absent before 1.0, and no workflow there runs it.*

1. **Riders accumulate on main.** Between a protocol bump and its first consumer
   sync, additive variants *ride* the same number with a `// rides vNN` comment
   ([protocol-governance.md](protocol-governance.md), bump rule 4). The bump itself is one owner,
   one commit: the `const` + the `protocol_version_is_vNN` pin test + the `(protocol vNN)` comments.

2. **Tag `v0.<protocol>.<patch>`** on the commit to publish, pushed to `paged-media/core`.
   `publish-wasm.yml` triggers on `tags: ["v*"]`. The npm version is derived from the
   tag (`${GITHUB_REF_NAME#v}`).

3. **`publish-wasm.yml` builds four packages** off that one tag: `@paged-media/canvas-wasm`
   (`-p paged-canvas-wasm --features gpu`), `introspect-wasm`, `sdk` (`paged-sdk` WebGPU
   ViewerSession), and `idml-viewer` (TS wrapper + bundled wasm). Each is `wasm-bindgen
   --target web` then `wasm-opt -Oz`. Two gates run inline: the toolchain is pinned from
   `scripts/rust-channel.sh` (not `@stable`), binaryen is pinned to 126 (apt's 116 miscompiles
   reftypes), and a **subset audit** (`cargo tree -p paged-sdk
   … | grep -E "paged-(mutate|canvas|script)"`) FAILS the publish if an editor crate
   enters the viewer graph.

4. **Dist-tag rule** (`disttag` step; governance, "Release coupling"): a version with a `-suffix`
   (`v0.N.0-rc.x`) publishes under `next`; a clean `0.N.P` under `latest` (npm default).
   The publish loop passes `--tag "$TAG"` uniformly. The publish step is a no-op when
   `NPM_TOKEN` is absent.

5. **Editor pin bump** — ONE commit (today: `1d38f05`). Three `package.json` pins
   (`packages/client`, `apps/canvas`, `apps/devtools`) → `0.<protocol>.0`,
   `packages/client/src/protocol.ts PROTOCOL_VERSION`, regenerated `pnpm-lock.yaml`.
   CI `protocol-version.yml` runs `check-protocol-version.sh` (installed canvas-wasm
   MINOR must equal `protocol.ts`); the worker handshake catches runtime drift
   (`protocolMismatch`). The **plugin-api-compat alarm** (`editor: apps/canvas/src/plugin-api-compat.ts`)
   is a typecheck-only assertion that the editor's real shapes still satisfy the published
   `@paged-media/plugin-api` contract — it trips the EDITOR's build, never a plugin author's.

6. **plugin-sdk sync-wire re-vendor** (today: `b78f430`). `plugin-sdk: scripts/sync-wire.mjs` copies
   the published `canvas-wasm` `.d.ts` into `plugin-sdk: packages/plugin-api/src/wire.d.ts` and stamps
   `// Synced from @paged-media/canvas-wasm@<version>`. CI `publish.yml` runs
   `sync-wire.mjs --check` as a HARD gate (content drift OR stale stamp → fail). The
   headless loader (`plugin-sdk: packages/plugin-sdk/src/wasm-loader.ts`) derives its expected
   protocol from that stamp's minor and throws on boot if the wasm disagrees.

7. **Plugin canaries.** plugin-sdk `publish.yml` fires on every merge to main, publishes
   only versions not yet on the registry, under `--tag canary`, then `npm dist-tag add …
   latest` (pre-1.0 npm forbids deleting `latest`, E400 — so `latest ≡ canary` lockstep,
   not absence; `assert-dist-tags.mjs` enforces it). `template-canary-smoke` re-runs the
   published canary against plugin-template (skips with a warning if `TEMPLATE_RO_TOKEN`
   is unset).

## Worked example — v0.35.0 (2026-06-07)

Tag `v0.35.0` → commit `9e78005`. The v35 bump
landed at core `1797050` (anchored paths + paragraph-bounds wire + font styles); ten
riders followed before publish (cell-text addressing `ad8ec22`, tables v2 `438d8f0`,
list defs `44463d5`, ScriptBudgetKind `849a777`, RebuildStats `cdc0517`, groups v2
`f9758e1`, dash arrays, numbering CRUD — see the tag annotation). Editor consumed it at
`1d38f05` ("canvas-wasm/introspect-wasm 0.35.0, PROTOCOL_VERSION 35"); plugin-sdk
re-vendored at `b78f430` ("wire + engine to 0.35.0"), wire.d.ts now stamped
`@paged-media/canvas-wasm@0.35.0`. plugin-api/plugin-sdk sit at `0.2.6-canary.0`,
plugin-cli at `0.1.0-canary.0` (the plugin-API axis is independent of the protocol).
