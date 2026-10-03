# ADR 005 — The wire recipe: every operation self-describing and invertible

**2026-06-07 · decision record · status: ACCEPTED (records the engine's
most-exercised structural pattern).**

**Sources:** `crates/paged-mutate/src/operation.rs` (the `PropertyPath`
enum + `label()` at `:1158`, exhaustiveness rationale at `:165`); `apply.rs`
(per-path application); `invert.rs` (the operation algebra: `SetProperty(new) →
SetProperty(prev)`, `Batch(ops) → Batch(rev.map(invert))`);
`crates/paged-introspect/src/descriptor.rs:36-40` (`PropertyPathJson`, "kept in 1:1
sync with `PropertyPath`; the two `From` impls stay exhaustive, so a new
variant fails to compile here until it is mirrored"); `crates/paged-script/src/lib.rs`
(`parse_property_path` `:874`, `property_path_label` `:1061` — the script
string↔path map); `crates/paged-canvas/src/channel.rs` (`PROTOCOL_VERSION`);
a completeness check in the capability registry, kept outside this repository.

## The decision

**Every wire mutation is defined once as a closed, typed `PropertyPath` (and
Operation/Mutation) variant, and each variant carries five co-located
obligations: apply, invert, a JSON descriptor mirror, a human label, and a
script-side parse — so the wire is self-describing and every op is invertible.**
Adding a property is "add the variant, satisfy the five obligations the compiler
and tests demand, bump the protocol if non-additive." This is the playbook
behind 187 paths landed across v27–v31.

## The five obligations (one variant → five touch-points)

1. **Operation** — a new `PropertyPath` variant on the closed enum
   (`operation.rs`). Closed-enum-over-`Vec<String>` is deliberate: it "preserves
   Rust's exhaustiveness" so omissions fail to compile, not at runtime.
2. **apply** — `apply.rs` mutates the model for the variant.
3. **invert** — `invert.rs` returns the undoing operation. The algebra is
   per-shape, not per-property: `SetProperty(new)→SetProperty(prev)`, batches
   reverse-and-invert. Every op is invertible *by construction*.
4. **descriptor** — `PropertyPathJson` in `crates/paged-introspect/src/descriptor.rs` is the
   **one descriptor source** feeding both the inspector's property pane and the
   wire. Its `From`/`Into` impls are exhaustive, so a new `PropertyPath` variant
   *will not compile* until mirrored here.
5. **script map** — `parse_property_path` (string→path) + `property_path_label`
   (path→string) in `paged-script` give the `paged.*` scripting surface the same
   paths under the same names. One label source (`PropertyPath::label()`) drives
   diagnostics, descriptors, and script names — no second naming map to drift.

## Why (reconstructed rationale)

Recorded only as an *addressing* decision in an internal implementation plan; the
end-to-end recipe was tribal. **Reconstructed** from the code shape: the value is
that introspection and scripting are not separate features but **derived
projections of the same descriptor source** — the original scripting-layer
design's "the inspector, REPL, scripts, undo are the same pipe" thesis, realized
as one enum with exhaustive mirrors.

## The mechanical guard

A completeness check in the capability registry (kept outside this repository)
is the completeness gate: **every** wire op (from the editor's live-probed
capability table), every `paged.*` function, every gesture, and every panel must
map to a registry row, or the check fails. The compiler enforces the five
touch-points per variant; the completeness check enforces that the registry
knows about every shipped op. Together they make "self-describing + invertible"
a verified invariant, not a convention.

## Consequences

- Additive variants (new `PropertyPath`, `#[serde(default)]` fields) never bump
  the protocol; new message/Operation/Mutation kinds always do (see
  [ADR 006](006-protocol-coupled-versioning.md)).
- The cost is real: five touch-points per property. The payoff is that undo,
  introspection, and scripting come *for free* with each one, and an incomplete
  landing fails to compile or trips the completeness check.

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The core of the decision stands: `PropertyPath` is one
closed enum with the same stated reason (`crates/paged-mutate/src/operation.rs:173-182`),
paths are applied in `paged-mutate`'s apply layer, and the inverse of a `SetProperty` is a
`SetProperty` carrying the previous value (`crates/paged-mutate/src/invert.rs:15-39`). The
recipe has changed shape: obligations 4 and 5 no longer exist as written. The descriptor
mirror is gone, and the two script-side maps are one table.

**1. The descriptor mirror was deleted (obligation 4).** `PropertyPathJson` and its two `From`
impls are gone; a descriptor carries `PropertyPath` directly.

- `crates/paged-introspect/src/descriptor.rs:27-43` — the `path` field is a `PropertyPath`. Its
  comment records the reason: the mirror and the enum "serialised to byte-identical JSON", and
  "What the mirror bought was the obligation to edit two enums for one capability."
- The name `PropertyPathJson` occurs nowhere else in the repository.

This supersedes obligation 4, "a JSON descriptor mirror" in the Decision, "one enum with
exhaustive mirrors" in the rationale, and the `descriptor.rs:36-40` entry in **Sources**.

**2. The two script-side maps became one table (obligation 5).** The names are written once,
in a macro in `paged-introspect`, and the script bridge reads them from there.

- `crates/paged-introspect/src/catalog.rs:476-547` — `property_paths!` generates the name table
  (`PROPERTY_PATHS`), the list of all paths, and `wire_name`, whose `match` has no wildcard arm
  (`:521-526`), so a new `PropertyPath` variant does not compile until it is given a name. The
  comment at `:479-491` records why: the two earlier lists agreed "by hand, with nothing
  checking it".
- `crates/paged-introspect/src/catalog.rs:549`, `:812` — each variant is placed in `advertised`
  (published as `settablePaths` in `crates/paged-introspect/catalog.json`) or in `hidden`,
  which requires a written reason. Tests pin the split at 219 advertised and 15 hidden
  (`:1144`, `:1023`, `:1103-1107`).
- `crates/paged-introspect/src/catalog.rs:150-167` — `lookup_path`, the one name → path lookup.
  Eight paths also answer to their raw wire spelling, published as `pathAliases` (`:106-110`,
  `:169-192`).
- `crates/paged-script/src/lib.rs:3957-3973` — `parse_property_path` and `property_path_label`
  remain as one-line wrappers over `lookup_path` and `wire_name`.
- `crates/paged-mutate/src/operation.rs:1364` — `PropertyPath::label()` still exists and
  returns dotted names (`"frame.bounds"`). Nothing in the repository calls it.

This supersedes obligation 5, including "One label source (`PropertyPath::label()`) drives
diagnostics, descriptors, and script names", and the `:874` / `:1061` references in
**Sources**. The catalog that the table feeds is the subject of
[ADR 019](019-capability-catalog-one-contract.md).

**3. Other locations in Sources.** `apply.rs` is now the directory
`crates/paged-mutate/src/apply/`; `SetProperty` is applied in
`crates/paged-mutate/src/apply/set_property.rs:28`. `label()` is at `operation.rs:1364` (was
`:1158`) and the closed-enum comment at `:173-178` (was `:165`). The `Mutation` enum is defined
in `crates/paged-wire/src/lib.rs:626` and re-exported from `paged-canvas`
(`crates/paged-canvas/src/channel.rs:3356`); see
[ADR 111](111-wire-vocabulary-leaf-crate.md).

**4. Guards in this repository.** "The mechanical guard" names the compiler and a check kept
outside this repository. Three tests here now guard the published names:

- `crates/paged-introspect/tests/catalog_apply_parity.rs:183-184` — every (element, settable
  path) pair the catalog advertises on a page-item kind is applied to a fixture node and must
  succeed; the list of known failures is empty (`:43-46`).
- `crates/paged-script/tests/advertised_paths_apply.rs:61-62` — every advertised path must be
  named somewhere in the apply layer's source.
- `crates/paged-introspect/src/catalog.rs:1343-1352` — the committed `catalog.json` must equal
  the generated one.

**5. The bump rule.** The first Consequences bullet says a new `PropertyPath` variant never
bumps the protocol. Protocol 62 was a bump for one; see the amendment to
[ADR 006](006-protocol-coupled-versioning.md).
