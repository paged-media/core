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
"""Dump every glyph an InDesign PDF page paints, one JSON object each.

    python3 tools/indesign-export/pdf-glyphs.py out/sample.pdf [page]

Where `pdf-text.py` reports a text-showing operator by its first glyph,
this follows the operator glyph by glyph: each glyph's character (from the
font's `ToUnicode` map), the point its outline is drawn from (its
horizontal origin, pt, y DOWN from the page's top), its drawn size, its
advance in font units, the font's writing mode (`h`, or `v` for a
vertical `Identity-V` font, whose glyphs advance down the page) and the
angle its text matrix turns it by (0 upright, 90 turned clockwise). A
glyph's em box follows from its origin: `x .. x + size * w / 1000`
across, its face's em top and bottom about `y` up and down. Standard
library only, over `pdf-strokes.py`'s stream reading.
"""
import importlib.util
import json
import math
import os
import re
import sys

sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location(
    "pdf_strokes", os.path.join(os.path.dirname(os.path.abspath(__file__)), "pdf-strokes.py"))
pdf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pdf)

TOKEN = re.compile(rb"\[(?:\([^)]*\)|<[0-9A-Fa-f\s]*>|[^\]])*\]|<[0-9A-Fa-f\s]*>|\((?:\\.|[^\\)])*\)"
                   rb"|/[^\s/\[\]()<>]+|<<|>>|[-+]?\d*\.?\d+|[A-Za-z'\"*]+")


def ref(body, key):
    m = re.search(rb"/" + key + rb"\s+(\d+) 0 R", body)
    return int(m.group(1)) if m else None


def array(objs, body, key):
    """The array value of `key`, following one indirect reference."""
    m = re.search(rb"/" + key + rb"\s*(\[(?:[^\[\]]|\[[^\]]*\])*\]|\d+ 0 R)", body)
    if not m:
        return None
    v = m.group(1)
    if not v.startswith(b"["):
        v = objs.get(int(v.split()[0]), b"").strip()
    return v


def numbers(text):
    return [float(x) for x in re.findall(rb"[-+]?\d*\.?\d+", text)]


