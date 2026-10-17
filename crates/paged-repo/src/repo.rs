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

use std::collections::{BTreeMap, BTreeSet};

use crate::merge::{is_ancestor, read_commit};
use crate::package::{read_tree, Bundle};
use crate::store::read;
use crate::sync::reachable;
use crate::{
    address, bundle_parts, decode, encode, import, merge_base, merge_trees, project, verify_chain,
    Actor, ActorKind, Commit, Error, EvidenceEntry, Hash, ImportedFrom, Kind, Policy, Provenance,
    Refusal, Store, Tree,
};

/// Who is acting, when, and for whom if an agent. The host supplies the clock.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub actor: Actor,
    pub ts: i64,
    pub provenance: Option<Provenance>,
}

impl Ctx {
    pub fn human(id: &str, ts: i64) -> Ctx {
        Ctx {
            actor: Actor::human(id),
            ts,
            provenance: None,
        }
    }

    fn on_behalf_of(&self) -> Option<&str> {
        self.provenance.as_ref().map(|p| p.on_behalf_of.as_str())
    }
}

/// A repository over a host's store, with the governance checks every host
/// shares. A policy that failed to load is kept as the refusal it produced, so
/// every write is refused rather than allowed.
pub struct Repo<S: Store> {
    pub store: S,
    policy: Result<Policy, Refusal>,
}

type Objects = BTreeMap<Hash, (Kind, Vec<u8>)>;

impl<S: Store> Repo<S> {
    pub fn new(store: S, policy: Result<Policy, Refusal>) -> Repo<S> {
        Repo { store, policy }
    }

    pub fn policy(&self) -> Result<&Policy, Refusal> {
        self.policy.as_ref().map_err(Clone::clone)
    }

    /// Appends an evidence entry and returns its address.
    pub fn record(&mut self, ctx: &Ctx, action: &str, target: &str, result: &str) -> Hash {
        let entries = self.store.evidence();
        let entry = EvidenceEntry {
            seq: entries.len() as u64,
            ts: ctx.ts,
            actor: ctx.actor.clone(),
            action: action.to_string(),
            target: target.to_string(),
            result: result.to_string(),
            prev: entries.last().map(|b| address(Kind::Evidence, b)),
        };
        let bytes = encode(&entry);
        let h = address(Kind::Evidence, &bytes);
        self.store.append_evidence(bytes);
        h
    }

    pub fn verify_evidence(&self) -> Result<Option<Hash>, Error> {
        verify_chain(&self.store.evidence())
    }

    fn refuse<T>(&mut self, ctx: &Ctx, action: &str, target: &str, r: Refusal) -> Result<T, Error> {
        self.record(ctx, action, target, &format!("refused: {r}"));
        Err(r.into())
    }

    /// Stores objects after checking each against its address.
    fn ingest(&mut self, objects: Objects) -> Result<(), Error> {
        for (h, (kind, bytes)) in &objects {
            if address(*kind, bytes) != *h {
                return Err(Error::Tampered(*h));
            }
        }
        for (h, (_, bytes)) in objects {
            self.store.put(h, bytes);
        }
        Ok(())
    }

    /// The only way a ref moves: policy, then compare-and-set.
    fn update_ref(
        &mut self,
        name: &str,
        expected: Option<Hash>,
        new: Option<Hash>,
        ctx: &Ctx,
        via_merge: bool,
    ) -> Result<(), Error> {
        let fast_forward = match (expected, new) {
            (Some(old), Some(new)) => is_ancestor(&self.store, old, new)?,
            _ => true,
        };
        let verdict = self.policy().and_then(|p| {
            p.authorize_ref(
                &ctx.actor,
                ctx.on_behalf_of(),
                name,
                expected.is_none(),
                new.is_none(),
                fast_forward,
                via_merge,
            )
        });
        if let Err(r) = verdict {
            return self.refuse(ctx, "move-ref", name, r);
        }
        self.store.cas_ref(name, expected, new)
    }

    /// Checks commits arriving from `ctx` that the store did not have before:
    /// agent commits carry provenance, others do not, and an agent pushes only
    /// its own commits.
    fn check_new_commits(&self, ctx: &Ctx, commits: &[Commit]) -> Result<(), Refusal> {
        for c in commits {
            match (c.author.kind == ActorKind::Agent, c.provenance.is_some()) {
                (true, false) => return Err(Refusal::MissingProvenance),
                (false, true) => return Err(Refusal::UnexpectedProvenance),
                _ => {}
            }
            if ctx.actor.kind == ActorKind::Agent && c.author != ctx.actor {
                return Err(Refusal::AuthorMismatch);
            }
        }
        Ok(())
    }

