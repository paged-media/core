//! Inspector API: read-side scene-graph introspection paired with
//! `idml-mutate` on the write side. The React app in `apps/devtools/`
//! consumes this crate (via `idml-introspect-wasm`).
//!
//! Three deliverables:
//!
//! 1. [`tree::build_tree`] — walk a [`Document`] into a serializable
//!    Spread → Page → Frame hierarchy the UI's tree pane renders.
//! 2. [`descriptor::describe`] — for a given [`NodeId`], list typed
//!    property descriptors (authored value + computed value + source).
//! 3. [`render_page_png`] — rasterise a single page to PNG bytes for
//!    the render pane. Behind the `render` feature so non-render
//!    consumers stay light.

pub mod descriptor;
pub mod tree;

#[cfg(feature = "render")]
pub mod render;

#[cfg(test)]
mod testutil;

pub use descriptor::{
    describe, AuthoredValue, ComputedValue, PropertyDescriptor, PropertyKind, PropertySource,
};
pub use tree::{build_tree, FrameEntry, InspectorTree, PageEntry, SpreadEntry};

#[cfg(feature = "render")]
pub use render::render_page_png;
