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
// W0.5 — master application
// ---------------------------------------------------------------------------

pub(super) fn apply_master_to_page(
    doc: &mut Document,
    page: &str,
    master: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let page_ref = find_page_mut(doc, page)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(page.to_string())))?;
    let prev = page_ref.applied_master.clone();
    page_ref.applied_master = master.map(str::to_string);
    Ok(AppliedOperation {
        op: Operation::ApplyMasterToPage {
            page: page.to_string(),
            master: master.map(str::to_string),
        },
        inverse: Operation::ApplyMasterToPage {
            page: page.to_string(),
            master: prev,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

