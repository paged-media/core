# ADR 003 — Color CMM: lcms2 native, qcms on wasm

**2026-06-07 · decision record · status: ACCEPTED (records a split shipped in
code; carries known parity debt).**

**Sources:** `crates/paged-color/Cargo.toml:17-31` (`lcms2 = "6"`
native-only; `qcms = "0.3"` target-gated to wasm32 + compiled natively for the
parity test); `crates/paged-color/src/cmm.rs:135` (the `Cmm` trait abstracts
"lcms2 native / qcms wasm32"), `:307-311` (Lab handling diverges on wasm),
`:395` (`mod native_export`), `crates/paged-color/tests/parity.rs` (lcms2-vs-qcms
side-by-side bake-off); the original design spec, §9.2 ("lcms2 compiled to WASM" —
never happened).

## The decision

**Color management uses lcms2 (the C library, via `lcms2-sys`) on native
targets and qcms (Mozilla's pure-Rust ICC library, ships in Firefox) on
wasm32 — not a single pure-Rust CMS, and not lcms2-compiled-to-wasm as
the original design spec (§9.2) promised.** The `Cmm` trait (`cmm.rs:135`) is the seam; the
lcms2-vs-qcms split never leaks past it.

## Why (reconstructed rationale)

The native side is a deliberate fidelity-first choice; the wasm side is a
toolchain-forced substitution — **reconstructed** from the Cargo.toml comments
and the parity test, not a prior written decision:

- **Fidelity-first on native (where the gate runs).** lcms2 is the reference CMM
  (Relative Colorimetric + BPC); the fidelity gate's ΔE2000 budget is measured
  on the native path. Re-implementing a CMS in pure Rust to ΔE ≤ 1.0 mean / 2.5
  p99 was not worth it when the industry-standard C library exists.
- **qcms on wasm because lcms2-in-wasm was not pursued.** Rather than carry the
  C cross-compile lane into the wasm build (the same toolchain cost
  [ADR 001](001-boa-over-quickjs.md) rejected for QuickJS), the wasm path uses
  qcms — pure-Rust, CMYK + ICCv4, battle-tested in Firefox. The original design
  spec's "lcms2 to WASM" silently became "qcms on wasm."

## Consequences

- **Native/wasm ΔE divergence is a known, measured debt.** The two engines are
  not byte-identical; `tests/parity.rs` compiles qcms *natively* specifically to
  run lcms2 and qcms on the same inputs and bound the divergence (the CMM
  bake-off evidence). The fidelity gate runs on native (lcms2) only — so the
  *shipped* path (wasm/qcms) is verified by the parity test, not by the gate.
- **Lab handling differs on wasm.** qcms exposes no Lab transform, so Lab colors
  are resolved analytically to sRGB first on wasm (`cmm.rs:307-311`); native
  lcms2 does Lab↔CMYK directly. This is the most likely source of any residual
  ΔE gap.
- ICC-on-wasm remains listed in the internal renderer gap list; the parity test
  is the guard that the wasm substitution stays inside the fidelity envelope.
