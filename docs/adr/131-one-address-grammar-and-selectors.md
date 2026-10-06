# ADR 131 — One address grammar for every object, with selectors

- **Status:** Accepted 2026-10-06; implemented at protocol 71.
- **Scope:** `crates/paged-wire` (`address.rs`, `selector.rs`, `query.rs`), `crates/paged-canvas`
  (`RequestQuery`, `CanvasModel::query` / `resolve_address` / `target_of`), `crates/paged-script`
  (`paged.query`, `paged.address`), `crates/paged-cli` (`paged query`),
  `crates/paged-introspect` (the generated `idGrammar`), `crates/paged-model`
  (`Spread::item_names`); mirrored by the plugin API.
- **Extends:** [ADR 019](019-capability-catalog-one-contract.md) (one generated catalog).

## Context

Up to protocol 70 the engine named a write target four different ways:

- `ElementId` covered six page-item kinds plus a story range, a table and a cell. Its string
  form accepted only the six item prefixes and `storyRange:`; tables and cells had no string
  form at all.
- `paged_mutate::NodeId` also had spreads, pages, stories and layers, but the wire could not
  carry them, so layer and page paths were hidden from the catalog.
- Swatches, styles, gradients, colour groups, numbering lists, masters, sections, guides,
  conditions and the document were named by bare id strings inside dedicated operations.
- Nothing selected by name or label, and a page item's `Name` was not modelled.

A data binding, a script, a command line and a plugin object all need to name targets the same
way. Without one grammar each surface invents its own, and a binding stored in a document cannot
point at a swatch, a page or a plugin layer.

## Decision

There is ONE address grammar. Core owns the parser and the formatter (`paged_wire::Address`);
the plugin API mirrors it in TypeScript. The existing `ElementId` string forms stay the
canonical spellings, so every published script keeps working.

### Grammar

```text
doc
textFrame:<id>  rectangle:<id>  oval:<id>  polygon:<id>  graphicLine:<id>  group:<id>
                aliases: textframe: rect: graphicline:
frame:<id>      a page item of unknown kind — resolved by a document, never emitted
page:<id>       page:#<n>   (n-th page, 1-based, document order)
spread:<id>  layer:<id>  master:<id>  story:<id>
storyRange:<storyId>@<start>..<end>    alias: story:<storyId>@<start>..<end>
field:<storyId>@<offset>               a field anchored at a story character offset
table:<tableId>                        the table's Self, unique per document
cell:<tableId>/<row>,<col>             zero-based
style:<family>/<selfId or name>        family = paragraph|character|object|cell|table
swatch:<rest>  gradient:<rest>  colorGroup:<rest>  numberingList:<rest>
condition:<rest>  conditionSet:<rest>  textVariable:<rest>
guide:<id>  section:<id>  hyperlink:<id>
master:<id> > <item address>           an item on a master spread
plugin:<pluginId>/<kind>/<rest>        opaque to core, carried through
$h:<name>                              a batch handle
```

- `<rest>` runs to the end of the address, so self ids such as `Color/Red` or
  `ParagraphStyle/Body Text` need no escaping. `%` escapes only where an id must stop at a
  delimiter (`@`, `/`, ` > `).
- `style:` accepts the self id or the display name; the formatter always writes the self id.
- `table:` / `cell:` name the table by its `Self`; the resolver finds the owning story, so the
  wire's `(story_id, table_id)` pair stays internal.
- Guides are positional: `guide:Guide/<spreadSelf>/<index>`, the id the guide operations use.

### Selectors

`query(selector) → Address[]`, evaluated in core over the containment tree doc → spread → page →
page items (groups nest), doc → story → table → cell, and the document's resources.

```text
selector   = step *( combinator step ) , unions with ","
combinator = " > " (child) / " " (descendant)
step       = ( address / kind / "*" ) *predicate
predicate  = [name=…] [label=key] [label.key=value] [kind=…] [layer=…] [style=…] :nth(n)
op         = "=" / "!=" / "^=" / "*="
```

Examples: `page:#3 > frame`, `frame[name="price"]`, `frame[label.x-paged:oid="a1b2"]`.

A selector is never a write target. `Mutation::Set` takes ONE address; a caller resolves a
selector with `RequestQuery` first and sends one `Set` per address (in one `Batch` for one undo
step).

## Stability

An address is a session handle, not a durable reference: InDesign renumbers `Self` ids on save.
Anything stored in a document addresses by a stable key — a `Name`, or a label — through a
selector, never by a raw `Self`.

## Consequences

- `ElementId → Address` is total; `Address::to_element_id` answers for the kinds that need no
  document.
- The catalog's `idGrammar` is generated from `AddressKind::form()` (an exhaustive match, so a
  new kind does not compile without a documented, worked example).
- `ElementId` stays the target of `SetElementProperty`; the new surfaces send addresses.

## The gates

- `crates/paged-wire` unit and property tests: every form parses, formats back canonically, and
  `parse` never panics.
- `crates/paged-canvas/tests/address_grammar.rs`: every form in the published grammar parses,
  formats back byte-identically and resolves on ONE document holding an object of every kind,
  and every core address maps to an engine target (ADR 132).
- `every_published_id_form_actually_parses` (`crates/paged-script/tests/script_surface.rs`).

## Implemented (protocol 71)

- `RequestQuery { selector }` → `QueryResult { addresses, error }`; `paged.query`,
  `paged.address`; `paged query <doc> <selector>`.
- `textVariable:` joined the grammar with the property schemas (ADR 132), and guides are indexed
  so `guide:` addresses resolve.
- A page item's `Name` lives in `Spread::item_names`. Reading and writing it in IDML belongs to
  the IDML adapter (plugin-publish), which lands it behind its `item-names` step; until core pins
  that adapter revision, a name set in paged does not reach an IDML save.
