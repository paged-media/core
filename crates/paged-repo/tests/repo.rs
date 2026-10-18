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
use std::io::{Cursor, Read, Write};

use paged_repo::*;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

const MIME: &[u8] = b"application/vnd.adobe.indesign-idml-package";

/// Text-like bytes that do not compress to nothing, from a fixed seed.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed * 2 + 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b"abcdefghij klmnop\"{}:,0123456789"[(x % 32) as usize]
        })
        .collect()
}

fn zip_of(parts: &[(&str, Vec<u8>)], method: CompressionMethod, reverse: bool) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut order: Vec<_> = parts.iter().collect();
    if reverse {
        order.reverse();
    }
    for (i, (path, bytes)) in order.into_iter().enumerate() {
        let time = DateTime::from_date_and_time(2020 + i as u16 % 5, 3, 4, 5, 6, 8).unwrap();
        let m = if *path == "mimetype" {
            CompressionMethod::Stored
        } else {
            method
        };
        let o = SimpleFileOptions::default()
            .compression_method(m)
            .last_modified_time(time);
        zip.start_file(*path, o).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn doc_parts(model: Vec<u8>) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("mimetype", MIME.to_vec()),
        ("designmap.xml", b"<Document/>".to_vec()),
        ("Stories/Story_u1.xml", b"<Story Self=\"u1\"/>".to_vec()),
        ("paged/core/model/document.pgm", model),
        ("paged/media.paged.data/session.json", b"{\"v\":1}".to_vec()),
    ]
}

fn doc(model: Vec<u8>) -> Vec<u8> {
    zip_of(&doc_parts(model), CompressionMethod::Deflated, false)
}

fn parts_of(file: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = ZipArchive::new(Cursor::new(file)).unwrap();
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut b = Vec::new();
            f.read_to_end(&mut b).unwrap();
            (f.name().unwrap().into_owned(), b)
        })
        .collect()
}

fn local() -> Repo<MemoryStore> {
    Repo::new(MemoryStore::default(), Ok(Policy::local("user:me")))
}

fn me() -> Ctx {
    Ctx::human("user:me", 1_000)
}

fn claude(run: &str) -> Ctx {
    Ctx {
        actor: Actor::agent("agent:claude"),
        ts: 2_000,
        provenance: Some(Provenance {
            client: "claude".into(),
            model: Some("claude-opus-5-5".into()),
            run: run.into(),
            on_behalf_of: "user:me".into(),
        }),
    }
}

fn snapshot_commit(
    repo: &mut Repo<MemoryStore>,
    file: &[u8],
    ref_name: &str,
    ctx: &Ctx,
) -> Result<Hash, Error> {
    let s = import(file, "doc.paged")?;
    repo.commit(s.objects, s.tree, ref_name, ctx, "Save", None)
}

#[test]
fn a_projection_carries_every_part_byte_for_byte() {
    let file = doc(noise(1_000_000, 7));
    let mut repo = local();
    repo.import(&file, "doc.paged", "main", &me()).unwrap();
    let out = repo.export("main", &me(), false).unwrap();
    assert_eq!(parts_of(&out), parts_of(&file));
    let names: Vec<String> = {
        let z = ZipArchive::new(Cursor::new(&out)).unwrap();
        z.file_names().map(|n| n.unwrap().into_owned()).collect()
    };
    let mut z = ZipArchive::new(Cursor::new(&out)).unwrap();
    assert_eq!(z.by_index(0).unwrap().name().unwrap(), "mimetype");
    assert_eq!(
        z.by_index(0).unwrap().compression(),
        CompressionMethod::Stored
    );
    assert_eq!(names.len(), 5);
}

#[test]
fn the_zip_does_not_change_an_address() {
    let parts = doc_parts(noise(600_000, 3));
    let a = import(&zip_of(&parts, CompressionMethod::Deflated, false), "a").unwrap();
    let b = import(&zip_of(&parts, CompressionMethod::Stored, true), "b").unwrap();
    assert_eq!(a.tree, b.tree);
    assert_ne!(a.imported_from.sha256, b.imported_from.sha256);
}

