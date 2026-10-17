# The paged repository format

The content-addressed store that holds a document's authoritative state (ADR 180). Implemented
once in the `paged-repo` crate and used by every host: the browser, the CLI and a server.

## Addresses

An address is a 32-byte BLAKE3 hash in **derive-key mode**: the context string names the
object kind and version, so equal bytes of two kinds never share an address.

| Kind | Context string | Bytes hashed |
|---|---|---|
| Blob | `paged:blob:v1` | the part's uncompressed bytes, or one chunk |
| Chunk list | `paged:chunks:v1` | canonical JSON of the chunk list |
| Tree | `paged:tree:v1` | canonical JSON of the tree |
| Commit | `paged:commit:v1` | canonical JSON of the commit |
| Evidence | `paged:evidence:v1` | canonical JSON of the entry |
| Recipe | `paged:recipe:v1` | canonical JSON of a projection recipe |

Addresses are written as 64 lowercase hex characters.

## Canonical JSON

Objects are serialized as compact JSON with fields in the order this document lists them,
absent optional fields omitted, integers only (no floats), and UTF-8 strings. A reader decodes
an object, re-encodes it, and rejects it if the bytes differ, so one object has one encoding.

## Objects

**Chunk list.** A part larger than 262 144 bytes is split by FastCDC (v2020; minimum 16 KiB,
average 64 KiB, maximum 256 KiB).

```json
{"v":1,"size":1048576,"chunks":["<hash>","<hash>"]}
```

**Tree.** One entry per container part, sorted by path (byte order). `kind` is `blob` or
`chunks`; `hash` names the blob or the chunk list; `size` is the part's uncompressed size.
Entries under `paged/repo/` never appear in a tree; that folder holds history (below).

```json
{"v":1,"entries":[{"path":"designmap.xml","kind":"blob","hash":"<hash>","size":1234}]}
```

**Commit.**

```json
{"v":1,"parents":["<hash>"],"tree":"<hash>",
 "author":{"id":"user:42","kind":"human"},
 "message":"Save","ts":1760659200000,
 "provenance":{"client":"claude","model":"claude-opus-5-5","run":"r-1","on_behalf_of":"user:42"},
 "imported_from":{"name":"brochure.idml","sha256":"<hex>"}}
```

- `kind` is `human`, `service` or `agent`. `ts` is Unix milliseconds, supplied by the host.
- `provenance` is required when `author.kind` is `agent`, and refused otherwise.
- `imported_from` appears only on a commit made by opening a file.

**Evidence entry.** Append-only; each entry names the address of the one before it.

```json
{"seq":7,"ts":1760659200000,"actor":{"id":"user:42","kind":"human"},
 "action":"export","target":"main","result":"ok","prev":"<hash>"}
```

The first entry has no `prev`. Verifying the log recomputes every address in order.

## Refs

| Ref | Who moves it | Rule |
|---|---|---|
| `main` | humans and services with write access | fast-forward; merge-only when the policy says so |
| `drafts/<actor>/<name>` | that actor | free |
| `agent/<run>/<name>` | agents | the only refs an agent writes at the default autonomy |
| `releases/<name>` | humans with write access | created once, never moved or deleted |

A ref moves only by compare-and-set against the value the writer last saw.

## Policy

A policy names the role of each actor (`owner`, `editor`, `reviewer`, `viewer`), the agent
autonomy (`read`, `propose`, `commit`), and whether `main` is merge-only. A host that has no
policy uses the **local policy**: the device's one human is owner, agents propose. A policy
that is present and does not parse refuses every write.

## Projections

- **`.paged`**: a ZIP of the tree's parts. `mimetype` first and stored; every other part in
  tree order, deflated; every timestamp 1980-01-01 00:00. Equal trees give equal bytes.
- **History bundle**: a `.paged` projection may add `paged/repo/objects.bin` (records of a
  one-byte kind tag — 1 blob, 2 chunk list, 3 tree, 4 commit, 5 evidence, 6 recipe — the
  32-byte address, a little-endian u32 length and the bytes), `paged/repo/refs.json` and
  `paged/repo/evidence.json`. Importing such a file restores the repository.

## Sync

To push a ref, a client walks from the new head back to commits the remote already has and
offers the addresses it found; the remote answers with the ones it lacks; the client sends
only those, then asks the remote to compare-and-set the ref.