    /// Accepts pushed objects and moves `ref_name`. Everything the new head
    /// reaches must be present once the objects are stored.
    pub fn receive(
        &mut self,
        objects: Objects,
        ref_name: &str,
        expected: Option<Hash>,
        new: Option<Hash>,
        ctx: &Ctx,
        via_merge: bool,
    ) -> Result<(), Error> {
        let mut fresh = Vec::new();
        for (h, (kind, bytes)) in &objects {
            if *kind == Kind::Commit && !self.store.has(h) {
                fresh.push(decode::<Commit>(bytes)?);
            }
        }
        if let Err(r) = self.check_new_commits(ctx, &fresh) {
            return self.refuse(ctx, "receive", ref_name, r);
        }
        self.ingest(objects)?;
        if let Some(head) = new {
            for (kind, h) in reachable(&self.store, &[head], |_| false)? {
                read(&self.store, kind, &h)?;
            }
        }
        self.update_ref(ref_name, expected, new, ctx, via_merge)
    }

    /// Commits `tree` (with the objects it needs) on top of `ref_name`.
    pub fn commit(
        &mut self,
        objects: Objects,
        tree: Hash,
        ref_name: &str,
        ctx: &Ctx,
        message: &str,
        imported_from: Option<ImportedFrom>,
    ) -> Result<Hash, Error> {
        let parent = self.store.get_ref(ref_name);
        let commit = Commit {
            v: 1,
            parents: parent.into_iter().collect(),
            tree,
            author: ctx.actor.clone(),
            message: message.to_string(),
            ts: ctx.ts,
            provenance: ctx.provenance.clone(),
            imported_from,
        };
        let bytes = encode(&commit);
        let h = address(Kind::Commit, &bytes);
        let mut objects = objects;
        objects.insert(h, (Kind::Commit, bytes));
        self.receive(objects, ref_name, parent, Some(h), ctx, false)?;
        Ok(h)
    }

    /// Opens a file. A file that carries history restores it into an empty
    /// repository; otherwise its tree is committed on `ref_name`, recording
    /// where it came from.
    pub fn import(
        &mut self,
        file: &[u8],
        name: &str,
        ref_name: &str,
        ctx: &Ctx,
    ) -> Result<Hash, Error> {
        let snap = import(file, name)?;
        if let Some(history) = snap.history {
            if self.store.refs().is_empty() && self.store.evidence().is_empty() {
                return self.restore(history, ref_name);
            }
        }
        self.commit(
            snap.objects,
            snap.tree,
            ref_name,
            ctx,
            &format!("Open {name}"),
            Some(snap.imported_from),
        )
    }

    fn restore(&mut self, history: Bundle, ref_name: &str) -> Result<Hash, Error> {
        verify_chain(&history.evidence)?;
        self.ingest(history.objects)?;
        for (name, head) in &history.refs {
            for (kind, h) in reachable(&self.store, &[*head], |_| false)? {
                read(&self.store, kind, &h)?;
            }
            self.store.cas_ref(name, None, Some(*head))?;
        }
        for e in history.evidence {
            self.store.append_evidence(e);
        }
        self.store
            .get_ref(ref_name)
            .ok_or_else(|| Error::Package(format!("history has no `{ref_name}`")))
    }

    pub fn head(&self, ref_name: &str) -> Option<Hash> {
        self.store.get_ref(ref_name)
    }

    pub fn read_commit(&self, hash: &Hash) -> Result<Commit, Error> {
        read_commit(&self.store, hash)
    }

    pub fn read_tree(&self, hash: &Hash) -> Result<Tree, Error> {
        read_tree(&self.store, hash)
    }

    /// First-parent history of `ref_name`, newest first.
    pub fn log(&self, ref_name: &str) -> Result<Vec<(Hash, Commit)>, Error> {
        let mut out = Vec::new();
        let mut next = self.store.get_ref(ref_name);
        while let Some(h) = next {
            let c = read_commit(&self.store, &h)?;
            next = c.parents.first().copied();
            out.push((h, c));
        }
        Ok(out)
    }

    /// Creates `to` at the head of `from`.
    pub fn branch(&mut self, from: &str, to: &str, ctx: &Ctx) -> Result<Hash, Error> {
        let head = self.store.get_ref(from).ok_or_else(|| Error::RefConflict {
            name: from.to_string(),
            expected: None,
            found: None,
        })?;
        self.update_ref(to, None, Some(head), ctx, false)?;
        Ok(head)
    }

    /// Creates `releases/<name>` at `commit`. It never moves afterwards.
    pub fn release(&mut self, name: &str, commit: Hash, ctx: &Ctx) -> Result<(), Error> {
        read_commit(&self.store, &commit)?;
        let ref_name = format!("releases/{name}");
        if self.store.get_ref(&ref_name).is_some() {
            return self.refuse(
                ctx,
                "move-ref",
                &ref_name,
                Refusal::ReleaseFrozen(ref_name.clone()),
            );
        }
        self.update_ref(&ref_name, None, Some(commit), ctx, false)
    }

