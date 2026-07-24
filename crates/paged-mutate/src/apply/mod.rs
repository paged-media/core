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

//! `apply(doc, op)` — the only function that mutates a [`Document`].
//!
//! Each variant of [`Operation`] dispatches to a small per-variant
//! helper. The helper captures the "before" state, performs the
//! mutation in place on the parse-layer structs, and hands the
//! captured pieces to the [`invert`](crate::invert) helpers to build
//! the matching inverse op. The result is bundled into an
//! [`AppliedOperation`] along with an [`InvalidationHint`].
//!
//! Batch atomicity: if any child fails, every previously-applied
//! child is rolled back by applying its inverse in reverse order
//! *before* `apply` returns the error. The document is then in the
//! state it was in before the batch began. The error carries the
//! index that failed.
//!
//! Stage 1 limitations (flagged in `docs/paged/scripting-layer.md`'s
//! Stage-1 deliverables):
//!   - `Document`'s pre-built indices (`text_frame_index`,
//!     `frame_for_story`) are not surgically maintained — they're
//!     valid for the unmutated open, and consumers that want them
//!     fresh after Insert/Remove/Move should rebuild via
//!     `Document::open` or a future `rebuild_indices` helper. The
//!     parse-layer leaf data is the source of truth.
//!   - `InsertNode`/`RemoveNode`/`MoveNode` support TextFrame and
//!     Rectangle children under a Spread parent. Group nesting,
//!     Page-level routing, and the other shape kinds (Oval, Polygon,
//!     GraphicLine) come as later stages.

use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, NodeId, Operation, PathAnchorSpec, PathfinderKind, PropertyPath, Value,
};
use crate::pathfinder::{pathfinder_boolean, PathfinderKind as InternalPathfinderKind};

