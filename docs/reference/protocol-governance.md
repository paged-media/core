# Protocol governance — the wire, the packages, the plugin API

Status: ACTIVE policy · 2026-06-07 · born from the v24→v34 sprint
(10 bumps in ~4 days). The rules
below were already practice; this makes them law and names the edges.

The rules were checked against the workflows of this repository at commit
`9f933f1` on 2026-10-02 (and, where a rule names another repository, against
`editor` at `28dc764` and `plugin-sdk` at `d90f727`). Where a rule and a
workflow differ, a status note at the top of the section says so; the rule text
is unchanged.

## The three versioning axes (and what owns what)

*Status note (2026-10-02): the editor no longer keeps a mirrored constant.
`editor: packages/client/src/protocol.ts` reads `PROTOCOL_VERSION` from the
version of the installed `@paged-media/canvas-wasm` package, and
`check-protocol-version.sh` is retired
(`editor: .github/workflows/protocol-version.yml`). The runtime handshake
remains.*

| axis | constant | moves when | consumers |
|---|---|---|---|
| **Engine wire protocol** | `crates/paged-canvas/src/channel.rs PROTOCOL_VERSION` (mirrored as `protocol.ts PROTOCOL_VERSION` in the editor) | a NEW message kind or Operation/Mutation variant ships, or any wire shape changes incompatibly | editor worker handshake; package minors |
| **npm package version** | `0.<protocol>.<patch>` on canvas-wasm / introspect-wasm / sdk / idml-viewer (tag-derived: `v0.<p>.<n>`) | every publish; **minor IS the protocol** | editor pins; `check-protocol-version.sh` asserts minor == protocol.ts |
| **Plugin API** | `@paged-media/plugin-api` semver + `apiVersion` manifest negotiation (`^0.2`) | the CONTRACT changes (types, host surface) — independent of engine protocol; the engine wire reaches plugins via the VENDORED `wire.d.ts` (sync-wire.mjs, synced from the published canvas-wasm package, `--check` hard-fails on drift) | bundles (draw/web/template) |

A protocol bump does NOT imply a plugin-api bump (wire types flow via
the vendored re-sync); a plugin-api bump does NOT imply a protocol
bump. The editor pins both independently.

## Bump rules (engine wire)

1. **Additive never bumps**: new PropertyPath variants, new fields with
   `#[serde(default)]`, new enum members on existing payloads, new
   read-only entries/collections. (187 paths were added across
   v27–v31 with zero consumer breakage this way.)
2. **New message kinds / Operation / Mutation variants ALWAYS bump** —
   exactly once per mergeable unit.
3. **One owner per bump.** The owner updates, in ONE change: the const,
   the pin test (`protocol_version_is_vNN` — rename it), and any
   "(protocol vNN)" comments. A bump landing with a stale pin test is
   the owner's bug (it happened at v30; the test caught it — that is
   the test's job, keep it).
4. **Riders**: between a bump landing and its first consumer sync,
   additional variants MAY ride the same number with an explicit
   comment `// rides vNN (added before first consumer sync)`.
   Precedents: InsertTextFrame (v28), ExportIdml (v29). After the
   first sync/publish of that number, riders are over — next change
   bumps.
5. **Concurrent lines**: announce intent-to-bump in the commit that
   starts the work; if two lines collide, the FIRST to land keeps the
   number, the second renumbers (constants are cheap, history isn't).

## Release coupling

*Workflow comments in this repository cite this section as
"protocol-governance §4" (`.github/workflows/ci.yml:242`,
`.github/workflows/publish-wasm.yml:172`).*

*Status note (2026-10-02): the tag-on-bump warning is the job `tag-on-bump` in
`.github/workflows/ci.yml:247`; `publish-wasm.yml` carries no such check. Since
2026-08 the same file also carries the hard gate `protocol-tag-guard`
(`.github/workflows/ci.yml:283`; see the breach record below). The dist-tag
rule is the step at `.github/workflows/publish-wasm.yml:177`. On the editor
side a bump is now the pin move alone (see the note under the axes table). In
plugin-sdk, `publish.yml` publishes under `--tag canary`
(`plugin-sdk: .github/workflows/publish.yml:90`) and has no step that guards
`latest`; the dist-tag policy is written down in
`plugin-sdk: scripts/assert-dist-tags.mjs`, which no workflow there runs.*

- Tag `v0.<protocol>.<patch>` on the commit to publish → the
  publish-wasm workflow ships all four packages. **Tag-on-bump**: a
  protocol bump on main without a matching tag is an unpublished
  protocol — CI warns (nudge check in publish-wasm/ci).
- Editor consumes a bump as ONE commit: pin bumps (3 package.json) +
  `PROTOCOL_VERSION` + regenerated lockfile. The drift check
  (`check-protocol-version.sh`, CI) and the runtime handshake
  (`protocolMismatch`) both guard.
- **Prerelease tags** (`v0.N.0-rc.x`) require `npm publish --tag` —
  the workflow passes a dist-tag derived from the version (anything
  with a `-suffix` publishes under `next`, never `latest`).
- plugin-sdk publishes canaries per merge (dist-tag `canary`,
  `latest` must NOT move pre-1.0 — guarded in publish.yml).

## Compat posture (current, honest)

Lockstep, no compat window: the worker hard-asserts equality. This is
CORRECT while the only consumers are first-party and same-repo-day
synced. The day a published SDK/viewer consumer can lag (docs embeds,
external users), equality must become a negotiated range — that design
(version ranges, capability flags, or both) is a prerequisite for any
1.x engine-package promise. Tracked as the published-consumer gate;
do not bolt on ad hoc.

## Where the numbers live (checklist for a bump owner)

core: channel.rs const + pin test · editor: protocol.ts + 3 pins +
lockfile · plugin-sdk: wire.d.ts re-sync (sync-wire) when the wire
TYPES changed · the internal feature registry: capability-table
re-capture if ops changed + completeness map · docs: nothing
(pin-based) · tag: `v0.<p>.<n>`.

## Breach record — the 2026 npm-token outage (recorded 2026-08-18)

Tag-on-bump was tested by reality and lost. While the npm publish
token was expired (from ~2026-06-20), NINE protocol bumps merged to
main untagged — protocols 52 through 60 — because the CI check was a
warn-annotation gating nothing, and warnings on green runs scroll by
unread. The line recovered at v0.61.0, which shipped the accumulated
backlog under one tag. Lesson applied 2026-08: core ci.yml now carries
`protocol-tag-guard`, a HARD fail on any main push that CHANGES
`PROTOCOL_VERSION` without its `v0.<P>.*` tag already on the repo (the
warn-only nudge stays for standing drift). The "CI warns" posture in
Release coupling above is thereby promoted, for the bump-carrying push
itself, to a hard gate.
