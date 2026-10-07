# ADR 132 — Typed property schemas, virtual paths and one `Set { address, path, value }`

- **Status:** Accepted 2026-10-06; implemented at protocol 71.
- **Scope:** `crates/paged-introspect` (`schema.rs`, `virtual_paths.rs`, the catalog's
  `propertySchemas`), `crates/paged-mutate` (`Value::Number`/`Enum`/`Ref`/`GradientStops`, the
  new `PropertyPath`s, `SetStyleProperty` over object/cell/table styles, effects on lines),
  `crates/paged-wire` (`Mutation::Set`, `VirtualTarget`), `crates/paged-canvas`
  (`virtual_set.rs`, `RequestGet`), `crates/paged-script` (`paged.set`/`paged.get`),
  `crates/paged-cli` (`paged set`/`paged get`).
- **Depends on:** [ADR 131](131-one-address-grammar-and-selectors.md). **Extends:**
  [ADR 019](019-capability-catalog-one-contract.md).

## Context

At protocol 70 every property went through `SetElementProperty` with a weakly typed `Value`:
enums travelled as `Text`, every script number became a `Length`, and a hand list in the script
bridge chose `Text` over `ColorRef`. Nothing recorded which node kinds a path accepts. About 45
of the wire operations were property writes with their own argument shape (`editSwatch`,
`layerSetVisible`, `setDocumentDefaults`, `renameMaster`, `setRowHeight`, …), so a surface that
wanted "set any property" had to know 45 shapes besides the generic door. Object, cell and
table style definitions, effects on lines, text variables, page margins and columns, document
setup and item names were modelled and unsettable.

## Decision

Every settable or readable thing is a triple **(address kind, path, schema)**, held in one table
in core. The catalog, the script coercion, the command line and the plugin SDK read it.

- **Schemas.** `PropertySchema` gives the value type (bool, number with unit, length, colour,
  enum with members, text, ref to another address kind, bounds, transform, point, list, …),
  nullability, range, default, access (`readWrite` / `readOnly` / `derived`), the IDML mapping
  where there is one, and — for a virtual path — the operation that writes it. The catalog
  publishes them as `propertySchemas`, beside `settablePaths`.
- **Values.** `Value` gains `Number`, `Enum`, `Ref` and `GradientStops`, additively; the apply
  layer coerces by schema (`Length` into a number, `Text` into an enum checked against its
  members, `"3mm"` into a length), so every older payload keeps applying.
- **Virtual paths.** A dedicated operation that sets a property is exposed as a path on its
  address kind: `swatch:<id>` `tint`, `layer:<id>` `layerVisible`, `doc` `defaultFillColor`,
  `page:<id>` `appliedMaster`, `table:<id>` `rows[2].height`, `cell:<t>/1,2` `rowSpan`,
  `field:<story>@<offset>` `value`, `frame` `labels[x-paged:oid]`, and so on.
- **One write.** `Mutation::Set { address, path, value }`. `address` is an ADR 131 address
  STRING on the wire. The engine resolves the address against the document to a
  `VirtualTarget` (below), checks the value against the schema, and lowers the write to the ONE
  existing operation that already writes that property — `SetProperty` for a real path, or the
  dedicated operation with its spec read and the one field merged. There is no second apply
  path: undo, inversion and batch handles (`$h:`, whole or inside an address) are that
  operation's.
- **One read.** `RequestGet { address, path }` → `PropertyValue { value, error }`, over the
  same table.

### Address → target

`VirtualTarget` is the resolved target: each variant is one ADR 131 kind, carrying the ids the
operations take. The map is total over the core kinds:

| Address | Target |
|---|---|
| `doc` | `Doc` |
| six item kinds, `master:<m> > <item>`, `storyRange:` | `Element { elementId }` |
| `frame:<id>` | `Element`, kind found in the document |
| `page:<id>`, `page:#n` | `Page { pageId }` (`#n` resolved) |
| `spread:`, `layer:`, `master:`, `story:`, `section:`, `guide:`, `hyperlink:` | the variant of that kind |
| `field:<story>@<offset>` | `Field { storyId, offset }` |
| `swatch:`, `gradient:`, `colorGroup:`, `numberingList:`, `condition:`, `conditionSet:`, `textVariable:` | the variant of that kind |
| `style:<family>/<id or name>` | `Style { family, styleId }` (name resolved) |
| `table:<id>` / `cell:<id>/<r>,<c>` | `Table` / `Cell`, with the owning story found |
| `plugin:…` | refused: plugin objects are set through the host's object registry |
| `$h:…` | substituted inside the batch that bound it, refused outside |

