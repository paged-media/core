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

pub(crate) mod corner_path;
pub(crate) mod drop_shadow;
pub(crate) mod fill_paint;
pub(crate) mod frame;
pub(crate) mod geometry;
pub(crate) mod glyph_shadow;
pub(crate) mod object_style;
pub(crate) mod stroke_paint;

#[allow(unused_imports)]
pub(crate) use frame::{Geometry, RenderCtx, ResolvedFrame};

#[allow(unused_imports)]
pub(crate) use corner_path::{corner_path_module, CornerPaths};
pub(crate) use drop_shadow::drop_shadow_module;
pub(crate) use fill_paint::fill_paint_module;
pub(crate) use glyph_shadow::emit_glyph_shadow_pass;
pub(crate) use object_style::{object_style_cascade, resolve_applied_style};
pub(crate) use stroke_paint::stroke_paint_module;
