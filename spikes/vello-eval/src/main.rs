//! Spike A: Vello evaluation harness.
//!
//! Goal: exercise Vello against the §10 feature set and identify every
//! capability gap before committing to Vello as the rasterizer.
//!
//! Pass criterion (from the development plan): a 1-page test document
//! within 20% ΔE of a reference PDF with no crashes, plus a written
//! feature-gap list mapped to idea.md §10.4.
//!
//! The cases below drive the evaluation. Each one renders to a PNG; the
//! fidelity harness compares against a paired reference PDF.

use anyhow::Result;

#[derive(Debug, Clone, Copy)]
struct EvalCase {
    name: &'static str,
    /// idea.md section this case exercises.
    exercises: &'static str,
    /// What to look for in the output.
    looking_for: &'static str,
}

const CASES: &[EvalCase] = &[
    EvalCase {
        name: "complex-bezier",
        exercises: "§10.1 path rasterization",
        looking_for: "antialiased edges on self-intersecting, high-curvature paths",
    },
    EvalCase {
        name: "glyph-outlines",
        exercises: "§8.4 glyph rasterization unified with vector path",
        looking_for: "ttf-parser outlines rendered via Vello match FreeType reference",
    },
    EvalCase {
        name: "blend-modes-pdf17",
        exercises: "§10.4 Color / Luminosity / Hue / Saturation",
        looking_for: "per-PDF-1.7-spec output; flag any mode Vello approximates or omits",
    },
    EvalCase {
        name: "transparency-groups",
        exercises: "§10.4 isolated / knockout groups",
        looking_for: "intermediate render-target semantics correct",
    },
    EvalCase {
        name: "tiled-raster",
        exercises: "§10.3 raster placement + resampling",
        looking_for: "bicubic resample quality; large-image tiling without artefacts",
    },
    EvalCase {
        name: "drop-shadow",
        exercises: "§10.4 separable Gaussian + offset",
        looking_for: "Vello's current blur/filter maturity",
    },
];

fn main() -> Result<()> {
    eprintln!("spike-vello-eval: {} cases queued\n", CASES.len());
    for case in CASES {
        eprintln!(
            "  [{}] {} — {}",
            case.exercises, case.name, case.looking_for
        );
    }

    // Sanity-check that the vello types link so Cargo resolves the dep stack.
    let _scene = vello::Scene::new();
    eprintln!("\nvello::Scene constructed OK");

    eprintln!("\nTODO: wire wgpu adapter, vello Renderer, render each case to a PNG.");
    eprintln!("TODO: compare against corpus/seeds/vello-eval/*.png via paged-diff.");
    Ok(())
}
