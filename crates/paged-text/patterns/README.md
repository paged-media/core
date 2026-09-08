# Vendored hyphenation patterns

libhyphen (`hyph_*.dic`) pattern files, verbatim from
[LibreOffice/dictionaries](https://github.com/LibreOffice/dictionaries),
fetched 2026-09-08. Each file keeps its upstream licence text beside it.

## Why these, and why not the others

Adobe's linguistics is libhyphen: InDesign 2025 ships an
`AdobeHunspellPlugin` whose `hyph_en_US.dic` — 25,221 patterns, marked
Adobe Confidential — reproduces InDesign's measured line breaks exactly
when Liang's algorithm is run over it with the paragraph's own limits.
That file is not ours to ship. The open `hyph_en_US.dic` here is the
closest licence-clean equivalent: on 25 English words swept in InDesign
it agrees on 24 against 16 for the pattern trie we used before, because
it carries the TUGboat hyphenation-exception log that the trie omits.

Only languages whose upstream licence is compatible with this repo's
MPL-2.0 OR PMEL dual licence are vendored:

| language | file | licence |
|---|---|---|
| English (US) | `hyph_en_US.dic` | BSD-style — `README_hyph_en_US.txt` |
| English (GB) | `hyph_en_GB.dic` | BSD-style — `README_hyph_en_GB.txt` |
| Spanish | `hyph_es.dic` | GPL-3+ / LGPL-3+ / **MPL-1.1+** (we take MPL) — `README_hyph_es.txt`, `LICENSE_es.md` |
| Dutch | `hyph_nl_NL.dic` | Revised BSD and/or CC BY 3.0 — `LICENSE_nl_NL.txt` |

Deliberately NOT vendored, pending a licensing decision — a commercial
licence sits beside the open one here, so copyleft data is not something
to adopt quietly:

| language | upstream licence |
|---|---|
| German | LGPL 2+ (patterns themselves LPPL) |
| French | LGPL — though upstream `hyph-fr.tex` is MIT from v2.12, so converting from TeX ourselves would be clean |
| Italian | LGPL |
| Portuguese | GPL |

Those four keep using `hypher`'s embedded tries, which are Apache-2.0 /
MIT. Nothing regresses for them.

## Format

Line 1 is the encoding (`UTF-8`, or `ISO8859-1` for Dutch — decoded at
load, the file is kept byte-for-byte as published). Then
`LEFTHYPHENMIN` / `RIGHTHYPHENMIN` / `COMPOUND*` declarations, `%`
comments, and one Liang pattern per line. `NEXTLEVEL` and the
non-standard `=`/`/` pattern forms are not implemented; the eight lines
that use them (all in Spanish) are skipped, and `libhyphen.rs` says so.