/// Apply an operation to `doc`. Returns the captured `AppliedOperation`
/// (carrying op + inverse + invalidation hint) on success. The only
/// mutation entry point in the crate.
pub fn apply(doc: &mut Document, op: &Operation) -> Result<AppliedOperation, OperationError> {
    match op {
        Operation::SetProperty { node, path, value } => apply_set_property(doc, node, *path, value),
        Operation::InsertNode {
            parent,
            position,
            node,
            z_slot,
        } => apply_insert_node(doc, parent, *position, *z_slot, node),
        Operation::RemoveNode { node } => apply_remove_node(doc, node),
        Operation::MoveNode {
            node,
            new_parent,
            position,
        } => apply_move_node(doc, node, new_parent, *position),
        Operation::Batch { ops } => apply_batch(doc, ops),
        Operation::InsertPage {
            after_page_id,
            master_id,
            spread_self_id,
            page_self_id,
            restore_spread_json,
        } => apply_insert_page(
            doc,
            after_page_id.as_deref(),
            master_id.as_deref(),
            spread_self_id.clone(),
            page_self_id.clone(),
            restore_spread_json.as_deref(),
        ),
        Operation::RemovePage { page_id } => apply_remove_page(doc, page_id),
        Operation::MoveLayer {
            layer_id,
            new_index,
        } => apply_move_layer(doc, layer_id, *new_index),
        Operation::InsertLayer {
            position,
            name,
            self_id,
        } => apply_insert_layer(doc, *position, name, self_id.as_deref()),
        Operation::RemoveLayer { layer_id } => apply_remove_layer(doc, layer_id),
        Operation::CreateSwatch { spec } => apply_create_swatch(doc, spec),
        Operation::EditSwatch { swatch_id, spec } => apply_edit_swatch(doc, swatch_id, spec),
        Operation::DeleteSwatch { swatch_id } => apply_delete_swatch(doc, swatch_id),
        Operation::CreateParagraphStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_paragraph_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameParagraphStyle { style_id, name } => {
            apply_rename_paragraph_style(doc, style_id, name)
        }
        Operation::DeleteParagraphStyle { style_id } => apply_delete_paragraph_style(doc, style_id),
        Operation::CreateCharacterStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_character_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameCharacterStyle { style_id, name } => {
            apply_rename_character_style(doc, style_id, name)
        }
        Operation::DeleteCharacterStyle { style_id } => apply_delete_character_style(doc, style_id),
        Operation::CreateObjectStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_object_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameObjectStyle { style_id, name } => {
            apply_rename_object_style(doc, style_id, name)
        }
        Operation::DeleteObjectStyle { style_id } => apply_delete_object_style(doc, style_id),
        Operation::CreateCellStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_cell_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameCellStyle { style_id, name } => {
            apply_rename_cell_style(doc, style_id, name)
        }
        Operation::DeleteCellStyle { style_id } => apply_delete_cell_style(doc, style_id),
        Operation::CreateTableStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_table_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameTableStyle { style_id, name } => {
            apply_rename_table_style(doc, style_id, name)
        }
        Operation::DeleteTableStyle { style_id } => apply_delete_table_style(doc, style_id),
        Operation::CreateGroup { spec } => apply_create_group(doc, spec),
        Operation::DissolveGroup {
            group_id,
            restore_slots,
        } => apply_dissolve_group(doc, group_id, restore_slots.as_deref()),
        Operation::SetGroupTransform {
            group,
            transform,
            prev,
        } => apply_set_group_transform(doc, group, *transform, *prev),
        Operation::CreateGradient { spec } => apply_create_gradient(doc, spec),
        Operation::EditGradient { gradient_id, spec } => {
            apply_edit_gradient(doc, gradient_id, spec)
        }
        Operation::DeleteGradient { gradient_id } => apply_delete_gradient(doc, gradient_id),
        Operation::CreateColorGroup { spec } => apply_create_color_group(doc, spec),
        Operation::EditColorGroup { group_id, spec } => apply_edit_color_group(doc, group_id, spec),
        Operation::DeleteColorGroup { group_id } => apply_delete_color_group(doc, group_id),
        Operation::CreateNumberingList { spec } => apply_create_numbering_list(doc, spec),
        Operation::EditNumberingList { list_id, spec } => {
            apply_edit_numbering_list(doc, list_id, spec)
        }
        Operation::DeleteNumberingList { list_id } => apply_delete_numbering_list(doc, list_id),
        Operation::SetStyleProperty {
            collection,
            style_id,
            path,
            value,
        } => apply_set_style_property(doc, *collection, style_id, *path, value),
        Operation::PathfinderBoolean {
            kept,
            others,
            op_kind,
        } => apply_pathfinder(doc, kept, others, *op_kind),
        Operation::LinkFrames { from, to } => apply_link_frames(doc, from, to),
        Operation::UnlinkFrames { frame, prev_next } => {
            apply_unlink_frames(doc, frame, prev_next.as_deref())
        }
        Operation::ApplyStyle {
            story_id,
            start,
            end,
            style,
            scope,
        } => apply_apply_style(doc, story_id, *start, *end, style, *scope),
        Operation::InsertField {
            story_id,
            offset,
            field,
        } => apply_insert_field(doc, story_id, *offset, field),
        Operation::DeleteField {
            story_id,
            offset,
            field,
        } => apply_delete_field(doc, story_id, *offset, field),
        Operation::InsertAnchoredFrame {
            story_id,
            offset,
            width,
            height,
            image_uri,
            self_id,
        } => anchored_frame::apply_insert_anchored_frame(
            doc,
            story_id,
            *offset,
            *width,
            *height,
            image_uri.as_deref(),
            self_id,
        ),
        Operation::RemoveAnchoredFrame { story_id, self_id } => {
            anchored_frame::apply_remove_anchored_frame(doc, story_id, self_id)
        }
        Operation::SetFieldValue {
            story_id,
            offset,
            value,
        } => apply_set_field_value(doc, story_id, *offset, value.as_deref()),
        Operation::PlaceImage {
            frame,
            image_uri,
            fit,
        } => place_image::apply_place_image(doc, frame, image_uri.as_deref(), fit.as_deref()),
        Operation::ReplaceImageBytes {
            frame,
            bytes,
            prior_has_image_element,
        } => replace_image_bytes::apply_replace_image_bytes(
            doc,
            frame,
            bytes.as_deref(),
            *prior_has_image_element,
        ),
        Operation::InsertGuide {
            spread_id,
            orientation,
            position,
            page_index,
            guide_id,
        } => apply_insert_guide(
            doc,
            spread_id,
            *orientation,
            *position,
            *page_index,
            guide_id.clone(),
        ),
        Operation::MoveGuide { guide_id, position } => apply_move_guide(doc, guide_id, *position),
        Operation::DeleteGuide { guide_id } => apply_delete_guide(doc, guide_id),
        Operation::SetConditionVisible { condition, visible } => {
            apply_set_condition_visible(doc, condition, *visible)
        }
        Operation::ActivateConditionSet { set } => apply_activate_condition_set(doc, set),
        Operation::RestoreConditionVisibility { states } => {
            apply_restore_condition_visibility(doc, states)
        }
        Operation::ApplyMasterToPage { page, master } => {
            apply_master_to_page(doc, page, master.as_deref())
        }
        Operation::DuplicatePage {
            page,
            clone_spread_json,
        } => apply_duplicate_page(doc, page, clone_spread_json.as_deref()),
        Operation::InsertSection {
            at_page,
            prefix,
            numbering_style,
            start_at,
            self_id,
        } => apply_insert_section(
            doc,
            at_page,
            prefix.clone(),
            numbering_style.clone(),
            *start_at,
            self_id.clone(),
        ),
        Operation::EditSection {
            section_id,
            prefix,
            numbering_style,
            start_at,
        } => apply_edit_section(
            doc,
            section_id,
            prefix.clone(),
            numbering_style.clone(),
            *start_at,
        ),
        Operation::DeleteSection { section_id } => apply_delete_section(doc, section_id),
        Operation::SetRowHeight {
            story_id,
            table_id,
            row,
            height,
        } => apply_set_row_height(doc, story_id, table_id, *row, *height),
        Operation::SetColumnWidth {
            story_id,
            table_id,
            col,
            width,
        } => apply_set_column_width(doc, story_id, table_id, *col, *width),
        Operation::InsertTableRow {
            story_id,
            table_id,
            at,
            restore,
        } => apply_insert_table_row(doc, story_id, table_id, *at, restore.as_deref()),
        Operation::DeleteTableRow {
            story_id,
            table_id,
            at,
        } => apply_delete_table_row(doc, story_id, table_id, *at),
        Operation::InsertTableColumn {
            story_id,
            table_id,
            at,
            restore,
        } => apply_insert_table_column(doc, story_id, table_id, *at, restore.as_deref()),
        Operation::DeleteTableColumn {
            story_id,
            table_id,
            at,
        } => apply_delete_table_column(doc, story_id, table_id, *at),
        Operation::InsertHeaderRow {
            story_id,
            table_id,
            restore,
        } => apply_insert_band_row(
            doc,
            story_id,
            table_id,
            TableBand::Header,
            restore.as_deref(),
        ),
        Operation::RemoveHeaderRow { story_id, table_id } => {
            apply_remove_band_row(doc, story_id, table_id, TableBand::Header)
        }
        Operation::InsertFooterRow {
            story_id,
            table_id,
            restore,
        } => apply_insert_band_row(
            doc,
            story_id,
            table_id,
            TableBand::Footer,
            restore.as_deref(),
        ),
        Operation::RemoveFooterRow { story_id, table_id } => {
            apply_remove_band_row(doc, story_id, table_id, TableBand::Footer)
        }
        Operation::SetCellSpan {
            story_id,
            table_id,
            row,
            col,
            row_span,
            column_span,
        } => apply_set_cell_span(doc, story_id, table_id, *row, *col, *row_span, *column_span),
    }
}

