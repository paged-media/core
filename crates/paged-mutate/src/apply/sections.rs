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

use super::*;
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, Operation,
};

// ---------------------------------------------------------------------------
// W0.5 — sections
// ---------------------------------------------------------------------------

/// Map a parsed `NumberingStyle` back to its IDML `PageNumberStyle`
/// attribute spelling so an inverse op round-trips through
/// `NumberingStyle::from_idml`. (`NumberingStyle::as_str` yields the
/// editor's lower-camel wire name, which `from_idml` does NOT accept.)
pub(super) fn numbering_style_to_idml(s: paged_parse::NumberingStyle) -> &'static str {
    use paged_parse::NumberingStyle::*;
    match s {
        Arabic => "Arabic",
        UpperRoman => "UpperRoman",
        LowerRoman => "LowerRoman",
        UpperAlpha => "UpperLetters",
        LowerAlpha => "LowerLetters",
    }
}

pub(super) fn apply_insert_section(
    doc: &mut Document,
    at_page: &str,
    prefix: Option<String>,
    numbering_style: Option<String>,
    start_at: Option<u32>,
    self_id: Option<String>,
) -> Result<AppliedOperation, OperationError> {
    // The anchor page must exist.
    if find_page_mut(doc, at_page).is_none() {
        return Err(OperationError::NodeNotFound(NodeId::Page(
            at_page.to_string(),
        )));
    }
    let sections = &mut doc.container.designmap.sections;
    let id = match self_id {
        Some(id) => id,
        None => {
            // Deterministic non-colliding `Section/u<n>`.
            let mut n = sections.len();
            let mut id = format!("Section/u{n}");
            while sections.iter().any(|s| s.self_id == id) {
                n += 1;
                id = format!("Section/u{n}");
            }
            id
        }
    };
    if sections.iter().any(|s| s.self_id == id) {
        return Err(OperationError::DuplicateNodeId { id });
    }
    let section = paged_parse::Section {
        self_id: id.clone(),
        page_start: Some(at_page.to_string()),
        continue_numbering: false,
        start_at,
        numbering_style: numbering_style
            .as_deref()
            .map(paged_parse::NumberingStyle::from_idml)
            .unwrap_or(paged_parse::NumberingStyle::Arabic),
        section_prefix: prefix.clone(),
        marker: None,
        include_prefix: prefix.is_some(),
    };
    sections.push(section);
    Ok(AppliedOperation {
        op: Operation::InsertSection {
            at_page: at_page.to_string(),
            prefix,
            numbering_style,
            start_at,
            self_id: Some(id.clone()),
        },
        inverse: Operation::DeleteSection { section_id: id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_edit_section(
    doc: &mut Document,
    section_id: &str,
    prefix: Option<Option<String>>,
    numbering_style: Option<String>,
    start_at: Option<Option<u32>>,
) -> Result<AppliedOperation, OperationError> {
    let sections = &mut doc.container.designmap.sections;
    let section = sections
        .iter_mut()
        .find(|s| s.self_id == section_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "sections".to_string(),
            id: section_id.to_string(),
        })?;
    // Capture prior values for the inverse.
    let prev_prefix = section.section_prefix.clone();
    let prev_style = numbering_style_to_idml(section.numbering_style).to_string();
    let prev_start = section.start_at;

    if let Some(p) = &prefix {
        section.section_prefix = p.clone();
        section.include_prefix = p.is_some();
    }
    if let Some(style) = &numbering_style {
        section.numbering_style = paged_parse::NumberingStyle::from_idml(style);
    }
    if let Some(s) = &start_at {
        section.start_at = *s;
    }

    Ok(AppliedOperation {
        op: Operation::EditSection {
            section_id: section_id.to_string(),
            prefix,
            numbering_style,
            start_at,
        },
        inverse: Operation::EditSection {
            section_id: section_id.to_string(),
            prefix: Some(prev_prefix),
            numbering_style: Some(prev_style),
            start_at: Some(prev_start),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_delete_section(
    doc: &mut Document,
    section_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let sections = &mut doc.container.designmap.sections;
    let pos = sections
        .iter()
        .position(|s| s.self_id == section_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "sections".to_string(),
            id: section_id.to_string(),
        })?;
    let removed = sections.remove(pos);
    let style_str = numbering_style_to_idml(removed.numbering_style).to_string();
    let at_page = removed.page_start.clone().unwrap_or_default();
    Ok(AppliedOperation {
        op: Operation::DeleteSection {
            section_id: section_id.to_string(),
        },
        inverse: Operation::InsertSection {
            at_page,
            prefix: removed.section_prefix.clone(),
            numbering_style: Some(style_str),
            start_at: removed.start_at,
            self_id: Some(removed.self_id.clone()),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

