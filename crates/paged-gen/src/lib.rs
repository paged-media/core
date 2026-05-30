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

//! IDML sample-corpus generator.
//!
//! Produces deterministic IDML packages for the renderer's diff
//! harness. Each emitted `.idml` is a multi-page document whose pages
//! each exercise one renderable feature variant — failure attribution
//! comes from per-page heatmaps + `Page.Name` carrying the variant
//! descriptor, so a single InDesign export covers many test cases.
//!
//! See `docs/idml-sample-generator.md` for the strategic argument and
//! `crates/paged-gen/src/samples/` for the concrete sample definitions.

pub mod geometry;
pub mod ids;
pub mod package;
pub mod xml;

pub mod builders;
pub mod samples;

pub use package::{write_idml, Sample};
