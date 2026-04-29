//! `idml-gen` — emit a generated IDML mega-file to disk.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "idml-gen", version, about)]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Emit one of the built-in mega-files into `--out`.
    Emit {
        /// Mega-file name. Phase 0 only ships `geometry`.
        #[arg(long)]
        sample: String,
        /// Output directory. The `.idml` lands at `<out>/<sample>.idml`.
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Cmd::Emit { sample, out } => emit_sample(&sample, &out),
    }
}

fn emit_sample(name: &str, out_dir: &std::path::Path) -> Result<()> {
    let sample = match name {
        "geometry" => idml_gen::samples::geometry::build(),
        "geometry-groups" => idml_gen::samples::geometry_groups::build(),
        "strokes-fills" => idml_gen::samples::strokes_fills::build(),
        "text" => idml_gen::samples::text::build(),
        "text-advanced" => idml_gen::samples::text_advanced::build(),
        "effects" => idml_gen::samples::effects::build(),
        "gradients" => idml_gen::samples::gradients::build(),
        "tables" => idml_gen::samples::tables::build(),
        "images" => idml_gen::samples::images::build(),
        other => {
            anyhow::bail!(
                "unknown sample {other:?}; known: geometry, geometry-groups, strokes-fills, text, text-advanced, effects, gradients, tables, images"
            )
        }
    };
    let bytes = idml_gen::write_idml(&sample).context("write idml")?;
    std::fs::create_dir_all(out_dir).with_context(|| format!("mkdir {}", out_dir.display()))?;
    let path = out_dir.join(format!("{name}.idml"));
    std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
    eprintln!(
        "wrote {} ({} bytes, {} pages)",
        path.display(),
        bytes.len(),
        sample.spreads.len()
    );
    Ok(())
}
