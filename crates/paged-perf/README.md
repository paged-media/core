# paged-perf

Performance workloads, work budgets and benches for the engine.

## Why it exists

In October 2026 two performance bugs reached the editor:
- **Font hashing:** a whole-file font hash ran per paragraph, about 17 s per rebuild on the 134-page annual.
- **Image decoding:** every repaint decoded the visible photos again, about 310 ms after each write.

Nothing in core saw either one. The only bench used two tiny fixtures with no registered fonts and no photos, so it never took the expensive path. Both bugs were found later by profiling the real document in the browser.

So everything here runs on a workload that carries what real documents carry.

## The workload (`paged_perf::build`)

| What | Why |
|---|---|
| `annual-base`: 134 facing pages, 7 masters, the full style sheet | the real document's structure |
| the whole `corpus/fonts` directory **registered**, variable faces included | the path where the font-hash bug hid |
| one story threaded through 116 body frames, ~474k characters (partly overset) | a keystroke reflows across frames |
| 29 generated JPEG / PNG / WebP photos placed through `ReplaceImageBytes` | the path where the decode bug hid |
| 12 tables | table layout |

It's authored through the mutation wire in five batches and builds in ~1.8 s (release). It's deterministic, and no fixture bytes are committed.

**Rule:** a new scenario states which expensive paths it covers (fonts, images, grow, keeps, effects). Without fonts or images it is a micro-bench, never a baseline.

## The three layers

1. **Work budgets** (`tests/perf_budgets.rs`, part of `cargo test`):
   - They count work, not time, through `paged_compose::perf`: font bytes hashed, images decoded, page scenes built, grow passes, story emits.
   - Counts are deterministic, so they gate on any runner.
   - With the font-id memo disabled, the load hashes 18.8 GB and four of the five tests fail.
   - **A budget is the measured value.** Lower it when an optimisation lands; never raise it to make a change pass.
2. **Wall-clock benches** (`benches/workload.rs`, criterion):
   - `load`, `write` (keystroke, paragraph style, frame write, 50-edit batch), `export` (`.paged`, IDML, PDF), `raster`.
   - These are trended, not gated.
3. **The browser:**
   - `cargo run --release -p paged-perf --example workload -- 1600 --save perf-annual.paged` writes the workload for the in-browser lane.
   - The worker's `perfCounters()` (canvas-wasm) exposes the same counters, plus the scene counts that exist only on wasm.

## Profiling playbook

- **Wasm:**
  - `scripts/perf/profile-wasm.sh` builds a **named** wasm (`wasm-opt -Oz -g`). The published build shows only `wasm-function[N]`.
  - Record a Chrome trace with `disabled-by-default-v8.cpu_profiler`.
  - Aggregate it with `python3 scripts/perf/aggregate-trace.py trace.json`.
- **Native:** `samply record target/release/deps/workload-* --bench` (install it with `cargo install samply`).

Two lessons:
- **Profile the real workload, with real fonts and images.** A faster, simpler baseline usually means it is running different code.
- **Check that a baseline takes the same path as what it's compared with.** The "wasm is 900× slower than native" premise came from a native run that skipped font hashing entirely.

## Known costs, measured and pinned

- **Only PAINT writes keep the text caches.** A write that moves, resizes, wraps, hides or re-layers a page item, and every style, swatch or structural operation, still clears them and lays out every story (52 here). The body-story key already holds chain geometry and wrap shapes, so geometry writes are the next candidates; each needs the same digest-gate sweep before it joins the list.
- **A first save deflates every photo again.** Images are container parts of their own (`paged/core/model/blobs/`), and a re-save of a loaded document carries them through as a raw zip copy (355 ms). The first save writes them through `write_paged`, which deflates every new part: 1.24 s, most of it spent compressing already-compressed JPEG, PNG and WebP. Storing them uncompressed needs a per-part option in plugin-publish.

## Fixed, with the measurement that found it

- **A frame write re-laid out every story** (52 emits, against 2 for a keystroke), because any non-text operation cleared the body-story emit cache. A write that only repaints a plain page item now keeps it (`Invalidation::PageItemPaint`, `crates/paged-canvas/tests/paint_invalidation.rs`): 0 emits, and the write costs about what a keystroke does.

- **`document.pgm` stored image bytes as JSON integer arrays**, one number per byte: the workload's 29 photos made a 211 MB part (67 MB zipped). Saving took 11.3 s and loading 1.9 s. Now 3.1 MB, 1.24 s (355 ms on a re-save) and 0.9 s.
