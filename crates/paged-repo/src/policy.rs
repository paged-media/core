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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Actor, ActorKind, Refusal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Reviewer,
    Editor,
    Owner,
}

/// How far agents may write without a human: read only, propose on `agent/…`
/// refs (the default), or commit wherever their human could.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Read,
    Propose,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub v: u32,
    pub roles: BTreeMap<String, Role>,
    pub autonomy: Autonomy,
    pub main_merge_only: bool,
}

/// The four ref families. Any other name is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    Main,
    Draft { owner: String },
    Agent { run: String },
    Release,
}

impl RefKind {
    pub fn parse(name: &str) -> Result<RefKind, Refusal> {
        let bad = || Refusal::BadRefName(name.to_string());
        if name == "main" {
            return Ok(RefKind::Main);
        }
        let mut it = name.split('/');
        let family = it.next().ok_or_else(bad)?;
        let rest: Vec<&str> = it.collect();
        if rest.iter().any(|s| s.is_empty() || *s == "." || *s == "..")
            || name
                .chars()
                .any(|c| c.is_control() || c == '\\' || c == ' ')
        {
            return Err(bad());
        }
        match (family, rest.as_slice()) {
            ("drafts", [owner, _, ..]) => Ok(RefKind::Draft {
                owner: owner.to_string(),
            }),
            ("agent", [run, _, ..]) => Ok(RefKind::Agent {
                run: run.to_string(),
            }),
            ("releases", [_, ..]) => Ok(RefKind::Release),
            _ => Err(bad()),
        }
    }
}

impl Policy {
    /// The policy of a repository that has none: the device's one human owns
    /// it, and agents propose.
    pub fn local(owner: &str) -> Policy {
        Policy {
            v: 1,
            roles: BTreeMap::from([(owner.to_string(), Role::Owner)]),
            autonomy: Autonomy::Propose,
            main_merge_only: false,
        }
    }

    /// Parses a stored policy. A policy that is present and does not parse is
    /// an error the repository keeps, and every write is then refused.
    pub fn parse(bytes: &[u8]) -> Result<Policy, Refusal> {
        let p: Policy =
            serde_json::from_slice(bytes).map_err(|e| Refusal::PolicyInvalid(e.to_string()))?;
        if p.v != 1 {
            return Err(Refusal::PolicyInvalid(format!("policy version {}", p.v)));
        }
        if !p.roles.values().any(|r| *r == Role::Owner) {
            return Err(Refusal::PolicyInvalid("no owner".into()));
        }
        Ok(p)
    }

    pub fn role(&self, id: &str) -> Option<Role> {
        self.roles.get(id).copied()
    }

    /// May `actor` (acting for `on_behalf_of` when an agent) move `name`?
    /// `fast_forward` and `via_merge` describe the move; `created`/`deleted`
    /// say whether the ref appears or disappears.
    pub fn authorize_ref(
        &self,
        actor: &Actor,
        on_behalf_of: Option<&str>,
        name: &str,
        created: bool,
        deleted: bool,
        fast_forward: bool,
        via_merge: bool,
    ) -> Result<(), Refusal> {
        let kind = RefKind::parse(name)?;
        let deny = || Refusal::NotPermitted {
            actor: actor.id.clone(),
            ref_name: name.to_string(),
        };
        let writer = |id: &str| self.role(id).is_some_and(|r| r >= Role::Editor);

        if actor.kind == ActorKind::Agent {
            let human = on_behalf_of.ok_or(Refusal::MissingProvenance)?;
            if !writer(human) || self.autonomy == Autonomy::Read {
                return Err(deny());
            }
            match kind {
                RefKind::Agent { .. } => return Ok(()),
                RefKind::Main if self.autonomy == Autonomy::Commit => {}
                _ => return Err(deny()),
            }
        } else if !writer(&actor.id) {
            return Err(deny());
        }

        match kind {
            RefKind::Release if !created => Err(Refusal::ReleaseFrozen(name.to_string())),
            RefKind::Release => Ok(()),
            RefKind::Draft { owner } if owner != actor.id => Err(deny()),
            RefKind::Draft { .. } | RefKind::Agent { .. } => Ok(()),
            RefKind::Main => {
                if deleted {
                    Err(deny())
                } else if self.main_merge_only && !via_merge && !created {
                    Err(Refusal::MergeOnly(name.to_string()))
                } else if !fast_forward && !created {
                    Err(Refusal::NotFastForward(name.to_string()))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// May `approver` approve a change authored by `author`? Never their own,
    /// never an agent, and only from reviewer up.
    pub fn authorize_approval(&self, approver: &Actor, author: &str) -> Result<(), Refusal> {
        if approver.id == author {
            return Err(Refusal::SelfApproval(approver.id.clone()));
        }
        if approver.kind == ActorKind::Agent
            || self.role(&approver.id).is_none_or(|r| r < Role::Reviewer)
        {
            return Err(Refusal::NotReviewer(approver.id.clone()));
        }
        Ok(())
    }
}