The protocol-70 JSON target object (`{ "kind": "layer", "layerId": "ua" }`) still deserialises
in the `address` position as a deprecated alias (`VirtualTarget → Address` is total too).

## Consequences

- `SetStyleProperty` accepts the object, cell and table style collections.
- The fields that were modelled and unsettable gained paths: page margins and columns, document
  bleed and slug, text-variable definitions, object/cell/table style fields, `item.name`; the
  effect paths reach graphic lines.
- Custom text variables can be created (`createTextVariable`) and instanced in text
  (`insertField` with `{ textVariable: { variableId } }`); one `Set` on `textVariableContents`
  switches every instance and re-bakes its text, which is how a data binding drives a value
  InDesign keeps.
- A swatch's colour is one value too: `swatch.color` (`{ space, value, model?, tint? }`) changes
  the space with its components in one write, so a panel never passes through a half-written
  colour; `space` and `value` stay as separate rows for reads and same-space edits.
- A three-state bool (`paragraphNumberingContinue`: on, off, inherit) is a nullable bool; `null`
  clears the override.
- The 45 dedicated property operations stay on the wire; new surfaces send `Set`.

## Implemented (protocol 71)

- **A `Set` is one undo step, whatever it resolves to.** The application-state operations
  (`setColorSettings`, `setProofSetup`, `setInkSetting`, `setUseStandardLabForSpots`,
  `setDocumentDefaults`) stay unlogged when sent on their own. A `Set` that resolves to one of
  them logs an `AppState` record whose inverse is the same operation built from the state it
  replaced, so `doc.renderingIntent` and the other colour rows undo and redo like every other
  row. Before this, the write reported one step and an undo popped the previous edit.
- **`cellFillTint` reads on a cell.** The read answers `null` (the fill swatch at 100 %). The
  model has no per-cell tint yet: the IDML adapter does not parse a cell's `FillTint`, and the
  write stays the typed no-op the apply arm documents. Modelling it needs a `TableCell` field
  plus the adapter's parse and emit, which has to land with the plugin-publish pin (the adapter
  builds `TableCell` without `..Default::default()`, so a new field breaks the pinned revision).
- **A script's undo step holds only its own writes.** A script that wrote through the host's
  object registry undoes as one step: its core writes, plus the engine writes the host made
  answering it, which the host reports as `HostObjectsResult.committed` (their `appliedSeq`s).
  A record logged while the script waited that is neither, such as a UI edit, is never folded:
  it keeps its place, and the script's writes on either side of it stay separate steps
  (`CanvasModel::group_undo_runs_since`). A host that reports nothing gets no folding of
  anything logged in the gap.
  **Known limit:** the fold merges only the session's consecutive own records, so a script
  write made BEFORE an interleaved UI edit (one logged while the script waited on the host)
  becomes its own undo step: the script then undoes in two steps around the UI edit. This is
  deliberate; merging across the foreign record would put the UI edit inside the script's
  step (`a_foreign_edit_before_the_hosts_commit_splits_the_script_around_it` in
  `crates/paged-script/tests/host_bridge.rs` pins it).
- **`null` clears a nullable row.** The wire's `text`, `enum` and `ref` payloads take `null`
  as the empty string, the engine's "no override", so `{ type, value: null }` clears exactly as
  `""` does, on `Set`, in Boa and on the command line. A required row still refuses it;
  `doc.activeConditionSet` is required, because activating a set is the only operation and
  there is nothing to clear to (`null_clears_every_nullable_text_enum_and_ref_row`).
- **A read tells mixed from missing.** `get_property` answers a `ReadError` with a stable code,
  and `RequestGet`'s `PropertyValue` carries it additively: `mixed: true` (no value, no error)
  when the object has the path but holds more than one value, such as a story range whose runs
  disagree; `code` (`noSuchPath`, `notFound`, `badAddress`, `noDocument`) on a refusal.
- **Typed commands through the script bridge.** `HostObjectCall` gains `invoke { id, args }`
  and `commands { owner? }` (additive): `paged.invoke(id, args)` and `paged.commands(owner?)`
  ask the host's registry, and an object `paged.batch` takes `{ op: "invoke", id, args,
  handle? }` ops, which send the whole batch to the host so the command shares its one undo
  step.

## The gates

- `catalog_json_artifact_is_current`, and the schema table is total over every `PropertyPath`
  and every virtual row.
- `catalog_apply_parity` and `crates/paged-canvas/tests/virtual_paths.rs`: every schema row is
  set through `Set`, read back through the read twin, and undone.
- `crates/paged-script/tests/object_model_surface.rs` and `script_surface.rs` (Boa);
  `crates/paged-cli/tests/object_surface.rs` and `cli_surface.rs` (command line).
