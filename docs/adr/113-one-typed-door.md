# ADR 113 — One typed door drives every surface: wasm, CLI, session and scripts

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-canvas-wasm` (`dispatch.rs`, the wasm shell), `crates/paged-cli`, `crates/paged-run`, the write path of `crates/paged-script`

## Context

The engine is reached from a browser worker, from a command line, from a long-lived
headless process and from embedded scripts.

The CLI states the rule it was built on. Driving it through the editor's dispatcher "means
the CLI cannot apply a mutation the editor cannot, or load a document with options the
editor cannot express — and it inherits fonts, colour profiles, PDF export, the
container-parts door, scripting and undo without this crate implementing any of them"
(`crates/paged-cli/src/engine.rs:19-23`). The headless session had broken that rule before
it was moved: "Two implementations of load, export and render lived in one crate, and only
one of them was the door the editor uses" (`crates/paged-cli/src/session.rs:236-238`).

## Decision

Message handling is one target-independent type, `paged_canvas_wasm::dispatch::WorkerCore`.
The wasm shell, the CLI and the headless session send their messages through it; scripts
run inside it and write with the same operation vocabulary.

- `WorkerCore::dispatch` takes a typed `MainToWorker` and returns a `WorkerToMain` plus a
  `CacheEffect`; `handle_message` is the same with JSON text on both sides. The module
  compiles on every target and has native tests.
- The wasm shell keeps what needs a browser: the clock, console logging, the GPU presenter
  and the scene cache. It applies the returned `CacheEffect` itself.
- `paged-canvas` exports no wasm-bindgen function. Its two `#[wasm_bindgen]` attributes
  are TypeScript declaration sections.
- The `paged` CLI holds one `WorkerCore` in a `Session` and sends typed messages through
  `Session::send`. Its rule: anything that mutates or consumes session state goes through
  `send`; pure reads that have no message kind use `Session::model`.
- The NDJSON session (`paged session`) is built on the same `Session`. `paged-run` is a
  binary whose `main` only calls `paged_cli::session::run()`.
- A script changes the document by building `Mutation` values and passing them to
  `CanvasModel::apply_mutation`, the call the dispatcher's `Mutate` arm makes
  (`crates/paged-canvas-wasm/src/dispatch.rs:288-301`).
- Each surface's reach is a test. The CLI names 32 of the 62 message kinds; the 30 others
  are listed with a reason each. The script bridge names every wire operation except
  `BindCreated`. Both lists may only shrink.
- The reference for a surface is a committed file with a test that it is current:
  `crates/paged-introspect/catalog.json` and `crates/paged-cli/cli.json` are generated;
  `web/idml-viewer/api-catalog.json` is written by hand; its test checks that every export
  of `src/index.ts` appears in it and that each member has a name, a signature and a
  summary, not that the entries match the code
  (`web/idml-viewer/test/api-catalog.test.ts:15-19`, `:49-63`).

## Evidence

- `crates/paged-canvas-wasm/src/dispatch.rs:15-31`, `:172`, `:210-214` — the module doc, `handle_message`, `dispatch`
- `crates/paged-canvas-wasm/src/lib.rs:25-31`, `:636-653` — the shell's remit; `handleMessage` forwards to the core and applies the effect
- `crates/paged-canvas/src/lib.rs:17-19`, `crates/paged-canvas/src/channel.rs:49`, `crates/paged-canvas/src/resolve.rs:52` — "Pure Rust. No wasm-bindgen"; the two `typescript_custom_section` attributes
- `crates/paged-cli/src/engine.rs:15-33`, `:72-88` — the rule, and `Session::send`
- `crates/paged-cli/tests/cli_surface.rs:41-48`, `:59-61`, `:125`, `:259-262` — no `paged wire <json>`; the shrink-only list; the counts
- `crates/paged-script/tests/script_surface.rs:50-59`, `crates/paged-script/src/lib.rs:1039-1386` — the script gate; the bridge's writes
- `crates/paged-run/src/main.rs:15-36`, `crates/paged-run/Cargo.toml:14-22` — the shim and why it stays
- `crates/paged-cli/src/cli.rs:15-26`, `:313-323`, `crates/paged-introspect/src/catalog.rs:1340-1352` — the committed artifacts and their is-current tests

## Alternatives considered

- A raw `paged wire <json>` subcommand: refused, because "a raw-JSON escape hatch beside a
  raw-JSON session is duplication" (`crates/paged-cli/tests/cli_surface.rs:45-46`).
- Renaming `paged-run` or giving it arguments: three consumers start it by name, and
  "anything it accepted would be a second, divergent surface for the same protocol"
  (`crates/paged-run/src/main.rs:20-32`).
- Hand-written reference pages: "writing a second copy of this tree by hand, which drifts
  the first time a flag is added" (`crates/paged-cli/src/cli.rs:19-21`).

## Consequences

A new message kind or operation forces a decision in the surface tests: reach it, or add a
line with a reason. The older binaries `paged-inspect` and `paged-export` remain and do not
use the dispatcher; they drive the pipeline directly.

The wasm shell exports more than `handleMessage`: direct calls for document bytes, gesture
deltas, geometry queries and rendering ([ADR 115](115-worker-boundary-transports.md)). One
of them, `loadDocumentDirect`, calls `CanvasModel::load` in the shell and stores the model
on the core, beside the dispatcher's `LoadDocument` arm
(`crates/paged-canvas-wasm/src/lib.rs:260-298`).

The headless session does not accept wire messages. Its requests are twelve `cmd` forms
(`crates/paged-cli/src/session.rs:97-175`) served through the same `Session`; the sentence at
`crates/paged-cli/tests/cli_surface.rs:43-44` that it "already speaks the whole protocol" is
wider than that.

## Related

- [ADR 005](005-wire-recipe.md), [ADR 019](019-capability-catalog-one-contract.md) — the closed vocabulary and the generated catalog these surfaces project
- [ADR 001](001-boa-over-quickjs.md), [ADR 109](109-engine-does-no-io.md), [ADR 110](110-one-undo-timeline.md) — the script runtime, the registries and the undo log every surface inherits
- [ADR 115](115-worker-boundary-transports.md) — how messages and bytes cross the worker boundary