    /// Merges `source` into `target` as a change request: every approval is
    /// checked against the source head's author, a merge-only or agent-authored
    /// change needs at least one, and parts changed on both sides conflict.
    pub fn merge(
        &mut self,
        source: &str,
        target: &str,
        ctx: &Ctx,
        approvals: &[Actor],
    ) -> Result<Hash, Error> {
        let missing_ref = |n: &str| Error::RefConflict {
            name: n.to_string(),
            expected: None,
            found: None,
        };
        let theirs = self
            .store
            .get_ref(source)
            .ok_or_else(|| missing_ref(source))?;
        let ours = self
            .store
            .get_ref(target)
            .ok_or_else(|| missing_ref(target))?;
        let author = read_commit(&self.store, &theirs)?.author;
        let policy = self.policy()?.clone();
        for a in approvals {
            if let Err(r) = policy.authorize_approval(a, &author.id) {
                return self.refuse(ctx, "approve", source, r);
            }
        }
        let needs_approval = author.kind == ActorKind::Agent || policy.main_merge_only;
        if needs_approval && approvals.is_empty() {
            let r = Refusal::NotReviewer("no approval".into());
            return self.refuse(ctx, "merge", source, r);
        }
        if is_ancestor(&self.store, ours, theirs)? {
            self.update_ref(target, Some(ours), Some(theirs), ctx, true)?;
            return Ok(theirs);
        }
        let base = merge_base(&self.store, ours, theirs)?;
        let empty = Tree {
            v: 1,
            entries: vec![],
        };
        let base_tree = match base {
            Some(b) => read_tree(&self.store, &read_commit(&self.store, &b)?.tree)?,
            None => empty,
        };
        let our_tree = read_tree(&self.store, &read_commit(&self.store, &ours)?.tree)?;
        let their_tree = read_tree(&self.store, &read_commit(&self.store, &theirs)?.tree)?;
        let merged = merge_trees(&base_tree, &our_tree, &their_tree).map_err(Error::Conflict)?;
        let tree_bytes = encode(&merged);
        let tree = address(Kind::Tree, &tree_bytes);
        let commit = Commit {
            v: 1,
            parents: vec![ours, theirs],
            tree,
            author: ctx.actor.clone(),
            message: format!("Merge {source} into {target}"),
            ts: ctx.ts,
            provenance: ctx.provenance.clone(),
            imported_from: None,
        };
        let commit_bytes = encode(&commit);
        let h = address(Kind::Commit, &commit_bytes);
        let objects = BTreeMap::from([
            (tree, (Kind::Tree, tree_bytes)),
            (h, (Kind::Commit, commit_bytes)),
        ]);
        self.receive(objects, target, Some(ours), Some(h), ctx, true)?;
        Ok(h)
    }

    /// The history every ref reaches, ready to travel in a file.
    pub fn bundle(&self) -> Result<Bundle, Error> {
        let heads: Vec<Hash> = self.store.refs().into_iter().map(|(_, h)| h).collect();
        let mut objects = BTreeMap::new();
        for (kind, h) in reachable(&self.store, &heads, |_| false)? {
            objects.insert(h, (kind, read(&self.store, kind, &h)?));
        }
        Ok(Bundle {
            objects,
            refs: self.store.refs().into_iter().collect(),
            evidence: self.store.evidence(),
        })
    }

    /// The `.paged` projection of `ref_name`, optionally carrying the history.
    /// Exports are reads, so they are recorded.
    pub fn export(
        &mut self,
        ref_name: &str,
        ctx: &Ctx,
        with_history: bool,
    ) -> Result<Vec<u8>, Error> {
        let head = self
            .store
            .get_ref(ref_name)
            .ok_or_else(|| Error::RefConflict {
                name: ref_name.to_string(),
                expected: None,
                found: None,
            })?;
        let tree = read_commit(&self.store, &head)?.tree;
        let action = if with_history {
            "export-history"
        } else {
            "export"
        };
        self.record(ctx, action, ref_name, "ok");
        let extra = if with_history {
            bundle_parts(&self.bundle()?)
        } else {
            Vec::new()
        };
        project(&self.store, &tree, &extra)
    }

    /// Removes every object no ref reaches. Returns how many went.
    pub fn gc(&mut self) -> Result<usize, Error> {
        let heads: Vec<Hash> = self.store.refs().into_iter().map(|(_, h)| h).collect();
        let live: BTreeSet<Hash> = reachable(&self.store, &heads, |_| false)?
            .into_iter()
            .map(|(_, h)| h)
            .collect();
        let dead: Vec<Hash> = self
            .store
            .addresses()
            .into_iter()
            .filter(|h| !live.contains(h))
            .collect();
        for h in &dead {
            self.store.remove(h);
        }
        Ok(dead.len())
    }
}
