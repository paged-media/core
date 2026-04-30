//! Module pipeline for cross-cutting page-item concerns.
//!
//! IDML's data model attaches the same set of cross-cutting attributes
//! to every page-item element — `<TransparencySetting>`,
//! `<StrokeTransparencySetting>`, `<FillTransparencySetting>`,
//! `<ContentTransparencySetting>`, `ItemTransform`, `AppliedObjectStyle`,
//! `FillColor`, `StrokeColor`, `GradientFillAngle`, etc. — and only the
//! geometry differs per shape. The shape-specific emit functions in
//! `pipeline.rs` historically duplicated ~70% of the same preamble
//! (stats / transform / fill-transparency / shadow / fill / stroke).
//!
//! This module hosts a small, fixed pipeline of "modules" — free
//! functions that each own one cross-cutting concern (drop shadow,
//! fill paint, stroke paint, ...). They consume a flattened
//! [`ResolvedFrame`] IR and emit display commands via the geometry
//! adapter in [`geometry`].
//!
//! See `docs/idea.md` and `/Users/drietsch/.claude/plans/vectorized-humming-lobster.md`
//! for the full design.

pub(crate) mod drop_shadow;
pub(crate) mod frame;

#[allow(unused_imports)]
pub(crate) use frame::{Geometry, RenderCtx, ResolvedFrame};

pub(crate) use drop_shadow::drop_shadow_module;
