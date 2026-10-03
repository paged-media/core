# ADR 006 — Protocol-coupled package versioning (`0.<protocol>.<patch>`)

**2026-06-07 · decision record · status: ACCEPTED (records active policy).**

**Sources:** [`../reference/protocol-governance.md`](../reference/protocol-governance.md)
(ACTIVE policy, 2026-06-07 — the three
versioning axes table + bump rules + release coupling);
`editor: packages/client/package.json:17` (`"@paged-media/canvas-wasm": "0.34.0"`);
`crates/paged-canvas/src/channel.rs` (`PROTOCOL_VERSION = ProtocolVersion(34)`);
`editor: packages/client/src/protocol.ts:24` (`PROTOCOL_VERSION = 34`);
`editor: scripts/check-protocol-version.sh`;
`.github/workflows/publish-wasm.yml`; an internal release runbook.

## The decision

**The npm minor version of every engine package (`canvas-wasm`,
`introspect-wasm`, `sdk`, `idml-viewer`) *is* the wire protocol number:
`0.<protocol>.<patch>`, tag-derived from `v0.<p>.<n>`.** Today: protocol 34 →
package `0.34.0`. This is recorded over plain semver deliberately.

## Why this beats semver-decoupled versioning

[`../reference/protocol-governance.md`](../reference/protocol-governance.md) makes
the case; this ADR records the *why* the coupling
was chosen over a conventional independent semver:

- **A package↔protocol mismatch is diagnosable at a glance.** The editor pins
  `canvas-wasm: 0.34.0` and asserts `PROTOCOL_VERSION === 34`. If the minor and
  the runtime handshake disagree, the failure names the exact version skew. With
  decoupled semver, "package 2.7.1 speaks protocol ?" requires a lookup table;
  here the version *is* the answer.
- **The bump rule is mechanical.** A new message kind / Operation / Mutation
  variant always bumps the protocol → always bumps the minor → always a new tag
  → `publish-wasm.yml` ships all four packages in lockstep. Additive changes
  (`#[serde(default)]` fields, new read-only collections, new `PropertyPath`
  variants per [ADR 005](005-wire-recipe.md)) never bump, so the version moves
  exactly when the wire shape moves incompatibly. One tag versions the whole
  lockstep.
- **CI enforces the equality.** `check-protocol-version.sh` asserts package minor
  == `protocol.ts`; the worker handshake hard-asserts equality at runtime
  (`protocolMismatch`). The version number is load-bearing, not cosmetic.

## Consequences

- **Lockstep, no compat window — and that is correct *while* consumers are
  first-party and same-repo-day synced.** The worker asserts equality, not a
  range. The day a published consumer can lag (docs embeds, external SDK users),
  equality must become a negotiated range; that range/capability-flag design is a
  named prerequisite for any 1.x engine-package promise (the published-consumer
  gate in [`../reference/protocol-governance.md`](../reference/protocol-governance.md)),
  not something to bolt on ad hoc.
- The plugin API (`@paged-media/plugin-api`) versions on a **separate** semver
  axis: a protocol bump does not imply a plugin-api bump and vice-versa. Wire
  types reach plugins via a vendored `wire.d.ts` re-sync, not via the protocol
  minor. Three axes, each owning its own thing.
- The `0.` major is held until the compat-window design lands; pre-1.0 the
  `latest` dist-tag must not move for canaries (guarded in
  `plugin-sdk: .github/workflows/publish.yml`).

## Amendment — 2026-10-02

Checked against the code at `9f933f1`, with the editor at `28dc764` and plugin-sdk at
`d90f727`. The decision stands: the protocol is 64
(`crates/paged-canvas/src/channel.rs:510`, pinned by the test at `:3765-3767`), the editor
pins `@paged-media/canvas-wasm` `0.64.0` (`editor: packages/client/package.json:54`), and the
equality is still exact, with no range. The text above is silent on how a release is cut
(point 1) and differs from the code in points 2 and 3. Bullet numbers below refer to the
section "Why this beats semver-decoupled versioning".

**1. How a release is cut.** One workflow, triggered by a tag, publishes the four packages.
The procedure is described in [`../reference/release-flow.md`](../reference/release-flow.md).

- `.github/workflows/publish-wasm.yml:38-41` — runs on a pushed tag `v*` or a manual dispatch.
- `.github/workflows/publish-wasm.yml:158-170` — the package version is the tag without its
  `v`. A run on a ref that is not a tag, or on a tag that is not
  `vMAJOR.MINOR.PATCH[-prerelease]`, stops here.
- `.github/workflows/publish-wasm.yml:177-188` — a version with a `-suffix` is published under
  the npm dist-tag `next`; any other under `latest`.
- `.github/workflows/publish-wasm.yml:217`, `:237`, `:273`, `:298` — the four build steps
  (`canvas-wasm`, `introspect-wasm`, `sdk`, `idml-viewer`); `:339-342` — four `npm publish`
  calls in sequence. A failure part-way leaves the earlier packages published (`:328-332`).
- The workflow does not read `PROTOCOL_VERSION`. That the minor equals the protocol follows
  from the tag that is pushed, not from a check at publish time.
- `.github/workflows/ci.yml:283-332` — `protocol-tag-guard`: a push to `main` that changes
  `PROTOCOL_VERSION` fails unless a tag `v0.<protocol>.*` exists. `:247-272` is the older
  check, which only warns.

**2. The editor no longer holds a second number.** "CI enforces the equality" describes a
script comparing the package minor with a constant in `protocol.ts`. Since 2026-10-01 the
editor derives the constant from the installed package:

- `editor: packages/client/src/protocol.ts:33-44` — `PROTOCOL_VERSION` is computed from the
  version of the installed `@paged-media/canvas-wasm` (`protocolFromVersion`, `:47-53`).
- `editor: .github/workflows/protocol-version.yml:10-13`, `:46-48` — the workflow states that no
  agreement step is left. `scripts/check-protocol-version.sh` no longer exists in the editor.
- The runtime check remains. The wasm exposes its compiled-in protocol
  (`crates/paged-canvas-wasm/src/lib.rs:209-214`); the editor's worker compares it with the
  derived number and reports `protocolMismatch` (`editor: apps/canvas/src/worker/worker.ts:277-287`),
  and the client then rejects every request (`editor: packages/client/src/client.ts:1522-1527`).

This supersedes, in the third bullet, "CI enforces the equality" and the
`check-protocol-version.sh` clause; in the first bullet, "asserts `PROTOCOL_VERSION === 34`";
and the `protocol.ts:24` and `check-protocol-version.sh` entries in **Sources**.

**3. The bump rule in practice.** The second bullet says additive changes "never bump, so the
version moves exactly when the wire shape moves incompatibly". The version history kept as
comments above the constant records two bumps for changes that bullet lists as additive:

- `crates/paged-canvas/src/channel.rs:465-469` — protocol 62 added one `PropertyPath` variant
  behind an existing message kind: "No new message: the vocabulary grew, not the wire shape,
  which is why this is a bump and not a redesign."
- `crates/paged-canvas/src/channel.rs:491-502`, `:1174-1175` — protocol 63 added an optional
  field with a serde default; the note calls the change "Additive".

Other additive fields still ride without a bump and say so (`crates/paged-canvas/src/channel.rs:555-557`,
`:3290-3294`). No comment in the repository states a revised rule.

**4. Dist-tags.** The last Consequences bullet is about plugin-sdk's canary releases
(`plugin-sdk: .github/workflows/publish.yml:9`, `:90`). For the four engine packages the rule
is the one in point 1: `latest` for a release tag, `next` for a prerelease tag.