#[test]
fn a_projection_is_deterministic() {
    let mut repo = local();
    repo.import(&doc(noise(400_000, 9)), "doc.paged", "main", &me())
        .unwrap();
    let one = repo.export("main", &me(), false).unwrap();
    let two = repo.export("main", &me(), false).unwrap();
    assert_eq!(one, two);
    let s1 = import(&one, "x").unwrap();
    assert_eq!(
        s1.tree,
        repo.read_commit(&repo.head("main").unwrap()).unwrap().tree
    );
}

#[test]
fn an_edit_stores_the_changed_chunks_only() {
    let model = noise(2_000_000, 11);
    let mut repo = local();
    snapshot_commit(&mut repo, &doc(model.clone()), "main", &me()).unwrap();
    let before = repo.store.objects.clone();

    let mut edited = model;
    edited.splice(1_000_000..1_000_010, b"a new word".iter().copied());
    snapshot_commit(&mut repo, &doc(edited), "main", &me()).unwrap();

    let added: Vec<_> = repo
        .store
        .objects
        .keys()
        .filter(|h| !before.contains_key(h))
        .collect();
    let added_bytes: usize = added.iter().map(|h| repo.store.objects[*h].len()).sum();
    // New chunk(s) around the edit, the chunk list, the tree, the commit.
    assert!(added.len() <= 6, "added {} objects", added.len());
    assert!(
        added_bytes < 600_000,
        "added {added_bytes} bytes for a 10-byte edit"
    );
}

#[test]
fn an_unchanged_save_costs_one_commit() {
    let file = doc(noise(500_000, 5));
    let mut repo = local();
    snapshot_commit(&mut repo, &file, "main", &me()).unwrap();
    let n = repo.store.objects.len();
    snapshot_commit(&mut repo, &file, "main", &me()).unwrap();
    assert_eq!(repo.store.objects.len(), n + 1);
    assert_eq!(repo.log("main").unwrap().len(), 2);
}

#[test]
fn opening_a_file_records_where_it_came_from() {
    let file = doc(noise(100, 1));
    let mut repo = local();
    let c = repo.import(&file, "brochure.idml", "main", &me()).unwrap();
    let from = repo.read_commit(&c).unwrap().imported_from.unwrap();
    assert_eq!(from.name, "brochure.idml");
    assert_eq!(from.sha256.len(), 64);
}

#[test]
fn a_push_sends_only_what_the_remote_lacks() {
    let mut a = local();
    let mut remote = local();
    snapshot_commit(&mut a, &doc(noise(1_500_000, 2)), "main", &me()).unwrap();
    push(&a, &mut remote, "main", &me()).unwrap();
    assert_eq!(remote.head("main"), a.head("main"));
    let first = remote.store.objects.len();

    let mut model = noise(1_500_000, 2);
    model[700_000] = b'#';
    snapshot_commit(&mut a, &doc(model), "main", &me()).unwrap();
    let offer = reachable(&a.store, &[a.head("main").unwrap()], |c| {
        remote.store.has(c)
    })
    .unwrap();
    let hashes: Vec<Hash> = offer.iter().map(|(_, h)| *h).collect();
    let want = missing(&remote.store, &hashes);
    assert!(want.len() <= 6 && want.len() < hashes.len());
    push(&a, &mut remote, "main", &me()).unwrap();
    assert_eq!(remote.store.objects.len(), first + want.len());
    assert_eq!(remote.head("main"), a.head("main"));
}

#[test]
fn of_two_racing_writers_one_wins() {
    let mut remote = local();
    let base = snapshot_commit(&mut remote, &doc(noise(10, 1)), "main", &me()).unwrap();
    let mut a = Repo::new(remote.store.clone(), Ok(Policy::local("user:me")));
    let mut b = Repo::new(remote.store.clone(), Ok(Policy::local("user:me")));
    snapshot_commit(&mut a, &doc(noise(10, 2)), "main", &me()).unwrap();
    snapshot_commit(&mut b, &doc(noise(10, 3)), "main", &me()).unwrap();
    push(&a, &mut remote, "main", &me()).unwrap();
    match push(&b, &mut remote, "main", &me()) {
        Err(Error::RefConflict { .. }) | Err(Error::Refused(Refusal::NotFastForward(_))) => {}
        other => panic!("second writer should lose, got {other:?}"),
    }
    assert_eq!(remote.head("main"), a.head("main"));
    assert_ne!(remote.head("main"), Some(base));
}

