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
pub mod annual_base;
pub mod baseline_grid;
pub mod blank_paragraphs;
pub mod column_rules;
pub mod composer;
pub mod conditions;
pub mod corners;
pub mod docx_pagination;
pub mod effects;
pub mod effects_attrs;
pub mod footnote_columns;
pub mod footnote_numbering;
pub mod footnote_references;
pub mod footnotes;
pub mod forced_line_break;
pub mod geometry;
pub mod geometry_groups;
pub mod gradient_geometry;
pub mod gradients;
pub mod hard_hyphen;
pub mod image_clipping;
pub mod images;
pub mod inline_objects;
pub mod justification;
pub mod justified_breaks;
pub mod justified_probe;
pub mod keeps;
pub mod keeps_reflow;
pub mod kerning;
pub mod layers_z;
pub mod layout;
pub mod line_ends;
pub mod line_shadows;
pub mod links_broken;
pub mod links_ok;
pub mod list_marker_styles;
pub mod list_markers;
pub mod list_overrides;
pub mod markers;
pub mod masters;
pub mod mixed_leading;
pub mod navigation;
pub mod nested_groups;
pub mod nested_styles;
pub mod numbering;
pub mod numbering_multilevel;
pub mod object_style_frames;
pub mod object_style_options;
pub mod opentype;
pub mod paragraph_attrs;
pub mod paste_into;
pub mod placed_pdf;
pub mod preflight;
pub mod rect_shadows;
pub mod reflow;
pub mod shaped_bands;
pub mod showcase_base;
pub mod soft_hyphens;
pub mod span_columns;
pub mod split_boundaries;
pub mod start_paragraph;
pub mod stroke_inset;
pub mod stroke_styles;
pub mod strokes_fills;
pub mod styles_cascade;
pub mod swatches;
pub mod tab_breaks;
pub mod table_cell_baseline;
pub mod table_cell_tints;
pub mod table_inline_text;
pub mod tables;
pub mod tables_overset;
pub mod tables_rows;
pub mod text;
pub mod text_advanced;
pub mod text_autosize;
pub mod text_in_shape;
pub mod text_letterspacing;
pub mod text_on_path;
pub mod text_overset;
pub mod text_wrap;
pub mod text_wrap_sides;
pub mod tints;
pub mod transparency;
pub mod transparency_targets;
pub mod variables;

/// Every built-in sample, by CLI name.
///
/// This is the ONE list. It used to be four: the `match` arms in
/// `bin/paged-gen.rs`, the `known:` string in that match's error, the
/// `SAMPLES` array in `scripts/regen-fixtures.sh`, and a hand-copied loop
/// in the EDITOR's `tests.yml`. The copies drifted in the direction no
/// guard could see — the build fails loudly on a name that does not
/// exist, but nothing noticed a name that exists and was left OUT. Both
/// `layers-z` and `paste-into` went missing from the editor's copy that
/// way, and four `layers-panel` specs failed there with `Could not find
/// EOCD` because they fetched a fixture CI had never emitted.
pub const SAMPLES: &[&str] = &[
    "geometry",
    "geometry-groups",
    "strokes-fills",
    "text",
    "text-advanced",
    "text-autosize",
    "text-letterspacing",
    "text-on-path",
    "text-overset",
    "text-in-shape",
    "text-wrap",
    "text-wrap-sides",
    "effects",
    "footnotes",
    "gradients",
    "tables",
    "tables-overset",
    "tables-rows",
    "images",
    "image-clipping",
    "anchored",
    "inline-objects",
    "transparency",
    "markers",
    "masters",
    "corners",
    "links-broken",
    "links-ok",
    "preflight",
    "numbering",
    "variables",
    "conditions",
    "swatches",
    "navigation",
    "styles-cascade",
    "layout",
    "blank-paragraphs",
    "hard-hyphen",
    "justified-breaks",
    "mixed-leading",
    "keeps",
    "start-paragraph",
    "span-columns",
    "split-boundaries",
    "forced-line-break",
    "composer",
    "list-markers",
    "tab-breaks",
    "list-overrides",
    "list-marker-styles",
    "stroke-inset",
    "shaped-bands",
    "reflow",
    "keeps-reflow",
    "docx-pagination",
    "nested-groups",
    "paste-into",
    "layers-z",
    "soft-hyphens",
    "showcase-base",
    "annual-base",
    "line-ends",
    "line-shadows",
    "rect-shadows",
    "gradient-geometry",
    "tints",
    "table-cell-baseline",
    "table-cell-tints",
    "table-inline-text",
    "footnote-references",
    "transparency-targets",
    "effects-attrs",
    "justification",
    "kerning",
    "nested-styles",
    "opentype",
    "object-style-frames",
    "baseline-grid",
    "baseline-grid-margin",
    "object-style-options",
    "stroke-styles",
    "paragraph-attrs",
    "placed-pdf",
    "column-rules",
    "footnote-numbering",
    "footnote-restart",
    "numbering-multilevel",
    "footnote-columns",
    "footnote-straddle",
];

