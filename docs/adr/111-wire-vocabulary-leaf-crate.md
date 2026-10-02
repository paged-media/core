# ADR 111 — The wire vocabulary is one leaf crate, split by feature

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-wire`; its consumers `paged-renderer`, `paged-canvas`, `paged-introspect`, `paged-script`, `paged-cli`

## Context

The types that every surface uses to address the engine were spread over the two heaviest
crates: `PageId` in `paged-renderer`, `ElementId` and the `Mutation` operations in
`paged-canvas`. The crate docs of `paged-wire` give the problem: these types are "the
contract the CLI, the wasm wire, the Boa bridge, the plugin host and the editor all speak",
and the capability catalog could not list the operations, because `paged-introspect`,
"light and published", "must not drag in the canvas model to read a list of names"
(`crates/paged-wire/src/lib.rs:18-25`).

The operation names were written in one place only, the match arms of
`Mutation::discriminant`, which cannot be enumerated. Code that needed the whole list
re-derived it "by scraping this function's match arms out of the source text or by
hand-copying them into another repo" (`crates/paged-wire/src/lib.rs:1659-1664`).

The types moved into `paged-wire` on 2026-09-09 (commit `afdeb69`). One day later the
publish workflow's dependency audit stopped a release: `paged-renderer` needs only `PageId`,
and taking the whole crate had put `paged-mutate` into the tree of the read-only viewer SDK
(`crates/paged-wire/src/lib.rs:41-47`, commit `08fb4b1`).

## Decision

The wire vocabulary lives in one crate, `paged-wire`, and that crate is cut in two by a
default feature.

- **Identities** (always compiled): `PageId`, `ElementId` with `SelectionMode` and
  `ElementSelection`, `TextCellAddr`, `ByteBuf`. They depend on `serde`, `tsify-next` and
  `wasm-bindgen` only.
- **Operations** (feature `mutations`, on by default): the `Mutation` enum and its roster.
  Their payload types come from `paged-mutate`, which is an optional dependency behind the
  feature.
- `paged-renderer` takes the crate with `default-features = false`. The four other
  consumers take the default.
- The operation names are written once, in `mutation_vocabulary!`. The macro generates
  `Mutation::discriminant` and the constant `MUTATION_NAMES`; `wire_tag_of` gives the
  camelCase spelling that serde writes. The roster holds 118 names.
- `ElementId::parse` and `ElementId::to_address` are the one grammar for the `kind:id`
  address strings that scripts and the command line accept.
- The old homes re-export the types, so `paged_renderer::PageId` and
  `paged_canvas::channel::Mutation` still resolve.
- The capability catalog publishes the roster as `operations`.

## Evidence

- `crates/paged-wire/src/lib.rs:15-52` — crate docs: why the crate exists and "Why `mutations` is a feature"
- `crates/paged-wire/Cargo.toml:9-18` — `default = ["mutations"]`, `mutations = ["dep:paged-mutate"]`, the optional dependency
- `crates/paged-renderer/Cargo.toml:41-44` — identities only, `default-features = false`
- `crates/paged-wire/src/lib.rs:1656-1683`, `:1685-1805`, `:1807-1827` — the macro, the roster, `wire_tag` and `wire_tag_of`
- `crates/paged-wire/src/lib.rs:157-218` — `ElementId::parse` and its doc ("It was a private function in the Boa bridge")
- `crates/paged-canvas/src/channel.rs:3356`, `crates/paged-canvas/src/element_selection.rs:34`, `crates/paged-renderer/src/pipeline/mod.rs:660-664` — the re-exports
- `crates/paged-introspect/src/catalog.rs:92-102`, `:194-201` — `ApiCatalog.operations`, built from `MUTATION_NAMES`
- `crates/paged-canvas/tests/wire_vocabulary.rs:56-73` — the roster is compared with the variant list in serde's own error message

## Alternatives considered

- The earlier layout: operations inside `paged-canvas`, `PageId` inside `paged-renderer`,
  the list of names re-derived by each consumer. Replaced for the reasons quoted above.
- One crate without a feature: in the tree for one day. It linked the mutation engine into
  the viewer SDK and was stopped by the audit described in
  [ADR 112](112-viewer-sdk-is-a-sibling.md).

## Consequences

Adding an operation means a variant in `Mutation` and a line in `mutation_vocabulary!`. The
macro's `discriminant` match has no wildcard arm, so a variant missing from the list does
not compile; a test checks the list against what serde accepts.

A workspace build never compiles the identities-only half, because another member turns
the feature on. `CLAUDE.md:106-108` gives the check: `cargo check -p paged-renderer` alone.

`ElementId::Table` and `ElementId::TableCell` have no address string; `parse` never returns
them and `to_address` returns `None` for them (`crates/paged-wire/src/lib.rs:175-177`).

Three comments are behind the code. The crate docs, `CLAUDE.md:98` and the test's floor
(`>= 117`) say 117 operations; the roster and `crates/paged-introspect/catalog.json` hold
118. The doc comment on `Mutation` still says "the worker rejects each variant with
`WorkerError::NotImplemented`" (`crates/paged-wire/src/lib.rs:610-611`). And the crate docs
call it a crate "that depends on the model and the mutation vocabulary and nothing else";
its manifest names `paged-mutate` (optional) and no model crate.

## Related

- [ADR 005](005-wire-recipe.md), [ADR 019](019-capability-catalog-one-contract.md) — one closed vocabulary, one generated catalog
- [ADR 112](112-viewer-sdk-is-a-sibling.md), [ADR 116](116-mutations-lowered-onto-operations.md) — the boundary the feature split protects; how `Mutation` is lowered onto `paged-mutate` operations
- [ADR 302](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/302-vendored-wire-types.md) — the plugin contract vendors the TypeScript generated from these types
