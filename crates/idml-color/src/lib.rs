//! Color management.
//!
//! Wraps Little CMS 2 for ICC-based transforms. All document paints are
//! resolved to a linear RGB working space before being shipped to the GPU;
//! the final sRGB (or proof-profile) conversion happens in a fragment
//! shader so blending remains physically meaningful.
//!
//! WASM build strategy is deferred until `spikes/wasm-size` concludes
//! whether `lcms2-sys` compiles cleanly to `wasm32-unknown-unknown` or we
//! need to bundle a separate lcms-wasm module.
