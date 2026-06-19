/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! Inspector API: read-side scene-graph introspection paired with
//! `paged-mutate` on the write side. The React app in `apps/devtools/`
//! consumes this crate (via `paged-introspect-wasm`).
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

pub mod catalog;
pub mod descriptor;
pub mod tree;

#[cfg(feature = "render")]
pub mod render;

#[cfg(test)]
mod testutil;

pub use catalog::{api_catalog, lookup_path, ApiCatalog};
pub use descriptor::{
    describe, AuthoredValue, ComputedValue, PropertyDescriptor, PropertyKind, PropertySource,
};
pub use tree::{build_tree, FrameEntry, InspectorTree, PageEntry, SpreadEntry};

#[cfg(feature = "render")]
pub use render::render_page_png;