/// SDK Phase 5 (v1 sweep) — apply a multi-target Pathfinder
/// boolean. Reads every input's path, runs flo_curves CSG via
/// `pathfinder::pathfinder_boolean`, then builds + applies an
/// internal Batch (FramePath on kept + RemoveNode for each other).
/// The returned AppliedOperation is the Batch — undo reverses
/// everything in one Cmd-Z. Inverse restoration of the removed
/// frames goes through the same path RemoveNode normally takes
/// (specs captured at remove time).
fn apply_pathfinder(
    doc: &mut Document,
    kept: &NodeId,
    others: &[NodeId],
    kind: PathfinderKind,
) -> Result<AppliedOperation, OperationError> {
    // 1. Snapshot every input's path. find_path_anchors_mut also
    //    handles the "no anchors → bounds rectangle" fallback in
    //    its callers; for Pathfinder we read anchors directly
    //    since the result IS a path replacement.
    let mut inputs: Vec<(Vec<paged_model::PathAnchor>, Vec<usize>)> =
        Vec::with_capacity(1 + others.len());
    for node in std::iter::once(kept).chain(others.iter()) {
        let (anchors, starts) = read_path(doc, node)?;
        inputs.push((anchors, starts));
    }
    // 2. Run the boolean.
    let internal_kind = match kind {
        PathfinderKind::Union => InternalPathfinderKind::Union,
        PathfinderKind::Intersect => InternalPathfinderKind::Intersect,
        PathfinderKind::Subtract => InternalPathfinderKind::Subtract,
        PathfinderKind::Exclude => InternalPathfinderKind::Exclude,
    };
    let (result_anchors, result_starts) = pathfinder_boolean(&inputs, internal_kind);
    // 3. Build the inner Batch.
    let result_spec_anchors: Vec<PathAnchorSpec> = result_anchors
        .iter()
        .map(PathAnchorSpec::from_parse)
        .collect();
    let mut batch_children: Vec<Operation> = Vec::with_capacity(1 + others.len());
    batch_children.push(Operation::SetProperty {
        node: kept.clone(),
        path: PropertyPath::FramePath,
        value: Value::FramePath {
            anchors: result_spec_anchors,
            subpath_starts: result_starts,
        },
    });
    for other in others {
        batch_children.push(Operation::RemoveNode {
            node: other.clone(),
        });
    }
    let batch = Operation::Batch {
        ops: batch_children,
    };
    // 4. Apply the Batch — the existing `apply_batch` machinery
    //    rolls back on any child failure and produces the inverse.
    let applied = apply(doc, &batch)?;
    // The op we record is the original PathfinderBoolean (so the
    // forward op is meaningful in logs); the inverse is the
    // Batch's inverse (which restores the removed frames + the
    // kept frame's prior path).
    Ok(AppliedOperation {
        op: Operation::PathfinderBoolean {
            kept: kept.clone(),
            others: others.to_vec(),
            op_kind: kind,
        },
        inverse: applied.inverse,
        invalidation: applied.invalidation,
    })
}

