# W0 — Blitz/WASM feasibility spike

Go/no-go for paged.web's embedded-engine bet (concept §4.1, §10 Q1):
can the Blitz stack (Stylo + Taffy + Parley + blitz-dom/-html/-paint)
compile to `wasm32-unknown-unknown`, actually execute there, and at
what size/speed?

## Verdict — **GO** (measured 2026-06-06)

| Question | Result |
| --- | --- |
| Compiles to wasm32? | **Yes** — whole stack (`blitz-* 0.3.0-alpha.4`, stylo 0.17), clean `cargo check`/`build`, no patches |
| Executes on wasm? | **Yes** — run via `wasm-bindgen --target nodejs`; fragment paints 19 commands (see delta note) |
| Binary size | raw 12.31 MB → `wasm-opt -Oz` 9.15 MB → **brotli 2.20 MB** (Vello EXCLUDED — counting backend; see sharing note) |
| Performance (native proxy) | persistent doc re-layout+repaint **58 µs/frame**; fresh doc with shared `FontContext` **373 µs**; naive fresh doc 59 ms (font discovery dominates); cold start 0.12–1.4 s (system-font discovery — absent on wasm) |
| Paint-path correctness | representative fragment (flexbox, borders, backgrounds, nested flow, text) → 12 fills + 3 glyph runs + 7 layers natively |

### The 22 vs 19 command delta (native vs wasm)

Exactly the 3 glyph runs: wasm32 has no system fonts, so text shapes
to nothing until faces are registered. This is a FEATURE for paged.web
— the determinism doctrine forbids silent system-font dependence
anyway; the integration registers pinned faces exactly like
`ViewerSession::register_font`. **W1 task: font registration parity +
a wasm-side glyph-run assertion.**

### Version alignment (the integration luck)

`anyrender_vello 0.11` pins **vello ^0.9 + wgpu ^29 — exactly the
engine's versions** (and kurbo 0.13 / peniko 0.6 match too). A real
integration paints into the SAME Vello/wgpu instance as
`paged-canvas`/`paged-sdk`, so the marginal wasm cost is the Blitz
stack alone (≈ the 2.20 MB number, likely less after sharing
kurbo/peniko/parley-adjacent code).

### Integration levers found (`blitz_dom::DocumentConfig`)

- `font_ctx: Option<parley::FontContext>` — share ONE context across
  webFrames: 160× faster fresh-document renders (59 ms → 373 µs).
- `net_provider` — inject a no-op/asset-resolver provider (no network,
  per the determinism doctrine).
- `ua_stylesheets` — replace the browser-ish UA sheet with a
  paged-controlled one (part of the published compatibility table).
- `media_type` — can evaluate `@media print`.
- `style_threading` — set `Sequential` on wasm32 (no rayon threads).

## Reproduce

```bash
cd spikes/blitz-wasm
cargo test                                   # paint-path sanity (native)
cargo run --release --example w0_bench       # perf numbers
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/spike_blitz_wasm.wasm \
  --target nodejs --out-dir /tmp/w0-node
node -e "console.log(require('/tmp/w0-node/spike_blitz_wasm.js').w0_render_fragment_command_count())"
wasm-opt -Oz ... && brotli -q 11 ...         # size numbers
```

## Notes

- Standalone crate (workspace `exclude`) — the Blitz tree stays out of
  the engine's `Cargo.lock`. This crate's own `Cargo.lock` is
  committed so the numbers are reproducible.
- `blitz-dom` features here: `system_fonts` (native test path) +
  `woff`. Dropped: accessibility, file_input, custom-widget, svg.
  With `svg` re-enabled the size will grow (usvg) — measure when the
  compatibility table calls for it.
- Blitz is alpha; the pin is exact (`=0.3.0-alpha.4`). Bump
  deliberately and re-run this spike's numbers.
