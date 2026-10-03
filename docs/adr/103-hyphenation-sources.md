# ADR 103 — Hyphenation sources and their precedence

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-text/src/hyphenate.rs`, `crates/paged-text/src/libhyphen.rs`, `crates/paged-text/patterns/`

## Context

The composer ([ADR 102](102-text-stack.md)) inserts a break opportunity at each point the
hyphenation patterns allow (`crates/paged-text/src/hyphenate.rs:32-34`). Until 2026-09-08
the patterns came only from the `hypher` crate's embedded tries.

The module doc records what changed: the two sources "are not equivalent and the difference
is measurable". On 25 English words swept in InDesign, the open `hyph_en_US.dic` from
LibreOffice "agrees on 24 and hypher's trie on 16, because the LibreOffice file folds in
the TUGboat hyphenation-exception log and the trie carries only the base patterns"
(`crates/paged-text/src/hyphenate.rs:17-30`). The libhyphen `.dic` format is described
there as "the same FORMAT Adobe hyphenates with".

Not every language's `.dic` file can be shipped. `crates/paged-text/patterns/README.md:18-40`
states the rule: only files whose upstream licence is compatible with this repository's
licence (see `LICENSE.md`) are vendored; the German, French, Italian and Portuguese files
are LGPL or GPL and are "Deliberately NOT vendored, pending a licensing decision".

A second measurement settled whose limits apply. A `.dic` file declares its own
`LEFTHYPHENMIN` / `RIGHTHYPHENMIN`; InDesign takes breaks the file's `RIGHTHYPHENMIN 3`
would forbid (`com-put-er`, `print-er`), "all measured on InDesign 20.0.1 by sweeping each
word's frame width" (`crates/paged-text/src/hyphenate.rs:322-331`).

## Decision

Hyphenation is Liang patterns from two sources, chosen per language, and the paragraph's
own settings decide which of the pattern breaks are allowed. In order:

1. **A soft hyphen wins.** A word that carries U+00AD breaks only at its soft hyphens; the
   dictionary is not consulted for that word.
2. **Vendored `.dic` files** for US English, British English, Spanish and Dutch. They are
   kept verbatim under `patterns/`, embedded with `include_bytes!`, and read by the crate's
   own parser in `libhyphen.rs`. They are matched with the paragraph's limits (after first,
   before last, minimum word length; defaults 2, 2 and 5). The file's declared minima are
   read and not applied.
3. **`hypher`'s tries** for German, French, Italian and Portuguese. Here the trie's own
   language bounds stand and the paragraph's limits can only tighten them.
4. **No patterns, no hyphenation.** A paragraph whose language is `[No Language]` or one
   the engine holds no patterns for is not hyphenated. A paragraph that names no language
   uses US English.

## Evidence

- `crates/paged-text/src/hyphenate.rs:86-112` — `Language::vendored`: four `include_bytes!` arms, `None` for the other four languages
- `crates/paged-text/src/hyphenate.rs:322-338` — vendored patterns matched with the paragraph's limits; "The file's own `RIGHTHYPHENMIN 3` is read and deliberately not applied"
- `crates/paged-text/src/hyphenate.rs:339-347` — the `hypher` path: bounds stand, limits only tighten, and why loosening them was rejected
- `crates/paged-text/src/hyphenate.rs:265-267`, `:429-432` — a word with a soft hyphen "hyphenates ONLY there"; `:124-128`, `:413-419` — unknown language means no hyphenation, no language means US English
- `crates/paged-text/src/libhyphen.rs:45-49` — `declared_left_min` / `declared_right_min`: "Kept for reference"; `:32-36` — what the parser skips
- `crates/paged-text/patterns/README.md:3-5`, `:18-40` — provenance (fetched 2026-09-08), the vendored files and the four that are not; `crates/paged-text/Cargo.toml:21` — `hypher = "0.1"`

## Alternatives considered

- `hypher` alone: the state before 2026-09-08, 16 of 25 measured words.
- Letting the pattern set's own minimum stand over the paragraph's limits everywhere:
  decided in commit `0330e9a` and reversed for the `.dic` files the same day in `e0461f0`,
  whose message calls the earlier conclusion "wrong, and it is retracted here". It still
  holds for the `hypher` languages: loosening their bounds was "measured and rejected",
  because the trie lacks the exception list and the looser bound unlocked breaks InDesign
  refuses (`crates/paged-text/src/hyphenate.rs:341-345`).
- Converting French from its upstream TeX source is named in the patterns README as a
  route that would be licence-clean. It is not done.

## Consequences

Four languages read vendored `.dic` files and four read `hypher`'s tries. The repository
measures the difference between the two sources for US English only: 24 of 25 words with
the `.dic` file, 16 with the trie.

The four `.dic` files (about 386 KB together) are compiled into every artifact that links
`paged-text`. Each is parsed on first use, so a document pays only for the languages it
names (`crates/paged-text/src/hyphenate.rs:219-221`).

The `.dic` parser is partial. `NEXTLEVEL` and the non-standard `=` and `/` pattern forms
are not implemented; a line that uses them is skipped (`crates/paged-text/src/libhyphen.rs:84-92`).
No pattern line in the four vendored files uses them. The module doc and the patterns README
count eight such lines, all Spanish (`crates/paged-text/src/libhyphen.rs:32-36`); the only
lines in `hyph_es.dic` that contain `/` or `=` are `%` comments. US and British English are separate
dictionaries with separate ids, and the id is part of the layout cache key
(`crates/paged-text/src/hyphenate.rs:190-192`).

Two comments predate the vendored files. `crates/paged-text/src/lib.rs:20` describes
hyphenation as "TeX patterns by default; Proximity if licensed", and
`crates/paged-text/src/compose.rs:611-631` logs it as "TeX patterns (hypher)". The
repository contains no Proximity dictionary.

## Related

- [ADR 102](102-text-stack.md), [ADR 120](120-indesign-is-the-oracle.md) — the breakers that consume the break opportunities; the measuring method used here
