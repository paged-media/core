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

"""Print a PDF's bookmarks (`/Outlines`) as JSON: per item its title,
depth, the 0-based page its destination names, and the destination's
view (`/XYZ left top zoom`, `/Fit`, …). Reads InDesign's object streams
the way `pdf-strokes.py` does.

    python3 tools/indesign-export/pdf-outlines.py file.pdf
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

    outlines = None
    for oid, body in objs.items():
        if re.search(rb"/Type\s*/Catalog\b", body):
            root = re.search(rb"/Pages\s+(\d+) 0 R", body)
            if root:
                walk(int(root.group(1)))
            o = re.search(rb"/Outlines\s+(\d+) 0 R", body)
            outlines = int(o.group(1)) if o else None
            break
    out = []

    def dest_of(body):
        d = re.search(rb"/Dest\s*\[(.*?)\]", body, re.S) or re.search(
            rb"/D\s*\[(.*?)\]", body, re.S)
        if not d:
            a = re.search(rb"/A\s+(\d+) 0 R", body)
            if a:
                return dest_of(objs.get(int(a.group(1)), b""))
            return None, None
        parts = d.group(1).split()
        page = int(parts[0]) if parts else None
        return (pages.index(page) if page in pages else page), b" ".join(parts[3:]).decode()

    def items(first, depth):
        oid = first
        while oid:
            body = objs.get(oid, b"")
            t = re.search(rb"/Title\s*(\((?:\\.|[^\\)])*\)|<[0-9A-Fa-f]*>)", body, re.S)
            page, view = dest_of(body)
            out.append({"title": text_of(t.group(1)) if t else None, "depth": depth,
                        "page": page, "view": view})
            f = re.search(rb"/First\s+(\d+) 0 R", body)
            if f:
                items(int(f.group(1)), depth + 1)
            n = re.search(rb"/Next\s+(\d+) 0 R", body)
            oid = int(n.group(1)) if n else None

    if outlines:
        f = re.search(rb"/First\s+(\d+) 0 R", objs.get(outlines, b""))
        if f:
            items(int(f.group(1)), 0)
    json.dump(out, sys.stdout, indent=1, ensure_ascii=False)
    print()


if __name__ == "__main__":
    main(sys.argv[1])
