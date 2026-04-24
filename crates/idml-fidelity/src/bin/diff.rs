//! `idml-diff`: compare two PNGs and report ΔE2000 + SSIM.
//!
//! Stub. Real implementation lands with Phase 0 corpus work; for now the
//! binary exists so CI plumbing can reference it without breaking.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "idml-diff", version, about)]
struct Args {
    /// Reference PNG (rasterised InDesign PDF).
    reference: std::path::PathBuf,
    /// Candidate PNG (renderer output).
    candidate: std::path::PathBuf,
    /// Optional heatmap output path.
    #[arg(long)]
    heatmap: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    eprintln!(
        "idml-diff: reference={}, candidate={}, heatmap={:?}",
        args.reference.display(),
        args.candidate.display(),
        args.heatmap,
    );
    eprintln!("not implemented yet; see crates/idml-fidelity for scope");
    Ok(())
}
