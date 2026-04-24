//! Resolved scene graph.
//!
//! Takes the parser AST and resolves style cascades (paragraph → character →
//! local overrides), link references, master-spread inheritance, and spot
//! swatches. Output is an immutable, `Arc`-indexed tree.
