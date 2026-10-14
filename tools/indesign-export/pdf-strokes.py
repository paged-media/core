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
"""Dump every stroke an InDesign PDF page paints, with its dash array.

    python3 tools/indesign-export/pdf-strokes.py corpus/generated/stroke-styles.pdf [page]

InDesign writes its dashed and dotted strokes as plain PDF: a `d` dash
operator and `m`/`l`/`c` paths under `cm` translations. Reading them back
answers how it lays a dash pattern on a shape, exactly, where a raster
could only estimate it. Prints one JSON object per stroke (`S`/`s`):
width, cap, join, dash array and phase, and the path's points in page
space (y up, PDF units = pt). Standard library only; handles the Flate
streams and the operators InDesign emits for strokes, not general PDF.
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
    """Every object body, including those packed in object streams."""
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


def page_contents(data):
    """Content stream bytes of each page, in page order."""
    objs = all_objects(data)
    pages = []

    # Page order is the page tree's, walked depth-first from the catalog.
    def walk(oid, out):
        body = objs.get(oid, b"")
        if re.search(rb"/Type\s*/Pages\b", body):
            k = re.search(rb"/Kids\s*\[([^\]]*)\]", body)
            for kid in re.findall(rb"(\d+) 0 R", k.group(1) if k else b""):
                walk(int(kid), out)
        elif re.search(rb"/Type\s*/Page\b", body):
            out.append(oid)

    page_ids = []
    for oid, body in objs.items():
        root = re.search(rb"/Pages\s+(\d+) 0 R", body)
        if root and re.search(rb"/Type\s*/Catalog\b", body):
            walk(int(root.group(1)), page_ids)
            break
    for pid in page_ids:
        body = objs.get(pid, b"")
        c = re.search(rb"/Contents\s*(\[[^\]]*\]|\d+ 0 R)", body)
        if not c:
            pages.append(b"")
            continue
        refs = [int(x) for x in re.findall(rb"(\d+) 0 R", c.group(1))]
        pages.append(b"".join(stream_of(objs[r]) or b"" for r in refs if r in objs))
    return pages


TOKEN = re.compile(rb"\[[^\]]*\]|/[^\s/\[\]()<>]+|\([^)]*\)|<<|>>|[-+]?\d*\.?\d+|[A-Za-z'\"*]+")


def mul(a, b):
    return [
        a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4], a[4] * b[1] + a[5] * b[3] + b[5],
    ]


def strokes(content):
    ctm = [1, 0, 0, 1, 0, 0]
    stack = []
    width, cap, join, dash, phase = 1.0, 0, 0, [], 0.0
    path, cur = [], None
    operands = []

    def pt(x, y):
        return (round(ctm[0] * x + ctm[2] * y + ctm[4], 3), round(ctm[1] * x + ctm[3] * y + ctm[5], 3))

    for tok in TOKEN.findall(content):
        if re.fullmatch(rb"[-+]?\d*\.?\d+", tok) or tok.startswith((b"[", b"/", b"(", b"<<", b">>")):
            operands.append(tok)
            continue
        op = tok.decode("latin-1")
        nums = []
        for o in operands:
            try:
                nums.append(float(o))
            except ValueError:
                pass
        if op == "q":
            stack.append((ctm[:], width, cap, join, dash[:], phase))
        elif op == "Q" and stack:
            ctm, width, cap, join, dash, phase = stack.pop()
        elif op == "cm" and len(nums) >= 6:
            ctm = mul(nums[-6:], ctm)
        elif op == "w" and nums:
            width = nums[-1]
        elif op == "J" and nums:
            cap = int(nums[-1])
        elif op == "j" and nums:
            join = int(nums[-1])
        elif op == "d" and operands:
            arr = [o for o in operands if o.startswith(b"[")]
            dash = [float(x) for x in re.findall(rb"[-+]?\d*\.?\d+", arr[-1])] if arr else []
            phase = nums[-1] if nums else 0.0
        elif op == "m" and len(nums) >= 2:
            cur = [pt(*nums[-2:])]
            path.append(cur)
        elif op == "l" and len(nums) >= 2 and cur is not None:
            cur.append(pt(*nums[-2:]))
        elif op == "c" and len(nums) >= 6 and cur is not None:
            cur.append(pt(*nums[-2:]))
        elif op == "re" and len(nums) >= 4:
            x, y, w, h = nums[-4:]
            cur = [pt(x, y), pt(x + w, y), pt(x + w, y + h), pt(x, y + h), pt(x, y)]
            path.append(cur)
        elif op == "h" and cur:
            cur.append(cur[0])
        elif op in ("S", "s", "B", "B*", "b", "b*"):
            yield {"width": width, "cap": cap, "join": join, "dash": dash, "phase": phase,
                   "subpaths": path}
            path, cur = [], None
        elif op in ("f", "F", "f*", "n", "W", "W*"):
            if op not in ("W", "W*"):
                path, cur = [], None
        operands = []


def main():
    data = open(sys.argv[1], "rb").read()
    pages = page_contents(data)
    which = [int(sys.argv[2]) - 1] if len(sys.argv) > 2 else range(len(pages))
    for i in which:
        for s in strokes(pages[i]):
            s["page"] = i + 1
            print(json.dumps(s))


if __name__ == "__main__":
    main()
