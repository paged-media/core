//! Text engine.
//!
//! Highest-risk subsystem; roughly 40% of project effort. Responsibilities:
//! - shaping each homogeneous run via rustybuzz
//! - Knuth-Plass line breaking with InDesign-calibrated penalty weights
//! - hyphenation (TeX patterns by default; Proximity if licensed)
//! - composition into frame-bound layouts with justification
//!
//! Calibration of Paragraph Composer parity happens in
//! `spikes/composer-calibration` before this crate takes a hard dependency
//! on any specific penalty configuration.
