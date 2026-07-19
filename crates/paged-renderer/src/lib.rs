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

//! Top-level renderer.
//!
//! Public Rust API. Coordinates parse → scene → text layout → compose →
//! GPU raster. Mirrors the TypeScript surface described in idea.md §14.

pub mod asset;
pub mod diagnostics;
pub mod flow;
pub mod pipeline;
pub mod resource_provider;

mod module;

pub use asset::{AssetResolver, BytesResolver};
pub use flow::{FlowLine, PlacedLine, TextFlow};
pub use diagnostics::{Diagnostic, DiagnosticCode, RenderDiagnostics, Severity};
pub use pipeline::{
    build, build_document, build_run_paint_picker, resolve_fill, resolve_stroke,
    BodyStoryEmissionDelta, BodyStoryPageDelta, BuiltDocument, BuiltPage, CellAddr, CellRect,
    ClusterPos, DateParts, DocumentClock, FontMetricsOverride, FontTable, LineLayout,
    MasterTextEmitDelta, PageId, PipelineOptions, PipelineStats, RunPaintPicker,
};
pub use resource_provider::{
    assemble_resource_tiles, mip_level_for_scale, ImageResourceProvider, ProviderTile,
    ResourcePyramid, ResourceTilesNeeded,
};

#[cfg(feature = "cpu")]
pub use pipeline::{render, render_built_page, render_document};

// Re-export Document so consumers only need one `use` for the common
// path: `use paged_renderer::{Document, pipeline, PipelineOptions};`.
pub use paged_scene::Document;

// Re-export the display-list IR so canvas crates depend on a single
// upstream and don't pull `paged-compose` directly.
pub use paged_compose::{DisplayCommand, DisplayList};
