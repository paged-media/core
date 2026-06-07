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

//! Concrete mega-file definitions. Each sub-module exposes a `build`
//! function returning a fully-populated `Sample`.

pub mod anchored;
pub mod conditions;
pub mod corners;
pub mod effects;
pub mod footnotes;
pub mod geometry;
pub mod geometry_groups;
pub mod gradients;
pub mod image_clipping;
pub mod images;
pub mod layout;
pub mod links_broken;
pub mod links_ok;
pub mod markers;
pub mod masters;
pub mod navigation;
pub mod nested_groups;
pub mod numbering;
pub mod preflight;
pub mod strokes_fills;
pub mod styles_cascade;
pub mod swatches;
pub mod tables;
pub mod text;
pub mod text_advanced;
pub mod text_autosize;
pub mod text_in_shape;
pub mod text_letterspacing;
pub mod text_on_path;
pub mod text_overset;
pub mod text_wrap;
pub mod transparency;
pub mod variables;
