#!/usr/bin/env python3
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# This file is part of paged (https://paged.media) and is additionally
# available under the Paged Media Enterprise License (PMEL). Full
# copyright and license information is available in LICENSE.md which is
# distributed with this source code.
#
#  @copyright  Copyright (c) And The Next GmbH
#  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
#

"""Print a PDF's logical structure (Tagged PDF) as JSON: the catalog's
`/MarkInfo`, `/Lang` and `/ViewerPreferences`, then the structure tree
from `/StructTreeRoot` depth-first — per element its type (`/S`, with the
role map's standard type when it maps one), its `/Alt` / `/ActualText` /
`/Lang`, its depth, and the pages and marked-content ids (MCIDs) it holds.
Reads InDesign's object streams the way `pdf-outlines.py` does.

    python3 tools/indesign-export/pdf-structure.py file.pdf
"""

import json
import re
import sys
import zlib


def objects(data):
    for m in re.finditer(rb"(\d+) 0 obj(.*?)endobj", data, re.S):
        yield int(m.group(1)), m.group(2)


def stream_of(body):
    m = re.search(rb"stream\r?\n", body)
    if not m:
        return None
    raw = body[m.end():body.rfind(b"endstream")]
    if b"FlateDecode" in body[:m.start()]:
        try:
            return zlib.decompress(raw)
        except zlib.error:
            return None
    return raw


def all_objects(data):
    objs = dict(objects(data))
    for body in list(objs.values()):
        if not re.search(rb"/Type\s*/ObjStm\b", body):
            continue
        raw = stream_of(body)
        n = re.search(rb"/N\s+(\d+)", body)
        first = re.search(rb"/First\s+(\d+)", body)
        if raw is None or not n or not first:
            continue
        head = [int(x) for x in raw[: int(first.group(1))].split()]
        pairs = list(zip(head[0::2], head[1::2]))[: int(n.group(1))]
        base = int(first.group(1))
        for k, (oid, off) in enumerate(pairs):
            end = base + pairs[k + 1][1] if k + 1 < len(pairs) else len(raw)
            objs.setdefault(oid, raw[base + off:end])
    return objs


def text_of(raw):
    raw = raw.strip()
    if raw.startswith(b"<") and not raw.startswith(b"<<"):
        b = bytes.fromhex(raw[1:-1].decode())
    else:
        s = raw[1:-1]
        s = re.sub(rb"\\([nrtbf()\\])", lambda m: {b"n": b"\n", b"r": b"\r", b"t": b"\t",
                   b"b": b"\b", b"f": b"\f"}.get(m.group(1), m.group(1)), s)
        s = re.sub(rb"\\([0-7]{1,3})", lambda m: bytes([int(m.group(1), 8)]), s)
        b = s
    if b.startswith(b"\xfe\xff"):
        return b[2:].decode("utf-16-be")
    return b.decode("latin-1")


def main(path):
    data = open(path, "rb").read()
    objs = all_objects(data)
    pages = []

    def walk(oid):
        body = objs.get(oid, b"")
        if re.search(rb"/Type\s*/Pages\b", body):
            k = re.search(rb"/Kids\s*\[([^\]]*)\]", body)
            for kid in re.findall(rb"(\d+) 0 R", k.group(1) if k else b""):
                walk(int(kid))
        elif re.search(rb"/Type\s*/Page\b", body):
            pages.append(oid)

    catalog = b""
    for oid, body in objs.items():
        if re.search(rb"/Type\s*/Catalog\b", body):
            catalog = body
            root = re.search(rb"/Pages\s+(\d+) 0 R", body)
            if root:
                walk(int(root.group(1)))
            break

    def deref(raw):
        m = re.fullmatch(rb"\s*(\d+) 0 R\s*", raw or b"")
        return objs.get(int(m.group(1)), b"") if m else raw

    def text_field(body, key):
        t = re.search(rb"/" + key + rb"\s*(\((?:\\.|[^\\)])*\)|<[0-9A-Fa-f]*>)", body, re.S)
        return text_of(t.group(1)) if t else None

    mark = re.search(rb"/MarkInfo\s*(<<.*?>>|\d+ 0 R)", catalog, re.S)
    out = {
        "markInfo": deref(mark.group(1)).decode("latin-1").strip() if mark else None,
        "lang": text_field(catalog, b"Lang"),
        "viewerPreferences": None,
        "roleMap": {},
        "elements": [],
    }
    vp = re.search(rb"/ViewerPreferences\s*(<<.*?>>|\d+ 0 R)", catalog, re.S)
    if vp:
        out["viewerPreferences"] = deref(vp.group(1)).decode("latin-1").strip()
    tree = re.search(rb"/StructTreeRoot\s+(\d+) 0 R", catalog)
    if not tree:
        json.dump(out, sys.stdout, indent=1, ensure_ascii=False)
        print()
        return
    root = objs.get(int(tree.group(1)), b"")
    rm = re.search(rb"/RoleMap\s*(<<.*?>>|\d+ 0 R)", root, re.S)
    if rm:
        for k, v in re.findall(rb"/([^\s/<>\[\]()]+)\s*/([^\s/<>\[\]()]+)", deref(rm.group(1))):
            out["roleMap"][k.decode()] = v.decode()

    def kids_of(body):
        k = re.search(rb"/K\s*(\[.*?\]|<<.*?>>|\d+\s+\d+\s+R|\d+)", body, re.S)
        if not k:
            return []
        raw = k.group(1)
        if raw.startswith(b"["):
            raw = raw[1:-1]
        out_kids = []
        # Object references, inline marked-content-reference dicts and
        # bare MCIDs, in order.
        for ref, gen, inline, mcid in re.findall(
            rb"(\d+)\s+(\d+)\s+R|(<<.*?>>)|(\d+)", raw, re.S
        ):
            if ref:
                out_kids.append((int(ref), "ref"))
            elif inline:
                m = re.search(rb"/MCID\s+(\d+)", inline)
                if m:
                    out_kids.append((int(m.group(1)), "mcid"))
            elif mcid:
                out_kids.append((int(mcid), "mcid"))
        return out_kids

    def elem(oid, depth, seen):
        if oid in seen:
            return
        seen.add(oid)
        body = objs.get(oid, b"")
        s = re.search(rb"/S\s*/([^\s/<>\[\]()]+)", body)
        typ = s.group(1).decode() if s else None
        pg = re.search(rb"/Pg\s+(\d+) 0 R", body)
        entry = {"depth": depth, "type": typ, "standard": out["roleMap"].get(typ, typ),
                 "page": pages.index(int(pg.group(1))) if pg and int(pg.group(1)) in pages else None,
                 "mcids": [], "alt": text_field(body, b"Alt"),
                 "actualText": text_field(body, b"ActualText"), "lang": text_field(body, b"Lang")}
        out["elements"].append(entry)
        for n, kind in kids_of(body):
            if kind == "mcid":
                entry["mcids"].append(n)
                continue
            kb = objs.get(n, b"")
            if re.search(rb"/Type\s*/MCR\b", kb):
                m = re.search(rb"/MCID\s+(\d+)", kb)
                if m:
                    entry["mcids"].append(int(m.group(1)))
            elif re.search(rb"/Type\s*/OBJR\b", kb):
                entry.setdefault("objects", 0)
                entry["objects"] += 1
            else:
                elem(n, depth + 1, seen)

    for n, kind in kids_of(root):
        if kind == "ref":
            elem(n, 0, set())
    json.dump(out, sys.stdout, indent=1, ensure_ascii=False)
    print()


if __name__ == "__main__":
    main(sys.argv[1])
