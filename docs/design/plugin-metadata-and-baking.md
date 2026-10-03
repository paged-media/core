# Plugin Metadata & Baking — Facility Design

**v0.2 · 2026-06-07 · status: ENGINE LAYER + SDK DOORS IMPLEMENTED**
(§2 carrier, §3 engine surface, the get/set doors of §3, and §4's
contract TYPES landed — core protocol v33 / plugin-api
0.2.5-canary.0. F1 is CI-fixtured in `paged-write` tests; F2 and the
host bake LOOP + badge/expand UX remain. Implementation deltas vs
v0.1: the key namespace is `x-paged:<full manifest id>` (not a
shortname — collision-free for third parties); labels live in a
`Spread::labels` side map (not on the item structs); the envelope
requires `data` to be an object; the engine gate validates prefix/
cap/envelope while PLUGIN-identity enforcement lives in the SDK door
(the engine has no caller identity).)**
**One facility, two consumers:** paged.draw live-shapes and
paged.web's `x-paged-web:*` source. Build it once.

*Status note (2026-10-02): checked against this repository at `9f933f1` and
`plugin-sdk` at `d90f727`. The carrier, the engine gate and the two SDK doors are
built as the header says. Three things differ from the text. The IDML reader and
writer it calls `paged-parse` and `paged-write` have moved to `plugin-publish`
(crates `idml-import` and `idml-export`; see
[ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md)).
The engine gate has gained an optional caller check
(`crates/paged-mutate/src/apply/layer.rs:561`). And the host bake loop of §4 is
still not built: what plugins leave in a document today is recorded in
[ADR 316](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/316-native-content-and-baking.md)
of plugin-sdk.*

## 1. Contract (the fidelity doctrine, made mechanical)

A document object may carry **namespaced plugin metadata** alongside a
**baked IDML form**. Three invariants, each CI-fixtured:

1. **Reopen restores live.** Paged reads the metadata, reconstructs the
   live construct (live shape parameters, web source), and treats the
   baked children as derived output.
2. **Foreign open shows baked.** InDesign (or any IDML consumer) sees
   only valid IDML — the baked form declared per object type
   (`bakedFallback: group | rectangle | raster` — already in the
   manifest schema).
3. **Never silently lossy.** The boundary is visible (badge affordance +
   an "expand" command that bakes permanently); a failed bake blocks the
   save with a diagnostic, never degrades quietly.

## 2. Carrier: IDML `Properties/Label` (KeyValuePair)

IDML's native extension point is the per-object `Label` — a
`KeyValuePair` list (`Key`, `Value` CDATA) that InDesign preserves
verbatim through open/save (it's how EasyCatalog-class plugins persist).
We use it as-is — no schema invention, maximal foreign-tool survival:

```
<Rectangle Self="u123" …>
  <Properties>
    <Label>
      <KeyValuePair Key="x-paged:web" Value="{…json…}"/>
    </Label>
  </Properties>
  …baked children/geometry…
</Rectangle>
```

- **Key convention:** `x-paged:<plugin-shortname>` (one entry per
  plugin per object). The `x-paged:` prefix is reserved; the engine
  rejects writes outside the calling plugin's key (the namespace rule,
  again, at a new chokepoint).
- **Value:** JSON envelope, schema'd:
  `{ v: 1, engine?: {…pins…}, data: {…plugin-defined…} }`.
  `v` is the plugin's metadata version (migrations are plugin-owned);
  `engine` carries determinism pins where relevant (paged.web's
  `blitz@x.y`, `boa@x.y`).
- **Size cap:** 64 KiB per entry, enforced at the mutation layer
  (rejects with a diagnostic; caps keep documents loadable and the
  Label mechanism friendly to other consumers). Assets never inline —
  they go to the document asset store (a separate facility).

## 3. Engine surface (paged-mutate / parse / write)

*Status note (2026-10-02): this section predates the implementation. As built,
`PropertyPath::PluginMetadata` is a unit variant and the key travels in the
value, `Value::PluginMetadata { key, value, caller, prev }`
(`crates/paged-mutate/src/operation.rs:1251`, `:2093`); the wire op is
`SetPluginMetadata` (`crates/paged-wire/src/lib.rs:1160`); the gate is
`apply_plugin_metadata` (`crates/paged-mutate/src/apply/layer.rs:531`); labels
are read by `plugin-publish: crates/idml-import/src/spread.rs:1861` and written
by `plugin-publish: crates/idml-export/src/rewrite.rs:1229`. The SDK doors are
`getMetadata(id)` and `setMetadata(id, envelope)`, with the key implicit
(`plugin-sdk: packages/plugin-api/src/host.ts:972`, `:980`).*

