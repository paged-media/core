# ADR 008 — Read surfaces as first-class wire collections

**2026-06-07 · decision record · status: ACCEPTED (records a shipped read-surface
design).**

**Sources:** `crates/paged-canvas/src/channel.rs` — `FontSummary` (`:1645`),
`LinkSummary` (`:1897`), `StorySummary` (`:1956`), `DocumentStats` (via `model`,
`:790`), `PreflightFinding` (`:247`), the `CollectionName` enum (`:1241`) and its
`as_str`/`from_str` matching the TS `CollectionName` union in
`editor: packages/catalog/src/types.ts` (`:1228`, `:1283`), the `RequestCollection { name }` request
(`:496`); `crates/paged-canvas/tests/w3a0_read_surface.rs` (collections
return live summaries over the wire); the panel-catalog design note
(`documentCollection` / `documentMeta` read kinds;
`editor: packages/shell/src/catalog/use-collection.ts`).

## The decision

**Aggregate document reads — fonts, links, stories, stats, preflight findings,
sections, swatches, etc. — are first-class typed collections served by the engine
over the wire (`RequestCollection { name } → summary[]`), not computed
editor-side by parsing the model.** Each read surface is a typed summary
(`FontSummary`, `LinkSummary`, `StorySummary`, `DocumentStats`,
`PreflightFinding`, …) keyed by a `CollectionName`, with a matching TS union so
the catalog's `documentCollection` / `documentMeta` binding kinds consume them
directly.

## Why (reconstructed rationale)

Recorded only in an internal working log; **reconstructed** from the wire shape:

- **Single source of truth in the engine.** The engine already holds the parsed
  model; it is the only place that *knows* the font list or the broken-link set.
  Re-deriving these editor-side would duplicate parsing logic across the wire
  boundary and let the two drift. A `FontSummary[]` computed once in Rust is
  authoritative; the panel renders it.
- **Collections ride the wire like everything else.** The read surface uses the
  same request/response channel as mutations and the same `CollectionName`↔TS-
  union discipline as the property paths ([ADR 005](005-wire-recipe.md)). New
  collections are additive (`#[serde(default)]` fields like `FontSummary.styles`)
  and ride a protocol number without a forced bump
  ([ADR 006](006-protocol-coupled-versioning.md)).
- **The panel binding ceiling stays tiny.** Because the engine serves whole
  collections, the catalog needs only a `documentCollection` read kind — not a
  query language in the panel layer. A review of the panel catalog confirmed this
  added exactly one read kind and zero write kinds; the binding ceiling held.

## Consequences

- Live mutations reflect in collections immediately (the `w3a0_read_surface`
  tests assert e.g. the `spreads` collection carries a guide after `InsertGuide`
  and drops it on undo) — collections are a *view*, not a cached snapshot.
- The editor never parses IDML; it consumes engine summaries. This keeps the
  editor thin and the parse logic single-homed, at the cost of a wire round-trip
  per collection (acceptable — reads are not on the hot pan/zoom path).
- `PreflightFinding` is the read surface behind PDF-export preflight; it reuses
  the same collection machinery rather than a bespoke export-only channel.

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The decision stands: a collection is requested by
name and answered by the engine from the live model. Two of the named surfaces are not
collections, and the line numbers in **Sources** have moved.

**1. `PreflightFinding` and `DocumentStats` do not travel as collections.** `CollectionName`
has 26 variants and none is for preflight findings or document statistics
(`crates/paged-canvas/src/channel.rs:2172-2224`; the dispatcher
`crates/paged-canvas/src/model.rs:7684-7714` matches all 26).

- `crates/paged-canvas/src/channel.rs:1773-1779` — `PreflightFinding` values are the
  `findings` field of the `PdfExported` reply, which ends a PDF export session; the type is at
  `:652-658`.
- `crates/paged-canvas/src/model.rs:174`, `:210` — `DocumentStats` is a field of the document
  handle returned on load (`crates/paged-canvas/src/channel.rs:1433`) and is also sent as the
  unsolicited `Stats` message (`:1458-1461`).

This supersedes the last Consequences bullet, and in the Decision the inclusion of "stats"
and "preflight findings" among the collections and of `DocumentStats` and `PreflightFinding`
among the summaries "keyed by a `CollectionName`".

**2. One collection is computed by rendering.** The first Consequences bullet calls
collections "a *view*, not a cached snapshot". `InkCoverage` rasterises every page and is
memoised until the next rebuild (`crates/paged-canvas/src/channel.rs:2211-2223`,
`crates/paged-canvas/src/model.rs:7716-7724`); the comment there describes every other
collection as "a cheap walk over parsed state".

**3. Current locations in `crates/paged-canvas/src/channel.rs`.** `RequestCollection { name }`
`:989`; the `CollectionReply` reply `:1636-1649`; `CollectionName` `:2172`, `as_str` `:2230`,
`from_str` `:2264`; `FontSummary` `:2726`; `LinkSummary` `:2978`; `StorySummary` `:3069`.
