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

use serde::{Deserialize, Serialize};

use crate::{address, decode, Actor, Error, Hash, Kind};

/// One append-only record of a read, an export, a refusal or a call. Writes
/// need no entry: the commit is their record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEntry {
    pub seq: u64,
    pub ts: i64,
    pub actor: Actor,
    pub action: String,
    pub target: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<Hash>,
}

/// Recomputes the chain: entry `n` has `seq == n` and names entry `n-1`'s
/// address. Returns the head address.
pub fn verify_chain(entries: &[Vec<u8>]) -> Result<Option<Hash>, Error> {
    let mut prev = None;
    for (i, bytes) in entries.iter().enumerate() {
        let entry: EvidenceEntry = decode(bytes).map_err(|_| Error::EvidenceBroken(i as u64))?;
        if entry.seq != i as u64 || entry.prev != prev {
            return Err(Error::EvidenceBroken(i as u64));
        }
        prev = Some(address(Kind::Evidence, bytes));
    }
    Ok(prev)
}