def parse_w(text, per):
    """A CIDFont `W` (per=1) or `W2` (per=3) array: cid -> tuple."""
    out = {}
    if not text:
        return out
    toks = re.findall(rb"\[[^\]]*\]|[-+]?\d*\.?\d+", text.strip()[1:-1])
    i = 0
    while i < len(toks):
        first = int(float(toks[i]))
        nxt = toks[i + 1]
        if nxt.startswith(b"["):
            vals = numbers(nxt)
            for k in range(len(vals) // per):
                out[first + k] = tuple(vals[k * per:(k + 1) * per])
            i += 2
        else:
            last = int(float(nxt))
            vals = [float(v) for v in toks[i + 2:i + 2 + per]]
            for c in range(first, last + 1):
                out[c] = tuple(vals)
            i += 2 + per
    return out


def to_unicode(stream):
    """A ToUnicode CMap: code (int) -> text."""
    out = {}
    if not stream:
        return out

    def text(hexstr):
        b = bytes.fromhex(hexstr.decode())
        return b.decode("utf-16-be", "replace")

    for block in re.findall(rb"beginbfchar(.*?)endbfchar", stream, re.S):
        for src, dst in re.findall(rb"<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>", block):
            out[int(src, 16)] = text(dst)
    for block in re.findall(rb"beginbfrange(.*?)endbfrange", stream, re.S):
        for a, b, dst in re.findall(rb"<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>\s*(<[0-9A-Fa-f]+>|\[[^\]]*\])", block):
            lo, hi = int(a, 16), int(b, 16)
            if dst.startswith(b"["):
                for k, d in enumerate(re.findall(rb"<([0-9A-Fa-f]+)>", dst)):
                    out[lo + k] = text(d)
            else:
                base = bytes.fromhex(dst[1:-1].decode())
                for k in range(hi - lo + 1):
                    v = int.from_bytes(base, "big") + k
                    out[lo + k] = v.to_bytes(len(base), "big").decode("utf-16-be", "replace")
    return out


class Font:
    def __init__(self, objs, oid):
        body = objs.get(oid, b"")
        base = re.search(rb"/BaseFont\s*/([^\s/>]+)", body)
        self.name = base.group(1).decode("latin-1").split("+")[-1] if base else str(oid)
        self.type0 = b"/Type0" in body
        enc = re.search(rb"/Encoding\s*/([^\s/>]+)", body)
        self.vertical = bool(enc and enc.group(1).endswith(b"-V"))
        tu = ref(body, b"ToUnicode")
        self.unicode = to_unicode(pdf.stream_of(objs.get(tu, b""))) if tu else {}
        if self.type0:
            desc = array(objs, body, b"DescendantFonts")
            cid = objs.get(int(numbers(desc)[0]), b"") if desc else b""
            dw = re.search(rb"/DW\s+([-\d.]+)", cid)
            self.dw = float(dw.group(1)) if dw else 1000.0
            self.w = parse_w(array(objs, cid, b"W"), 1)
            dw2 = array(objs, cid, b"DW2")
            self.dw2 = numbers(dw2) if dw2 else [880.0, -1000.0]
            self.w2 = parse_w(array(objs, cid, b"W2"), 3)
        else:
            first = re.search(rb"/FirstChar\s+(\d+)", body)
            self.first = int(first.group(1)) if first else 0
            self.widths = numbers(array(objs, body, b"Widths") or b"[]")

    def codes(self, raw):
        if self.type0:
            return [int.from_bytes(raw[i:i + 2], "big") for i in range(0, len(raw) - 1, 2)]
        return list(raw)

    def width(self, code):
        if self.type0:
            return self.w.get(code, (self.dw,))[0]
        k = code - self.first
        return self.widths[k] if 0 <= k < len(self.widths) else 0.0

    def vmetrics(self, code):
        """(w1y, vx, vy) in font units for a vertical font."""
        if code in self.w2:
            return self.w2[code]
        return (self.dw2[1], self.width(code) / 2.0, self.dw2[0])


def string_bytes(tok):
    if tok.startswith(b"<"):
        return bytes.fromhex(re.sub(rb"\s", b"", tok[1:-1]).decode())
    s = tok[1:-1]
    out, i = bytearray(), 0
    while i < len(s):
        c = s[i]
        if c == 0x5C and i + 1 < len(s):
            n = s[i + 1]
            esc = {ord("n"): 10, ord("r"): 13, ord("t"): 9, ord("b"): 8, ord("f"): 12}
            if n in esc:
                out.append(esc[n]); i += 2
            elif 0x30 <= n <= 0x37:
                m = re.match(rb"[0-7]{1,3}", s[i + 1:i + 4])
                out.append(int(m.group(0), 8) & 0xFF); i += 1 + len(m.group(0))
            else:
                out.append(n); i += 2
        else:
            out.append(c); i += 1
    return bytes(out)


def dict_body(objs, body, key):
    """The dictionary value of `key` in `body`, inline or by reference."""
    m = re.search(rb"/" + key + rb"\s*(\d+) 0 R", body)
    if m:
        return objs.get(int(m.group(1)), b"")
    m = re.search(rb"/" + key + rb"\s*<<", body)
    if not m:
        return None
    depth, i = 0, m.end() - 2
    while i < len(body):
        if body[i:i + 2] == b"<<":
            depth += 1
            i += 2
        elif body[i:i + 2] == b">>":
            depth -= 1
            i += 2
            if depth == 0:
                return body[m.end() - 2:i]
        else:
            i += 1
    return None


def pages(data, objs):
    """Each page's (content stream, font name -> object id), in page order.
    Font names are resolved per page (and its parent Pages node): InDesign
    reuses `/T1_0` for different fonts on different pages."""
    out = []

    def walk(oid, inherited):
        body = objs.get(oid, b"")
        res = dict_body(objs, body, b"Resources") or inherited
        if re.search(rb"/Type\s*/Pages\b", body):
            k = re.search(rb"/Kids\s*\[([^\]]*)\]", body)
            for kid in re.findall(rb"(\d+) 0 R", k.group(1) if k else b""):
                walk(int(kid), res)
        elif re.search(rb"/Type\s*/Page\b", body):
            fonts = {}
            fdict = dict_body(objs, res or b"", b"Font") or b""
            for name, ref in re.findall(rb"/([^\s/<>]+)\s+(\d+) 0 R", fdict):
                fonts[name.decode("latin-1")] = int(ref)
            c = re.search(rb"/Contents\s*(\[[^\]]*\]|\d+ 0 R)", body)
            refs = [int(x) for x in re.findall(rb"(\d+) 0 R", c.group(1))] if c else []
            content = b"".join(pdf.stream_of(objs[r]) or b"" for r in refs if r in objs)
            out.append((content, fonts))

    for oid, body in objs.items():
        root = re.search(rb"/Pages\s+(\d+) 0 R", body)
        if root and re.search(rb"/Type\s*/Catalog\b", body):
            walk(int(root.group(1)), None)
            break
    return out


def glyphs(content, objs, refs, page_height):
    cache = {}
    ctm, stack = [1, 0, 0, 1, 0, 0], []
    tm = tlm = [1, 0, 0, 1, 0, 0]
    font, size, leading, rise, th, tc, tw = None, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0
    operands = []

    def show(raw):
        nonlocal tm
        f = font
        for code in f.codes(raw):
            w0 = f.width(code)
            trm = pdf.mul([size * th, 0, 0, size, 0, rise], pdf.mul(tm, ctm))
            if f.vertical:
                w1y, vx, vy = f.vmetrics(code)
                # The outline is drawn from the horizontal origin, which
                # sits the position vector (vx, vy) before the vertical one.
                ox, oy = -vx / 1000.0, -vy / 1000.0
                origin = (trm[0] * ox + trm[2] * oy + trm[4], trm[1] * ox + trm[3] * oy + trm[5])
            else:
                origin = (trm[4], trm[5])
            angle = math.degrees(math.atan2(-trm[1], trm[0])) + 0.0
            yield {
                "char": f.unicode.get(code, ""), "code": code, "font": f.name,
                "size": round(math.hypot(trm[2], trm[3]), 3),
                "xscale": round(math.hypot(trm[0], trm[1]) / max(math.hypot(trm[2], trm[3]), 1e-9), 4),
                "mode": "v" if f.vertical else "h",
                "x": round(origin[0], 3), "y": round(page_height - origin[1], 3),
                "w": w0, "angle": round(angle, 2),
            }
            space = tw if (not f.type0 and code == 32) else 0.0
            if f.vertical:
                ty = w1y / 1000.0 * size + tc + space
                tm = pdf.mul([1, 0, 0, 1, 0, ty], tm)
            else:
                tx = (w0 / 1000.0 * size + tc + space) * th
                tm = pdf.mul([1, 0, 0, 1, tx, 0], tm)

    for tok in TOKEN.findall(content):
        if re.fullmatch(rb"[-+]?\d*\.?\d+", tok) or tok.startswith((b"[", b"/", b"(", b"<")):
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
            stack.append(ctm[:])
        elif op == "Q" and stack:
            ctm = stack.pop()
        elif op == "cm" and len(nums) >= 6:
            ctm = pdf.mul(nums[-6:], ctm)
        elif op == "BT":
            tm = tlm = [1, 0, 0, 1, 0, 0]
        elif op == "Tf" and len(operands) >= 2:
            name = operands[-2].decode("latin-1").lstrip("/")
            oid = refs.get(name)
            if oid not in cache:
                cache[oid] = Font(objs, oid)
            font, size = cache[oid], nums[-1] if nums else size
        elif op == "TL" and nums:
            leading = nums[-1]
        elif op == "Ts" and nums:
            rise = nums[-1]
        elif op == "Tc" and nums:
            tc = nums[-1]
        elif op == "Tw" and nums:
            tw = nums[-1]
        elif op == "Tz" and nums:
            th = nums[-1] / 100.0
        elif op == "Tm" and len(nums) >= 6:
            tm = tlm = nums[-6:]
        elif op in ("Td", "TD") and len(nums) >= 2:
            tlm = pdf.mul([1, 0, 0, 1, nums[-2], nums[-1]], tlm)
            tm = tlm
            if op == "TD":
                leading = -nums[-1]
        elif op == "T*":
            tlm = pdf.mul([1, 0, 0, 1, 0, -leading], tlm)
            tm = tlm
        elif op in ("Tj", "'", '"') and font is not None:
            if op in ("'", '"'):
                tlm = pdf.mul([1, 0, 0, 1, 0, -leading], tlm)
                tm = tlm
            for o in operands:
                if o.startswith((b"(", b"<")):
                    yield from show(string_bytes(o))
        elif op == "TJ" and font is not None and operands:
            for part in re.findall(rb"\((?:\\.|[^\\)])*\)|<[0-9A-Fa-f\s]*>|[-+]?\d*\.?\d+", operands[-1][1:-1]):
                if part.startswith((b"(", b"<")):
                    yield from show(string_bytes(part))
                else:
                    adj = -float(part) / 1000.0 * size
                    tm = pdf.mul([1, 0, 0, 1, 0, adj] if font.vertical else [1, 0, 0, 1, adj * th, 0], tm)
        operands = []


def main():
    data = open(sys.argv[1], "rb").read()
    objs = pdf.all_objects(data)
    heights = [float(h) for h in re.findall(rb"/MediaBox\s*\[\s*[-\d.]+\s+[-\d.]+\s+[-\d.]+\s+([-\d.]+)", data)]
    all_pages = pages(data, objs)
    which = [int(sys.argv[2]) - 1] if len(sys.argv) > 2 else range(len(all_pages))
    for i in which:
        content, refs = all_pages[i]
        for g in glyphs(content, objs, refs, heights[0] if heights else 841.89):
            g["page"] = i + 1
            print(json.dumps(g, ensure_ascii=False))


if __name__ == "__main__":
    main()