/// SDK Phase 5 (v1 sweep) — read a frame's full path. Returns
/// the anchors + subpath_starts as a cloned snapshot. When the
/// frame has no explicit anchors (a Rectangle declared via
/// `GeometricBounds` alone), synthesises a four-corner closed
/// path from the bounds so Pathfinder still has geometry to
/// operate on.
fn read_path(
    doc: &Document,
    node: &NodeId,
) -> Result<(Vec<paged_model::PathAnchor>, Vec<usize>), OperationError> {
    use paged_model::PathAnchor;
    let raw = node.self_id();
    for parsed in &doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(f) = parsed
                    .spread
                    .text_frames
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if !f.anchors.is_empty() {
                        return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                    }
                    return Ok((rect_anchors_from_bounds(f.bounds), vec![0]));
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(f) = parsed
                    .spread
                    .rectangles
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if !f.anchors.is_empty() {
                        return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                    }
                    return Ok((rect_anchors_from_bounds(f.bounds), vec![0]));
                }
            }
            NodeId::Oval(_) => {
                if let Some(f) = parsed
                    .spread
                    .ovals
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    // Oval's parse layer doesn't expose an
                    // explicit anchors vec — synthesise a four-
                    // corner approximation from bounds for now.
                    // A future polish emits the proper four-arc
                    // ellipse anchors.
                    return Ok((rect_anchors_from_bounds(f.bounds), vec![0]));
                }
            }
            NodeId::Polygon(_) => {
                if let Some(f) = parsed
                    .spread
                    .polygons
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(f) = parsed
                    .spread
                    .graphic_lines
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                }
            }
            _ => {}
        }
    }
    let _ = PathAnchor {
        anchor: (0.0, 0.0),
        left: (0.0, 0.0),
        right: (0.0, 0.0),
    };
    Err(OperationError::NodeNotFound(node.clone()))
}

