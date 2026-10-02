# Concept

Why this repository exists, what it is for, and what it will never do. This page states
intent, and each paragraph names its source in a comment. What is built today is in
[`status.md`](status.md); how it is built is in [`architecture.md`](architecture.md).

## Why it exists

`paged` is a render engine for page-layout documents, written in Rust. Its stated aim is
pixel-faithful rendering of Adobe IDML: it lays out text with a composer calibrated against
InDesign, emits a display list, and rasterises that list on the CPU or on the GPU. The
public APIs target native code and WebAssembly alike.
<!-- source: README.md:3-6 -->

This repository is the open engine: the rendering pipeline, including the parts that decide
fidelity (the calibrated line breaking, ICC colour, the WebGPU path). The editor built on
top of it lives in a separate repository and consumes the engine as published
`@paged-media` packages across a package boundary.
<!-- source: README.md:8-12; CLAUDE.md:20-26 -->

The engine is format-agnostic in design, and IDML is the first input format. The document
model is the engine's own ("the model is Paged's, not IDML's"): a format parser depends on
the model crate and imports into it, and the render and mutation stack speaks the model's
types.
<!-- source: README.md:14; crates/paged-model/src/lib.rs:15-21 -->

## What it is for

- **Rendering a document the same way everywhere.** One pipeline produces one display list
  per page; a CPU rasteriser and a Vello/WebGPU rasteriser both consume it, natively and in
  the browser. <!-- source: README.md:3-6, 24-36; crates/paged-gpu/src/lib.rs:17-24 -->
- **Being the engine behind an editor.** The worker-side document model, a typed message
  channel, and a mutation channel in which every operation is typed, serialisable and
  invertible, so undo, scripting and gestures share one path.
  <!-- source: crates/paged-canvas/src/lib.rs:15-32; crates/paged-mutate/src/lib.rs:15-21 -->
- **One door for every surface.** The command line drives the same typed entry point as the
  editor's worker, so it cannot apply a change the editor cannot, and it inherits fonts,
  colour profiles, PDF export, scripting and undo without implementing them again.
  <!-- source: crates/paged-cli/src/engine.rs:15-23 -->
- **A read-only viewer for the browser.** A small WebGPU session that loads a document and
  presents it to a canvas, linking nothing from the editing stack.
  <!-- source: crates/paged-sdk/src/lib.rs:15-21 -->
- **Print output.** PDF export is a second backend over the same display list: text stays
  text, vectors stay vectors, and the same input yields the same bytes.
  <!-- source: crates/paged-export-pdf/src/lib.rs:15-30 -->
- **Fidelity that is measured.** Rendering changes are gated on visual agreement with
  references exported by InDesign. The diff harness was built before the renderer, so that
  every later change is measurable, and a licence-clear golden set is committed so that
  outside contributors can run the gate.
  <!-- source: CONTRIBUTING.md:36-44; crates/paged-fidelity/src/lib.rs:21-22; README.md:197-199 -->

## What it will never do

- **Become a source dependency of the editor.** The editor never takes a Rust path
  dependency on the engine; it depends on the published packages.
  <!-- source: .github/workflows/publish-wasm.yml:3-6; CLAUDE.md:22-26 -->
- **Need a private repository to build or to pass its gates.** The repository keeps its own
  copies of the fonts it tests with, and the fidelity gate runs on the committed set.
  <!-- source: scripts/check-font-hashes.sh:4-6; corpus/generated/diff.sh:44-53; CONTRIBUTING.md:38-40 -->
- **Ship a fallback typeface or a colour profile.** The host supplies both. Text whose
  family no registered font answers for is not shaped, and the command line warns of it.
  <!-- source: README.md:160-163; crates/paged-color/src/profiles.rs:26-28 -->
- **Loosen a fidelity threshold to make a failure pass.** The regression is fixed first;
  thresholds are tightened after a fix, never before.
  <!-- source: CLAUDE.md:126-129; CONTRIBUTING.md:46-48 -->
- **Put editing code into the viewer, or give the viewer a second renderer.** The viewer
  session links no mutation, canvas or scripting crate; without WebGPU it rejects.
  <!-- source: crates/paged-sdk/src/lib.rs:17-27; crates/paged-sdk/WEBGPU.md:50-53 -->
- **Open a second, untyped door on the command line.** There is no raw-JSON subcommand;
  nothing in the CLI loads a document or applies a mutation except through the dispatcher.
  <!-- source: crates/paged-cli/tests/cli_surface.rs:41-48; crates/paged-cli/src/engine.rs:25-29 -->
- **Let a script reach the document any other way than through the host functions.**
  <!-- source: crates/paged-script/src/lib.rs:17-21 -->
- **Let a plugin's container part replace document content.** A part must live under the
  `paged/` namespace; the IDML parts, the `mimetype` entry and `manifest.json` are off
  limits. <!-- source: crates/paged-canvas/src/model.rs:4564-4568 -->
- **Invent a number it could not measure.** Ink coverage reports how many pixels carry real
  separation data instead of estimating the rest. <!-- source: crates/paged-gpu/src/separations.rs:44-55 -->
- **Take a dependency that is not permissively licensed.** `cargo deny` gates licences,
  advisories and sources. <!-- source: CONTRIBUTING.md:50-54; .github/workflows/licenses.yml:23 -->
