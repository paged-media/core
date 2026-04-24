# IDML Faithful Renderer

Pixel-faithful renderer for Adobe IDML documents, built on Rust + WebGPU.

See `idea.md` for the full technical specification and
`/root/.claude/plans/do-a-deep-research-humming-wigderson.md` for the
development strategy this scaffold implements.

## Workspace layout

```
crates/
├── idml-parse/       ZIP + XML → AST
├── idml-scene/       Resolved scene graph
├── idml-text/        Shaping, line breaking, hyphenation, composition
├── idml-color/       ICC transforms (lcms2)
├── idml-compose/     Scene graph → display list
├── idml-gpu/         wgpu backend, Vello integration, PathRasterizer trait
├── idml-renderer/    Top-level public API
├── idml-fidelity/    Corpus + diff harness + CI gate
└── idml-wasm/        wasm-bindgen surface for the browser

spikes/
├── vello-eval/             Spike A — Vello feature coverage
├── composer-calibration/   Spike B — Knuth-Plass calibration vs InDesign
└── wasm-size/              Spike C — measure compressed WASM size

corpus/
└── seeds/            Golden IDMLs + reference PDFs (populated in Phase 0)
```

## Spike pass criteria

| Spike | Criterion |
| --- | --- |
| A — Vello eval | 1-page test doc rendered ≤ 20% ΔE of reference PDF; written feature-gap list mapped to idea.md §10.4 |
| B — Composer calibration | ≥ 95% line-break parity on a 30-paragraph calibration corpus |
| C — WASM size | Compressed artefact ≤ 3.5 MB (target 3 MB) |
| Fidelity harness | Deterministic ΔE2000 + SSIM output across Linux and macOS; flags a synthetic 1-pixel shift correctly |

## Getting started

```bash
# Validate the workspace compiles.
cargo check --workspace

# Run Spike A harness (native, prints cases + constructs a Vello Scene).
cargo run -p spike-vello-eval

# Spike C: build for wasm32 and measure.
./spikes/wasm-size/measure.sh
```

## Branch

All development lands on `claude/read-idea-file-vHroZ` per the session
branch requirement.