fn rect_anchors_from_bounds(b: paged_model::Bounds) -> Vec<paged_model::PathAnchor> {
    use paged_model::PathAnchor;
    let (t, l, r, btm) = (b.top, b.left, b.right, b.bottom);
    let corner = |x: f32, y: f32| PathAnchor {
        anchor: (x, y),
        left: (x, y),
        right: (x, y),
    };
    vec![corner(l, t), corner(r, t), corner(r, btm), corner(l, btm)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(text: &str) -> paged_model::CharacterRun {
        paged_model::CharacterRun {
            text: text.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn split_run_at_partitions_by_char_index() {
        let (l, r) = split_run_at(run_with("hello"), 2);
        assert_eq!(l.text, "he");
        assert_eq!(r.text, "llo");
    }

    #[test]
    fn split_run_at_zero_keeps_all_on_the_right() {
        let (l, r) = split_run_at(run_with("hello"), 0);
        assert_eq!(l.text, "");
        assert_eq!(r.text, "hello");
    }

    #[test]
    fn split_run_at_or_past_end_keeps_all_on_the_left() {
        let (l, r) = split_run_at(run_with("hi"), 2);
        assert_eq!(l.text, "hi");
        assert_eq!(r.text, "");
        // char_idx beyond the char count clamps to the byte length.
        let (l2, r2) = split_run_at(run_with("hi"), 99);
        assert_eq!(l2.text, "hi");
        assert_eq!(r2.text, "");
    }

    #[test]
    fn split_run_at_respects_multibyte_char_boundaries() {
        // "é" + "🚀" are 2- and 4-byte chars; splitting at char 1 must
        // land on a valid UTF-8 boundary, not mid-codepoint.
        let (l, r) = split_run_at(run_with("é🚀x"), 1);
        assert_eq!(l.text, "é");
        assert_eq!(r.text, "🚀x");
    }
}

// ── module decomposition (audit 2026-06-11, 1.6 / 01 A2) ──────────────
// apply.rs grew past 11k lines; split into per-domain submodules. The
// glob imports preserve the former flat namespace (every helper still
// calls every other helper) — purely a file-layout change, net-zero
// behaviour. The named re-export keeps `crate::apply::new_*` stable for
// lib.rs's callers.
mod anchored_frame;
mod batch_page;
mod character;
mod conditions;
mod duplicate_page;
mod guides;
mod helpers;
mod insert_node;
mod layer;
mod master;
mod move_node;
mod paragraph;
mod path_topology;
mod place_image;
mod remove_node;
mod replace_image_bytes;
mod sections;
mod set_property;

use batch_page::*;
use character::*;
use conditions::*;
use duplicate_page::*;
use guides::*;
use helpers::*;
use insert_node::*;
use layer::*;
use master::*;
use move_node::*;
use paragraph::*;
use path_topology::*;
use remove_node::*;
use sections::*;
use set_property::*;

pub(crate) use path_topology::{new_oval, new_rectangle, new_text_frame};
