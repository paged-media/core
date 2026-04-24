//! Display-list compositor.
//!
//! Walks the laid-out scene graph and emits a structured command buffer:
//! paths, fills, clips, blend state, effects. The display list is the
//! handoff format to the GPU rasterizer and is versioned so it can also
//! be used as a stable intermediate representation for tooling.
