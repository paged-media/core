//! Build-time validation of the Vello CMYK-parity WGSL shaders.
//!
//! The runtime path creates compute pipelines from these shaders on
//! every Vello render that needs the overprint compute path; a typo
//! that survives into the binary would only surface as a runtime
//! `create_compute_pipeline` error and a knockout fallback. Parsing
//! and validating the WGSL here via naga catches the issue in CI
//! (the `vello-backend` feature is exercised by `cargo check` /
//! `cargo build`).
//!
//! naga's build cost is negligible (a few ms per shader); we accept
//! that it's an unconditional build-dep rather than a feature-gated
//! one because cargo's `[build-dependencies]` table doesn't honour
//! per-feature gating cleanly. The validator is no-ops when the
//! shader files aren't present (e.g. on a stripped-down checkout).

fn main() {
    // Always emit the rerun-if-changed lines so cargo re-runs us when
    // either shader source moves, regardless of whether validation
    // succeeds (we'd still want a fresh attempt next build).
    let shaders: &[&str] = &[
        "src/cmyk_compute/shaders/splat_or_overprint.wgsl",
        "src/cmyk_compute/shaders/recomposite.wgsl",
    ];
    for path in shaders {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=build.rs");

    // Only attempt naga validation when the WGSL files are actually
    // present in the checkout. Builds against a slimmed-down dist
    // tarball that omits them should keep working.
    for path in shaders {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => panic!("idml-gpu build.rs: cannot read {path}: {e}"),
        };
        let module = match naga::front::wgsl::parse_str(&source) {
            Ok(m) => m,
            Err(e) => {
                let msg = e.emit_to_string(&source);
                panic!("idml-gpu build.rs: WGSL parse error in {path}:\n{msg}");
            }
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        if let Err(e) = validator.validate(&module) {
            let msg = e.emit_to_string(&source);
            panic!("idml-gpu build.rs: WGSL validation error in {path}:\n{msg}");
        }
    }
}