- Parse: `Label` already round-trips? **Verify first** — if
  `paged-parse` drops Labels today, carrying them through parse → scene
  → (the serializer) is step 0. The new `paged-write` carry-through
  work is the natural landing zone.
- New `PropertyPath::PluginMetadata { key }` with
  `Value::PluginMetadata(Option<String>)` — one SetProperty arm, full
  undo/inverse for free, invalidation `frame_style`-class. Write gate:
  key must equal the caller's plugin namespace; value must parse as the
  envelope and fit the cap.
- Read: `element_properties` includes plugin metadata entries;
  `host.document` grows `getMetadata(id, key)` / set via the ordinary
  mutate door (no new write path).

## 4. The bake() contract (SDK-side)

*Status note (2026-10-02): this section predates the implementation and is only
partly built. `contribute.objectType` is no longer reserved: the host adapter
registers an object type and its matcher
(`plugin-sdk: packages/plugin-sdk/src/host-impl.ts:1459`). It takes no baker.
`ObjectTypeBaker` and `BakeContext` are exported types
(`plugin-sdk: packages/plugin-api/src/host.ts:1011`, `:1015`) that no host code
calls, so nothing below about when the host calls `bake()` happens today; see
[ADR 316](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/316-native-content-and-baking.md)
of plugin-sdk.*

```ts
interface ObjectTypeBaker {
  /** Produce the baked IDML form (mutations creating/refreshing the
   *  object's DERIVED children) from the live metadata. Pure:
   *  (metadata, geometry) in, mutation batch out. */
  bake(ctx: BakeContext): Mutation[];
}
```

- Registered with `contribute.objectType` (reserved today — this design
  is what un-reserves it).
- The host calls `bake()` on metadata change (debounced) and before
  save/export; the batch applies atomically (one undo step) and is
  marked derived so reopen-with-plugin can replace it.
- A throwing/failing bake = diagnostic + save blocked (invariant 3).

## 5. Acceptance fixtures (the facility's definition of done)

*Status note (2026-10-02): the metadata half of F1 is fixtured in
`crates/paged-canvas/tests/idml_export_saveback.rs:740` and in
`plugin-publish: crates/idml-export/tests/label_roundtrip.rs`. The "derived
children regenerated" half depends on the bake loop of §4, which is not built.*

- **F1 round-trip live:** create live construct → save → reopen in
  Paged → live construct identical (metadata-equal, derived children
  regenerated bit-equal).
- **F2 foreign-open baked:** save → strip plugin (no bundle loaded) →
  document loads, renders the baked form, fidelity gate green; InDesign
  open verified via the existing corpus harness once exportable.

## 6. Consumer sequencing

*Status note (2026-10-02): this section predates the implementation. paged.web
marks its frame with this metadata and keeps the source in a container part as
well
([ADR 406](https://github.com/paged-media/plugin-web/blob/main/docs/adr/406-web-frame-and-source-storage.md));
its bake is an explicit command
([ADR 407](https://github.com/paged-media/plugin-web/blob/main/docs/adr/407-baking-flattens-to-native-items.md)).
paged.draw stores live constructs as recipes in container parts
([ADR 356](https://github.com/paged-media/plugin-draw/blob/main/docs/adr/356-live-constructs-are-recipes.md)).*

1. **paged.web** first (simplest bake: source → nothing yet /
   placeholder rectangle is already the baked form; later vector
   baking) — proves carrier + caps + gate.
2. **paged.draw live shapes** second (parametric rect/ellipse/polygon →
   baked path) — proves bake-on-change + the badge/expand UX.

## Open for review

- Cap size (64 KiB?) and whether `Label` vs a dedicated
  `x-paged`-namespaced XML element is the safer carrier for very large
  web sources (Label CDATA escaping overhead).
- Where the badge UX lives (selection chrome vs layers panel vs both).
- Whether `engine` pins belong in the envelope (per-object) or
  document-level (per-plugin singleton) — an open question in
  paged.web's design suggests both.
