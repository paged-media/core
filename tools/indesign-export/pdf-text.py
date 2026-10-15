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
"""Dump every text-showing operator an InDesign PDF page paints.

    python3 tools/indesign-export/pdf-text.py out/ruby-kenten.pdf [page]

Prints one JSON object per `Tj` / `TJ` / `'` / `"`: the font's base name,
its size as drawn (the `Tf` size through the text and graphics matrices,
horizontally and vertically), the origin of its first glyph in page space
(pt, y DOWN from the page's top, as InDesign's rulers measure), the
string's raw codes, and the character and word spacing in force (`Tc`,
`Tw`, text-space units: times the drawn size, the room InDesign sets
between characters, as it spreads ruby and kenten). Answers what the scripting DOM cannot: where and how
big InDesign sets the text it generates itself, such as ruby and emphasis
marks. Standard library only, over `pdf-strokes.py`'s stream reading.
"""
import importlib.util
import json
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


def font_names(objs):
    """Resource name -> BaseFont, over every font dictionary in the file."""
    base = {}
    for oid, body in objs.items():
        m = re.search(rb"/BaseFont\s*/([^\s/>]+)", body)
        if m and re.search(rb"/Type\s*/Font\b", body):
            base[oid] = m.group(1).decode("latin-1")
    names = {}
    for body in objs.values():
        for d in re.findall(rb"/Font\s*<<(.*?)>>", body, re.S):
            for name, ref in re.findall(rb"/([^\s/]+)\s+(\d+) 0 R", d):
                if int(ref) in base:
                    names[name.decode("latin-1")] = base[int(ref)]
    return names


def shows(content, names, page_height):
    ctm = [1, 0, 0, 1, 0, 0]
    stack = []
    tm = tlm = [1, 0, 0, 1, 0, 0]
    font, size, leading, rise, hscale, tc, tw = None, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0
    operands = []
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
        elif op == "Tf" and operands:
            font = operands[-2].decode("latin-1").lstrip("/") if len(operands) >= 2 else font
            size = nums[-1] if nums else size
        elif op == "TL" and nums:
            leading = nums[-1]
        elif op == "Ts" and nums:
            rise = nums[-1]
        elif op == "Tc" and nums:
            tc = nums[-1]
        elif op == "Tw" and nums:
            tw = nums[-1]
        elif op == "Tz" and nums:
            hscale = nums[-1]
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
        elif op in ("Tj", "TJ", "'", '"'):
            if op in ("'", '"'):
                tlm = pdf.mul([1, 0, 0, 1, 0, -leading], tlm)
                tm = tlm
            m = pdf.mul([size * hscale / 100.0, 0, 0, size, 0, rise], pdf.mul(tm, ctm))
            codes = b"".join(o for o in operands if o.startswith((b"[", b"(", b"<")))
            yield {
                "font": names.get(font, font), "size_x": round((m[0] ** 2 + m[1] ** 2) ** 0.5, 3),
                "size_y": round((m[2] ** 2 + m[3] ** 2) ** 0.5, 3),
                "x": round(m[4], 3), "y": round(page_height - m[5], 3),
                "codes": codes.decode("latin-1"), "tc": tc, "tw": tw,
            }
        operands = []


def main():
    data = open(sys.argv[1], "rb").read()
    objs = pdf.all_objects(data)
    names = font_names(objs)
    heights = [float(h) for h in re.findall(rb"/MediaBox\s*\[\s*[-\d.]+\s+[-\d.]+\s+[-\d.]+\s+([-\d.]+)", data)]
    pages = pdf.page_contents(data)
    which = [int(sys.argv[2]) - 1] if len(sys.argv) > 2 else range(len(pages))
    for i in which:
        for s in shows(pages[i], names, heights[0] if heights else 841.89):
            s["page"] = i + 1
            print(json.dumps(s, ensure_ascii=False))


if __name__ == "__main__":
    main()