#[test]
fn an_agent_proposes_on_its_own_branch_and_a_human_merges() {
    let mut repo = local();
    snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me()).unwrap();

    let refused = snapshot_commit(&mut repo, &doc(noise(10, 2)), "main", &claude("r1"));
    assert!(matches!(
        refused,
        Err(Error::Refused(Refusal::NotPermitted { .. }))
    ));

    repo.branch("main", "agent/r1/headline", &claude("r1"))
        .unwrap();
    snapshot_commit(
        &mut repo,
        &doc(noise(10, 2)),
        "agent/r1/headline",
        &claude("r1"),
    )
    .unwrap();
    let head = repo.head("agent/r1/headline").unwrap();
    assert!(repo.read_commit(&head).unwrap().provenance.is_some());

    let unapproved = repo.merge("agent/r1/headline", "main", &me(), &[]);
    assert!(matches!(unapproved, Err(Error::Refused(_))));
    let self_approved = repo.merge(
        "agent/r1/headline",
        "main",
        &me(),
        &[Actor::agent("agent:claude")],
    );
    assert!(matches!(
        self_approved,
        Err(Error::Refused(Refusal::SelfApproval(_)))
    ));

    let mut policy = Policy::local("user:me");
    policy.roles.insert("user:rev".into(), Role::Reviewer);
    let mut repo = Repo::new(repo.store, Ok(policy));
    repo.merge(
        "agent/r1/headline",
        "main",
        &me(),
        &[Actor::human("user:rev")],
    )
    .unwrap();
    assert_eq!(repo.head("main"), Some(head));
}

#[test]
fn an_agent_commit_needs_provenance() {
    let mut repo = local();
    snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me()).unwrap();
    let mut bare = claude("r2");
    repo.branch("main", "agent/r2/x", &bare).unwrap();
    bare.provenance = None;
    let r = snapshot_commit(&mut repo, &doc(noise(10, 2)), "agent/r2/x", &bare);
    assert!(matches!(r, Err(Error::Refused(Refusal::MissingProvenance))));
}

#[test]
fn read_autonomy_refuses_every_agent_write() {
    let mut policy = Policy::local("user:me");
    policy.autonomy = Autonomy::Read;
    let mut repo = Repo::new(MemoryStore::default(), Ok(policy));
    snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me()).unwrap();
    assert!(repo.branch("main", "agent/r3/x", &claude("r3")).is_err());
}

#[test]
fn a_broken_policy_refuses_every_write() {
    let policy = Policy::parse(b"{\"v\":1,\"roles\":{}");
    assert!(policy.is_err());
    let mut repo = Repo::new(MemoryStore::default(), policy);
    let r = snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me());
    assert!(matches!(r, Err(Error::Refused(Refusal::PolicyInvalid(_)))));
    assert!(repo.head("main").is_none());
    assert_eq!(repo.store.evidence.len(), 1, "the refusal is recorded");
}

#[test]
fn refs_follow_their_family_rules() {
    let mut repo = local();
    let c1 = snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me()).unwrap();
    repo.release("2026-10", c1, &me()).unwrap();
    let c2 = snapshot_commit(&mut repo, &doc(noise(10, 2)), "main", &me()).unwrap();
    assert!(matches!(
        repo.release("2026-10", c2, &me()),
        Err(Error::Refused(_))
    ));
    assert_eq!(repo.head("releases/2026-10"), Some(c1));

    let other = Ctx::human("user:other", 5);
    let mut policy = Policy::local("user:me");
    policy.roles.insert("user:other".into(), Role::Editor);
    let mut repo = Repo::new(repo.store, Ok(policy));
    assert!(repo.branch("main", "drafts/user:me/try", &other).is_err());
    repo.branch("main", "drafts/user:other/try", &other)
        .unwrap();
    assert!(repo.branch("main", "feature/x", &me()).is_err());

    // Moving main back to c1 is not a fast-forward.
    let s = import(&doc(noise(10, 1)), "x").unwrap();
    let r = repo.receive(s.objects, "main", Some(c2), Some(c1), &me(), false);
    assert!(matches!(r, Err(Error::Refused(Refusal::NotFastForward(_)))));
}

