//! Mutable editing layer over an immutable `idml_scene::Document`.
//!
//! The base `Document` stays read-only forever. `Project` overlays a
//! sparse patch table + a per-story rope + an undo stack on top, and
//! exposes a `ResolvedView` that the renderer consumes in place of
//! `&Document`. Commands are the only mutation surface; every command
//! returns a `Patch` describing what got invalidated, which is the
//! input the incremental pipeline uses to evict caches.
//!
//! M0 ships the type skeleton: `Project`, `Command`, `Patch`, `NodeId`
//! and a no-op `apply` that bumps the epoch and records undo. Real
//! mutation lands in M1 (frame transforms) and M2 (text). The shape
//! of the public surface here is what `idml-edit-wasm` binds for the
//! browser.

pub mod caret;
pub mod command;
pub mod guides;
pub mod hittest;
pub mod ids;
pub mod patch;
pub mod persist;
pub mod project;
pub mod rope;
pub mod style_graph;

pub use caret::{CaretPos, Selection};
pub use command::{Command, EditError};
pub use guides::{compute_snap, GuideSegment, SnapResult};
pub use hittest::{hit_test_spread, AabbPt, FrameHit};
pub use ids::{NodeId, ParaId, RunId, StoryId};
pub use patch::{InvalidationKind, Patch};
pub use persist::PersistError;
pub use project::{Project, ProjectStats};
pub use style_graph::StyleGraph;
