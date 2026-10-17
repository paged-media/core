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
use std::io::{Cursor, Write};

use paged_repo::*;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// What a host keeps on disk: packs with their indexes, refs, evidence lines.
#[derive(Default)]
struct Disk {
    packs: BTreeMap<u32, PackFile>,
    refs: Vec<u8>,
    evidence: Vec<u8>,
    reads: usize,
}

impl Disk {
    fn write(&mut self, f: Flush) {
        if let Some(p) = f.pack {
            self.packs.insert(p.id, p);
        }
        self.refs = f.refs;
        for line in f.evidence {
            self.evidence.extend_from_slice(&line);
            self.evidence.push(b'\n');
        }
    }

    fn open(&self) -> Repo<PackStore> {
        let mut s = PackStore::new();
        for (id, p) in &self.packs {
            s.load_index(*id, &p.index).unwrap();
        }
        if !self.refs.is_empty() {
            s.load_refs(&self.refs).unwrap();
        }
        s.load_evidence(&self.evidence).unwrap();
        Repo::new(s, Ok(Policy::local("user:me")))
    }

    fn read(&mut self, l: &Loc) -> Result<Vec<u8>, Error> {
        self.reads += 1;
        let p = &self.packs[&l.pack];
        Ok(p.data[l.offset as usize..l.offset as usize + l.len as usize].to_vec())
    }
}

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

fn doc(model: Vec<u8>) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default();
    for (p, b) in [
        (
            "mimetype",
            b"application/vnd.adobe.indesign-idml-package".to_vec(),
        ),
        ("designmap.xml", b"<Document/>".to_vec()),
        ("paged/core/model/document.pgm", model),
    ] {
        zip.start_file(p, o).unwrap();
        zip.write_all(&b).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn me() -> Ctx {
    Ctx::human("user:me", 1)
}

#[test]
fn a_repository_survives_a_reopen_and_faults_blobs_in() {
    let mut disk = Disk::default();
    let mut repo = disk.open();
    let file = doc(noise(900_000, 1));
    repo.import(&file, "d.paged", "main", &me()).unwrap();
    disk.write(repo.store.flush());
    let s = import(&doc(noise(900_000, 2)), "d").unwrap();
    repo.commit(s.objects, s.tree, "main", &me(), "Save", None)
        .unwrap();
    disk.write(repo.store.flush());
    assert_eq!(disk.packs.len(), 2);
    let head = repo.head("main");

    // Cold: indexes, refs and evidence only. Metadata first, then blobs on demand.
    let mut cold = disk.open();
    assert_eq!(cold.head("main"), head);
    for (h, loc) in cold.store.metadata() {
        let bytes = disk.read(&loc).unwrap();
        cold.store.provide(h, bytes).unwrap();
    }
    assert_eq!(cold.log("main").unwrap().len(), 2);
    disk.reads = 0;
    let out = faulting(
        &mut cold,
        |l| disk.read(l),
        |r| r.export("main", &me(), false),
    )
    .unwrap();
    assert!(disk.reads > 0);
    assert_eq!(
        import(&out, "x").unwrap().tree,
        repo.read_commit(&head.unwrap()).unwrap().tree
    );
    // The export was recorded once, however many faults it took.
    let exports = cold.store.evidence().len();
    disk.write(cold.store.flush());
    assert_eq!(exports, 1);
    disk.open().verify_evidence().unwrap();
}

#[test]
fn an_unchanged_save_writes_a_pack_of_one_commit() {
    let mut disk = Disk::default();
    let mut repo = disk.open();
    let file = doc(noise(600_000, 3));
    repo.import(&file, "d", "main", &me()).unwrap();
    disk.write(repo.store.flush());
    let s = import(&file, "d").unwrap();
    repo.commit(s.objects, s.tree, "main", &me(), "Save", None)
        .unwrap();
    let f = repo.store.flush();
    let pack = f.pack.unwrap();
    assert_eq!(pack.index.len(), 45, "one object: the commit");
}

#[test]
fn the_pack_store_and_the_memory_store_agree() {
    let file = doc(noise(700_000, 4));
    let mut mem = Repo::new(MemoryStore::default(), Ok(Policy::local("user:me")));
    let disk = Disk::default();
    let mut packed = disk.open();
    let a = mem.import(&file, "d", "main", &me()).unwrap();
    let b = packed.import(&file, "d", "main", &me()).unwrap();
    assert_eq!(a, b);
    assert_eq!(
        mem.export("main", &me(), true).unwrap(),
        packed.export("main", &me(), true).unwrap()
    );
}

#[test]
fn a_tampered_pack_is_refused() {
    let mut disk = Disk::default();
    let mut repo = disk.open();
    repo.import(&doc(noise(10, 5)), "d", "main", &me()).unwrap();
    disk.write(repo.store.flush());
    for p in disk.packs.values_mut() {
        let last = p.data.len() - 1;
        p.data[last] ^= 1;
    }
    let mut cold = disk.open();
    let r = faulting(
        &mut cold,
        |l| disk.read(l),
        |r| r.export("main", &me(), false),
    );
    assert!(matches!(r, Err(Error::Tampered(_))));
}