#[test]
fn a_tampered_object_is_refused_on_read() {
    let mut repo = local();
    repo.import(&doc(noise(300_000, 4)), "d", "main", &me())
        .unwrap();
    let victim = *repo.store.objects.keys().next().unwrap();
    repo.store.objects.get_mut(&victim).unwrap()[0] ^= 1;
    assert!(matches!(
        repo.export("main", &me(), false),
        Err(Error::Tampered(_)) | Err(Error::Malformed(_)) | Err(Error::NonCanonical)
    ));
}

#[test]
fn tampered_evidence_breaks_the_chain() {
    let mut repo = local();
    repo.import(&doc(noise(10, 1)), "d", "main", &me()).unwrap();
    for _ in 0..3 {
        repo.export("main", &me(), false).unwrap();
    }
    assert!(repo.verify_evidence().unwrap().is_some());
    let e = &mut repo.store.evidence[1];
    let s = String::from_utf8(e.clone())
        .unwrap()
        .replace("\"ok\"", "\"no\"");
    *e = s.into_bytes();
    assert!(matches!(
        repo.verify_evidence(),
        Err(Error::EvidenceBroken(2))
    ));
}

#[test]
fn a_file_can_carry_its_history() {
    let mut repo = local();
    repo.import(&doc(noise(400_000, 6)), "d", "main", &me())
        .unwrap();
    snapshot_commit(&mut repo, &doc(noise(400_000, 8)), "main", &me()).unwrap();
    repo.release("v1", repo.head("main").unwrap(), &me())
        .unwrap();
    let file = repo.export("main", &me(), true).unwrap();
    assert!(parts_of(&file).contains_key("paged/repo/objects.bin"));

    let mut fresh = local();
    let head = fresh.import(&file, "d.paged", "main", &me()).unwrap();
    assert_eq!(Some(head), repo.head("main"));
    assert_eq!(fresh.store.refs, repo.store.refs);
    // The file carries the evidence as it stood before this export was recorded.
    let before_export = &repo.store.evidence[..repo.store.evidence.len() - 1];
    assert_eq!(fresh.store.evidence, before_export);
    fresh.verify_evidence().unwrap();
    assert_eq!(fresh.log("main").unwrap().len(), 2);

    // Opened by a repository that already has history, the file is a plain import.
    let s = import(&file, "d").unwrap();
    let tree = repo.read_tree(&s.tree).unwrap();
    assert!(tree
        .entries
        .iter()
        .all(|e| !e.path.starts_with(HISTORY_PREFIX)));
}

