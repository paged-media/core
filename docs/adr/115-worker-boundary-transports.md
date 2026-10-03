# ADR 115 — The worker boundary uses three transports

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-canvas` (`channel.rs`, `camera.rs`, the layout block in `gesture.rs`), `crates/paged-canvas-wasm/src/lib.rs`, `ByteBuf` in `crates/paged-wire`

## Context

The editor runs the engine in a Web Worker and talks to it from the main thread. Three
kinds of traffic cross that boundary: requests and replies that must arrive whole and in
order; values that change on every input event; and payloads of many megabytes.

The code records a reason for each lane. The message handler returns a string because that
"keeps the boundary simple — no nested serde-wasm-bindgen conversions, just text in and text
out" (`crates/paged-canvas-wasm/src/lib.rs:639-642`). The camera is kept out of the message
channel to "avoid the `postMessage` round-trip (which serialises through the message queue
and costs at least a frame)" (`crates/paged-canvas/src/camera.rs:19-22`). Document bytes get
their own entry point because a byte array inside JSON is "8×-inflated" and documents of
100 MB and more failed with "capacity overflow" on that path
(`crates/paged-canvas-wasm/src/lib.rs:251-256`).

## Decision

A host and the engine's worker object, `CanvasWorker`, exchange data over three transports.

1. **JSON-string envelopes.** `MainToWorker` and `WorkerToMain`, each
   `{seq, protocol, kind, payload}`, cross as JSON text through
   `handleMessage(string) -> string`. A reply echoes the request's `seq`; an unsolicited
   message has `seq: null`. The `Tsify` derives on the kind enums generate the TypeScript
   declarations; the two outer envelope types are declared by hand.
2. **Shared-memory mailboxes.** Two `SharedArrayBuffer` layouts of 32 bytes each: the camera
   (`scale`, `tx`, `ty`, a 64-bit generation counter) and the gesture slot (handle, `dx`,
   `dy`, modifier bits, `seq`, generation). The writer stores the fields, then bumps the
   generation; the reader takes the latest value. The Rust constants are the authority and
   are exported (`cameraSabLayout()`, `gestureSabLayout()`) so the host can check its copy.
   No Rust code touches the buffers: the host's worker script reads them and calls
   `presentFrame(scale, tx, ty, dpr)` and `updateGestureRaw(handle_lo, handle_hi, dx, dy,
   modifier_bits)` with plain numbers.
3. **Direct calls with bytes.** `loadDocumentDirect(seq, bytes, font, cmyk_icc_profile)`
   takes the document as a byte slice and returns the same JSON reply `handleMessage` would.
   `renderTilePng` returns PNG bytes for the render loop.

Every envelope carries `PROTOCOL_VERSION` (64 at this commit). The worker answers `Hello`
with `Ready {protocol}` and exposes a `protocolVersion` getter.

## Evidence

- `crates/paged-canvas/src/channel.rs:18-28` — envelopes, `seq`, and "Camera updates are NOT in this channel"; `:45-60` why `Tsify` is off the outer envelopes, and the hand-written types
- `crates/paged-canvas/src/channel.rs:708-730`, `:1407-1427` — the envelope structs and the `kind`/`payload` tagging
- `crates/paged-canvas-wasm/src/lib.rs:637-653` — `handleMessage`; `:209-214` the `protocolVersion` getter
- `crates/paged-canvas/src/camera.rs:24-53` — camera layout, writer and reader steps, the accepted torn read, "the constants below are Rust-authoritative"
- `crates/paged-canvas/src/gesture.rs:46-71` — the gesture mailbox layout; `crates/paged-canvas-wasm/src/lib.rs:589-611` `updateGestureRaw`; `:676-712` the layout exports
- `crates/paged-canvas-wasm/src/lib.rs:251-266` — `loadDocumentDirect`; `:216-226` `renderTilePng`
- `crates/paged-wire/src/lib.rs:582-592` — `ByteBuf`: bytes inside an envelope are a JSON array of numbers
- `editor: apps/canvas/src/worker/worker.ts:175-178`, `:277-299`, `:682-701` — the host compares the layouts and the version after the wasm loads and posts a `protocolMismatch` warning; its drain loop reads the gesture buffer and calls `updateGestureRaw`

## Alternatives considered

- Typed objects instead of JSON text: rejected for the message handler in the comment quoted
  above. `crates/paged-canvas-wasm/Cargo.toml:49` declares `serde-wasm-bindgen`; no Rust
  code under `crates/` calls it.
- A binary wire, not built: the `ByteBuf` comment says a future CBOR or MessagePack protocol
  would render "a real bytes blob without code change" (`crates/paged-wire/src/lib.rs:586-588`).

## Consequences

The text path is handled by a function that compiles on every target, so it is tested
natively with JSON strings (`crates/paged-canvas-wasm/tests/dispatch.rs:43`). Native callers
skip the text: the CLI passes typed `MainToWorker` values to the same dispatch
(`crates/paged-cli/src/engine.rs:86`, [ADR 113](113-one-typed-door.md)).

A host must mirror the two buffer layouts and the envelope union. `UpdateGesture` also
exists as an envelope kind; the camera has no envelope form. `CameraLayout` is a
plain-bytes stand-in; its comment says the wasm side wraps a real buffer, which no code
does (`crates/paged-canvas/src/camera.rs:139-142`). A torn camera read is accepted as "a
single-frame visual glitch" (`:43-48`).
Only document load has a direct byte lane for input. `RegisterFont`, `WritePagedPart`,
image tiles and the exported PDF, IDML and `.paged` bytes travel as JSON number arrays.

The engine does not compare the `protocol` field of an incoming envelope; the check is the
host's. The comment on `PROTOCOL_VERSION` says the main thread "refuses to proceed on
mismatch" (`crates/paged-canvas/src/channel.rs:62-65`); the editor's worker script posts a
warning and continues. The layout comments name three different TypeScript files as the
mirror (`crates/paged-canvas/src/camera.rs:51`, `crates/paged-canvas/src/gesture.rs:68`,
`crates/paged-canvas-wasm/src/lib.rs:602`).

## Related

- [ADR 006](006-protocol-coupled-versioning.md) — what a change to `PROTOCOL_VERSION` means for package versions
- [ADR 113](113-one-typed-door.md), [ADR 114](114-interaction-lives-in-the-engine.md), [ADR 111](111-wire-vocabulary-leaf-crate.md) — the dispatch behind `handleMessage`; the gestures the mailbox feeds; where the payload types live
- [ADR 202](https://github.com/paged-media/editor/blob/main/docs/adr/202-render-worker-owns-the-canvas.md) — the editor side of the same boundary