/// Build a sample by its CLI name, or `None` if the name is unknown.
pub fn build(name: &str) -> Option<crate::Sample> {
    Some(match name {
        "geometry" => geometry::build(),
        "geometry-groups" => geometry_groups::build(),
        "strokes-fills" => strokes_fills::build(),
        "text" => text::build(),
        "text-advanced" => text_advanced::build(),
        "text-autosize" => text_autosize::build(),
        "text-letterspacing" => text_letterspacing::build(),
        "text-on-path" => text_on_path::build(),
        "text-overset" => text_overset::build(),
        "text-in-shape" => text_in_shape::build(),
        "text-wrap" => text_wrap::build(),
        "text-wrap-sides" => text_wrap_sides::build(),
        "effects" => effects::build(),
        "footnotes" => footnotes::build(),
        "gradients" => gradients::build(),
        "tables" => tables::build(),
        "tables-overset" => tables_overset::build(),
        "tables-rows" => tables_rows::build(),
        "images" => images::build(),
        "image-clipping" => image_clipping::build(),
        "anchored" => anchored::build(),
        "inline-objects" => inline_objects::build(),
        "transparency" => transparency::build(),
        "markers" => markers::build(),
        "masters" => masters::build(),
        "corners" => corners::build(),
        "links-broken" => links_broken::build(),
        "links-ok" => links_ok::build(),
        "preflight" => preflight::build(),
        "numbering" => numbering::build(),
        "variables" => variables::build(),
        "conditions" => conditions::build(),
        "swatches" => swatches::build(),
        "navigation" => navigation::build(),
        "styles-cascade" => styles_cascade::build(),
        "layout" => layout::build(),
        "blank-paragraphs" => blank_paragraphs::build(),
        "hard-hyphen" => hard_hyphen::build(),
        "justified-breaks" => justified_breaks::build(),
        "justified-probe" => justified_probe::build(),
        "mixed-leading" => mixed_leading::build(),
        "keeps" => keeps::build(),
        "start-paragraph" => start_paragraph::build(),
        "span-columns" => span_columns::build(),
        "split-boundaries" => split_boundaries::build(),
        "forced-line-break" => forced_line_break::build(),
        "composer" => composer::build(),
        "list-markers" => list_markers::build(),
        "tab-breaks" => tab_breaks::build(),
        "list-overrides" => list_overrides::build(),
        "list-marker-styles" => list_marker_styles::build(),
        "stroke-inset" => stroke_inset::build(),
        "shaped-bands" => shaped_bands::build(),
        "reflow" => reflow::build(),
        "keeps-reflow" => keeps_reflow::build(),
        "docx-pagination" => docx_pagination::build(),
        "nested-groups" => nested_groups::build(),
        "paste-into" => paste_into::build(),
        "layers-z" => layers_z::build(),
        "soft-hyphens" => soft_hyphens::build(),
        "showcase-base" => showcase_base::build(),
        "annual-base" => annual_base::build(),
        "line-ends" => line_ends::build(),
        "stroke-styles" => stroke_styles::build(),
        "paragraph-attrs" => paragraph_attrs::build(),
        "placed-pdf" => placed_pdf::build(),
        "column-rules" => column_rules::build(),
        "footnote-numbering" => footnote_numbering::build(&footnote_numbering::NUMBERING),
        "footnote-restart" => footnote_numbering::build(&footnote_numbering::RESTART),
        "numbering-multilevel" => numbering_multilevel::build(),
        "footnote-columns" => footnote_columns::build(&footnote_columns::COLUMNS),
        "footnote-straddle" => footnote_columns::build(&footnote_columns::STRADDLE),
        "line-shadows" => line_shadows::build(),
        "rect-shadows" => rect_shadows::build(),
        "gradient-geometry" => gradient_geometry::build(),
        "tints" => tints::build(),
        "table-cell-baseline" => table_cell_baseline::build(),
        "table-cell-tints" => table_cell_tints::build(),
        "table-inline-text" => table_inline_text::build(),
        "footnote-references" => footnote_references::build(),
        "transparency-targets" => transparency_targets::build(),
        "effects-attrs" => effects_attrs::build(),
        "justification" => justification::build(),
        "kerning" => kerning::build(),
        "nested-styles" => nested_styles::build(),
        "opentype" => opentype::build(),
        "object-style-frames" => object_style_frames::build(),
        "baseline-grid" => baseline_grid::build(),
        "baseline-grid-margin" => baseline_grid::build_margin(),
        "object-style-options" => object_style_options::build(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direction the old four-way duplication could never check: a
    /// name listed in `SAMPLES` that no arm builds.
    #[test]
    fn every_listed_sample_builds() {
        for name in SAMPLES {
            assert!(
                build(name).is_some(),
                "SAMPLES lists {name:?} but samples::build has no arm for it"
            );
        }
    }

    /// …and the other direction, so adding an arm without listing it is
    /// caught too. Counted rather than reflected over (Rust cannot
    /// enumerate match arms), so this is a deliberate shrink-only pin:
    /// bump it in the same commit that adds the sample TO `SAMPLES`.
    #[test]
    fn the_list_is_not_missing_a_sample() {
        assert_eq!(
            SAMPLES.len(),
            89,
            "sample count changed — add the new name to SAMPLES (and only then \
             update this number), or the editor's CI silently stops emitting it"
        );
    }
}