#[test]
fn branches_that_changed_different_parts_merge() {
    let mut repo = local();
    let base = doc_parts(noise(10, 1));
    snapshot_commit(
        &mut repo,
        &zip_of(&base, CompressionMethod::Deflated, false),
        "main",
        &me(),
    )
    .unwrap();
    repo.branch("main", "drafts/user:me/a", &me()).unwrap();

    let mut ours = base.clone();
    ours[2].1 = b"<Story Self=\"u1\">ours</Story>".to_vec();
    snapshot_commit(
        &mut repo,
        &zip_of(&ours, CompressionMethod::Deflated, false),
        "main",
        &me(),
    )
    .unwrap();

    let mut theirs = base.clone();
    theirs[4].1 = b"{\"v\":2}".to_vec();
    snapshot_commit(
        &mut repo,
        &zip_of(&theirs, CompressionMethod::Deflated, false),
        "drafts/user:me/a",
        &me(),
    )
    .unwrap();

    let m = repo.merge("drafts/user:me/a", "main", &me(), &[]).unwrap();
    let parts = parts_of(&repo.export("main", &me(), false).unwrap());
    assert_eq!(parts["Stories/Story_u1.xml"], ours[2].1);
    assert_eq!(parts["paged/media.paged.data/session.json"], theirs[4].1);
    assert_eq!(repo.read_commit(&m).unwrap().parents.len(), 2);

    let mut clash = base.clone();
    clash[2].1 = b"<Story Self=\"u1\">theirs</Story>".to_vec();
    repo.branch("main", "drafts/user:me/b", &me()).unwrap();
    snapshot_commit(
        &mut repo,
        &zip_of(&clash, CompressionMethod::Deflated, false),
        "drafts/user:me/b",
        &me(),
    )
    .unwrap();
    let mut again = ours.clone();
    again[1].1 = b"<Document v=\"2\"/>".to_vec();
    again[2].1 = b"<Story Self=\"u1\">ours again</Story>".to_vec();
    snapshot_commit(
        &mut repo,
        &zip_of(&again, CompressionMethod::Deflated, false),
        "main",
        &me(),
    )
    .unwrap();
    match repo.merge("drafts/user:me/b", "main", &me(), &[]) {
        Err(Error::Conflict(paths)) => assert_eq!(paths, vec!["Stories/Story_u1.xml".to_string()]),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn diff_names_changed_parts() {
    let a = import(&doc(noise(10, 1)), "a").unwrap();
    let b = import(&doc(noise(10, 2)), "b").unwrap();
    let mut store = MemoryStore::default();
    for (h, (kind, bytes)) in a.objects.into_iter().chain(b.objects) {
        store.put(kind, h, bytes);
    }
    let repo = Repo::new(store, Ok(Policy::local("user:me")));
    let d = diff_trees(
        &repo.read_tree(&a.tree).unwrap(),
        &repo.read_tree(&b.tree).unwrap(),
    );
    assert_eq!(
        d,
        vec![Change {
            path: "paged/core/model/document.pgm".into(),
            kind: ChangeKind::Modified
        }]
    );
}

#[test]
fn collection_keeps_what_refs_reach() {
    let mut repo = local();
    snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me()).unwrap();
    let s = import(&doc(noise(10, 99)), "orphan").unwrap();
    for (h, (kind, bytes)) in s.objects {
        repo.store.put(kind, h, bytes);
    }
    assert!(repo.gc().unwrap() >= 2);
    repo.export("main", &me(), false).unwrap();
}

#[test]
fn hostile_packages_are_refused() {
    let climbing = zip_of(
        &[("../evil", b"x".to_vec())],
        CompressionMethod::Deflated,
        false,
    );
    assert!(matches!(import(&climbing, "x"), Err(Error::Package(_))));
    let history_in_tree = zip_of(
        &[("paged/repo/objects.bin", vec![9, 9])],
        CompressionMethod::Deflated,
        false,
    );
    assert!(import(&history_in_tree, "x").is_err());
    assert!(import(b"not a zip", "x").is_err());
}

#[test]
fn objects_have_one_encoding() {
    let c = br#"{"v":1,"size":3,"chunks":[]}"#;
    assert!(decode::<ChunkList>(c).is_ok());
    assert!(matches!(
        decode::<ChunkList>(br#"{"size":3,"v":1,"chunks":[]}"#),
        Err(Error::NonCanonical)
    ));
    assert!(decode::<ChunkList>(br#"{"v":1, "size":3,"chunks":[]}"#).is_err());
    assert_ne!(address(Kind::Blob, b"x"), address(Kind::Tree, b"x"));
}

/// Real files: `PAGED_REPO_CORPUS=<dir>` round-trips every `.idml`/`.paged` in it.
#[test]
fn corpus_files_round_trip() {
    let Ok(dir) = std::env::var("PAGED_REPO_CORPUS") else {
        return;
    };
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        if ext != "idml" && ext != "paged" {
            continue;
        }
        let file = std::fs::read(&p).unwrap();
        let mut repo = local();
        repo.import(&file, "f", "main", &me()).unwrap();
        let out = repo.export("main", &me(), false).unwrap();
        let (a, b) = (parts_of(&file), parts_of(&out));
        let a: BTreeMap<_, _> = a.into_iter().filter(|(k, _)| !k.ends_with('/')).collect();
        assert_eq!(a, b, "{}", p.display());
        n += 1;
    }
    eprintln!("round-tripped {n} files");
}

#[test]
fn restoring_a_version_is_a_new_commit() {
    let mut repo = local();
    let v1 = snapshot_commit(&mut repo, &doc(noise(10, 1)), "main", &me()).unwrap();
    snapshot_commit(&mut repo, &doc(noise(10, 2)), "main", &me()).unwrap();
    let r = repo.restore(v1, "main", &me()).unwrap();
    assert_eq!(
        repo.read_commit(&r).unwrap().tree,
        repo.read_commit(&v1).unwrap().tree
    );
    assert_eq!(repo.log("main").unwrap().len(), 3);
    let old = repo.export_commit(v1, "v1", &me(), false).unwrap();
    assert_eq!(old, repo.export("main", &me(), false).unwrap());
}
