#!/usr/bin/env bash
# corpus/generated/text-diff.sh — the TEXT half of the InDesign oracle.
#
# The pixel gate (diff.sh) cannot see missing words: twelve glyphs on an
# A4 page are under 1% of its pixels and move neither the mean ΔE nor the
# p99. `conditions` proved it: InDesign's reference printed the run its
# hidden condition should have suppressed, and the gate passed at a mean
# ΔE of 0.105. This gate reads WHAT is printed:
#
#   reference: pdftotext -bbox-layout of InDesign's PDF, one record per line
#   candidate: paged-inspect --emit-breaks, one record per laid-out line
#
# and compares, page by page, the sequence of lines (whitespace
# normalised). The candidate's records mark what the engine generated
# (`generated` spans): a list's marker is dropped on both sides — from the
# candidate exactly, from the reference where one of the page's lines
# opens with a marker the engine printed there (InDesign's PDF sets a
# marker in a larger size as a line of its own) — and a tab leader is
# printed on both, the candidate's from the record. Two modes, per fixture:
#   "text_gate": true     the page's CHARACTERS as a multiset: a missing or
#                         extra word fails, a different line break or
#                         hyphenation point does not
#   "text_gate": "lines"  additionally the page's lines: a different break fails
# It reuses render-diff.sh's
# per-fixture fonts (`<stem>.fonts.sh`), so both gates measure the same
# composition.
#
# Usage:
#   ./corpus/generated/text-diff.sh                 # every fixture with "text_gate": true
#   ./corpus/generated/text-diff.sh text keeps      # these fixtures (hard)
#   IDML_TEXT_GATE=advisory ./text-diff.sh ...      # report, never fail
#   IDML_TEXT_MODE=lines IDML_TEXT_GATE=advisory ./text-diff.sh   # survey every fixture in lines mode
#
# Exit: 0 all compared fixtures match (or advisory), 1 a mismatch, 2 setup error.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GENERATED_DIR="$ROOT/corpus/generated"
FONTS="$ROOT/corpus/fonts"
THRESHOLDS="$GENERATED_DIR/fidelity-thresholds.json"
OUT="${IDML_TEXT_DIFF_OUT:-/tmp/idml-text-diff}"
GATE="${IDML_TEXT_GATE:-hard}"

command -v pdftotext >/dev/null || { echo "install poppler (pdftotext)"; exit 2; }

fixtures=()
if [ "$#" -gt 0 ]; then
    fixtures=("$@")
else
    while IFS= read -r line; do
        [ -n "$line" ] && fixtures+=("$line")
    done < <(python3 -c "
import json
print('\n'.join(f['name'] for f in json.load(open('$THRESHOLDS'))['fixtures'] if f.get('text_gate')))
")
fi
if [ "${#fixtures[@]}" -eq 0 ]; then
    echo "text-diff: no fixture opts in (\"text_gate\": true in fidelity-thresholds.json)"
    exit 0
fi

(cd "$ROOT" && cargo build -q --release -p paged-gen --bin paged-gen \
    -p paged-renderer --bin paged-inspect)
GEN="$ROOT/target/release/paged-gen"
INSPECT="$ROOT/target/release/paged-inspect"

mkdir -p "$OUT"
fail=0
for NAME in "${fixtures[@]}"; do
    pdf="$GENERATED_DIR/$NAME.pdf"
    if [ ! -f "$pdf" ]; then
        echo "[$NAME] no reference PDF — skipped"
        continue
    fi
    "$GEN" emit --sample "$NAME" --out "$GENERATED_DIR" >/dev/null
    # The same fonts render-diff.sh registers (its defaults, then the
    # fixture's own `<stem>.fonts.sh`).
    DEFAULT_FONT="$FONTS/SourceSerif4.ttf"
    FONT_FLAGS=(
        --font-family "Open Sans=$FONTS/OpenSans.ttf"
        --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
        --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
    )
    if [ -f "$GENERATED_DIR/$NAME.fonts.sh" ]; then
        # shellcheck disable=SC1090
        . "$GENERATED_DIR/$NAME.fonts.sh"
    fi
    # Date variables print the day InDesign exported the reference (as in
    # render-diff.sh).
    DATE_FLAG=()
    META="$GENERATED_DIR/$NAME.export.meta.json"
    if [ -f "$META" ]; then
        EXPORT_DAY="$(python3 - "$META" <<'PY'
import json, sys, datetime
raw = json.load(open(sys.argv[1])).get("exported_at", "")
try:
    print(datetime.datetime.strptime(" ".join(raw.split()[1:4]), "%b %d %Y").strftime("%Y-%m-%d"))
except ValueError:
    pass
PY
)"
        [ -n "$EXPORT_DAY" ] && DATE_FLAG=(--document-date "$EXPORT_DAY")
    fi
    dir="$OUT/$NAME"; rm -rf "$dir"; mkdir -p "$dir"
    "$INSPECT" "$GENERATED_DIR/$NAME.idml" --default-font "$DEFAULT_FONT" \
        "${FONT_FLAGS[@]}" ${DATE_FLAG[@]+"${DATE_FLAG[@]}"} --no-missing-image-placeholder \
        --emit-breaks "$dir/cand.jsonl" >/dev/null 2>&1
    pdftotext -bbox-layout "$pdf" "$dir/ref.html"
    # IDML_TEXT_MODE=lines surveys every fixture in lines mode (which of
    # the "chars" fixtures could be promoted); the gate's own mode is the
    # fixture's `text_gate`.
    MODE="${IDML_TEXT_MODE:-$(python3 -c "
import json
spec = next((f for f in json.load(open('$THRESHOLDS'))['fixtures'] if f['name'] == '$NAME'), {})
print('lines' if spec.get('text_gate') == 'lines' else 'chars')
")}"
    set +e
    python3 - "$dir/ref.html" "$dir/cand.jsonl" "$NAME" "$dir/text-gate.json" "$MODE" <<'PY'
import json, re, sys, html
from collections import Counter
from xml.etree import ElementTree as ET

ref_path, cand_path, name, out_path = sys.argv[1:5]

def norm(s):
    s = s.replace("\u00a0", " ").replace("\u00ad", "").replace("\t", " ")
    return re.sub(r"\s+", " ", s).strip()

def squeeze(s):
    return re.sub(r"\s+", "", s)

# Candidate: one record per laid-out line. Its `generated` spans (byte
# ranges of `source_text`) say what the engine printed that the story
# does not hold. A list marker is set aside, per page, to be dropped from
# the reference too; a tab leader prints its repeats in place of the tab
# (`text`), as InDesign's PDF carries them; the rest (page numbers,
# footnote numbers, variables) print on both sides and stay.
cand = []
markers = {}
# Per page, the characters of one copy of each leader the engine drew: a
# leader's copy count turns on a sub-point gap, so the two sides may set
# one copy more or fewer per leader (macOS and Linux builds have).
leader_slack = {}
for l in open(cand_path, encoding="utf-8"):
    r = json.loads(l)
    raw = r.get("source_text", "").encode("utf-8")
    out, at = [], 0
    for g in sorted(r.get("generated", []), key=lambda g: g["start"]):
        if g["kind"] not in ("list_marker", "tab_leader") or g["start"] < at:
            continue
        out.append(raw[at:g["start"]].decode("utf-8", "replace"))
        if g["kind"] == "list_marker":
            m = squeeze(raw[g["start"]:g["end"]].decode("utf-8", "replace"))
            if m:
                markers.setdefault(r["page_idx"], Counter())[m] += 1
            out.append(" ")
        else:
            drawn = g.get("text", "")
            out.append(" " + drawn + " ")
            unit = Counter(ch for ch in set(drawn) if not ch.isspace())
            if unit:
                leader_slack.setdefault(r["page_idx"], Counter()).update(unit)
        at = g["end"]
    out.append(raw[at:].decode("utf-8", "replace"))
    text = norm("".join(out))
    if text:
        cand.append({"page": r["page_idx"], "text": text, "y": r["baseline_y_pt"]})

def strip_marker(text, page_markers):
    """`text` without the engine's marker it opens with (whitespace inside
    the marker aside: tracking splits `1.` into `1 .`), and the marker;
    the longest of the page's markers still unmatched wins."""
    for m in sorted((m for m, n in page_markers.items() if n > 0), key=len, reverse=True):
        i = k = 0
        while k < len(m) and i < len(text):
            if text[i].isspace():
                i += 1
            elif text[i] == m[k]:
                i += 1
                k += 1
            else:
                break
        if k == len(m):
            return text[i:].strip(), m
    return text, None

# Reference: <page><flow><block><line><word xMin yMin xMax yMax>w</word>…
raw = open(ref_path, encoding="utf-8").read()
raw = raw[raw.index("<body"):raw.rindex("</body>") + 7]
body = ET.fromstring(re.sub(r"&(?!amp;|lt;|gt;|quot;|apos;)", "&amp;", raw))
ref = []
unmatched = {}
pages = [p for p in body.iter() if p.tag.endswith("page")]
for pi, page in enumerate(pages):
    rows = []
    for line in [l for l in page.iter() if l.tag.endswith("line")]:
        words = [w for w in line if w.tag.endswith("word")]
        if not words:
            continue
        text = norm(" ".join(html.unescape(w.text or "") for w in words))
        if text:
            rows.append({"page": pi, "text": text,
                         "y": max(float(w.get("yMax")) for w in words),
                         "x": min(float(w.get("xMin")) for w in words)})
    # A marker the engine printed on this page is dropped where a line
    # opens with it, a line that is only the marker first.
    left = Counter(markers.get(pi, {}))
    rows.sort(key=lambda r: (squeeze(r["text"]) not in left, r["y"]))
    for row in rows:
        row["text"], m = strip_marker(row["text"], left)
        if m:
            left[m] -= 1
    if +left:
        unmatched[pi] = +left
    ref.extend(r for r in rows if r["text"])

def by_page(rows):
    out = {}
    for r in rows:
        out.setdefault(r["page"], []).append(r)
    for v in out.values():
        v.sort(key=lambda r: r["y"])
    return out

R, C = by_page(ref), by_page(cand)
mode = sys.argv[5] if len(sys.argv) > 5 else "chars"
# Whitespace, hyphens (a hyphenation point is a break, not text) and the
# placeholders that stand in for an anchored object or a marker.
SKIP = set(" \t\u00a0-\u00ad\u2010\u2011\ufffc\ufeff")

def chars(rows):
    return Counter(ch for r in rows for ch in r["text"] if ch not in SKIP)

def unhyphen(t):
    return t[:-1] if t.endswith("-") else t

def join_fragments(rows, cand_lines):
    """pdftotext splits a line whose word spaces are wide (a justified line
    with few words, a forced break) into several `<line>`s on one baseline.
    Join a left-to-right run of such fragments back into one line, but only
    where the joined text is a line the engine set: fragments that are
    really two columns on a shared baseline stay apart."""
    out, used = [], set()
    by_y = {}
    for i, r in enumerate(rows):
        by_y.setdefault(round(r["y"] * 2) / 2, []).append(i)
    for idxs in by_y.values():
        if len(idxs) < 2:
            continue
        idxs.sort(key=lambda i: rows[i].get("x", 0.0))
        k = 0
        while k < len(idxs):
            for j in range(len(idxs), k + 1, -1):
                run = idxs[k:j]
                joined = norm(" ".join(rows[i]["text"] for i in run))
                if unhyphen(joined) in cand_lines:
                    out.append(dict(rows[run[0]], text=joined))
                    used.update(run)
                    k = j - 1
                    break
            k += 1
    out.extend(r for i, r in enumerate(rows) if i not in used)
    return out

problems = []
leader_notes = []
# Only the pages the reference has: a sample that grew after its reference
# was exported has pages InDesign never set, which the pixel gate skips
# too (`max_pages_with_pdf`). They are counted, not compared.
compared = sorted(p for p in set(R) | set(C) if p < len(pages))
beyond = sorted(p for p in set(C) if p >= len(pages))
for page in compared:
    rr, cc = R.get(page, []), C.get(page, [])
    rc, ccnt = chars(rr), chars(cc)
    problem = {}
    slack = leader_slack.get(page, Counter())
    if rc != ccnt and slack:
        off = (rc - ccnt) + (ccnt - rc)
        if all(slack[ch] >= n for ch, n in off.items()):
            leader_notes.append({"page": page + 1, "leader_copies": dict(off)})
            rc = ccnt
    if rc != ccnt:
        problem["indesign_only"] = "".join(sorted((rc - ccnt).elements()))[:120]
        problem["paged_only"] = "".join(sorted((ccnt - rc).elements()))[:120]
    if mode == "lines":
        rr = join_fragments(rr, {unhyphen(c["text"]) for c in cc})
        rl = sorted(unhyphen(r["text"]) for r in rr)
        cl = sorted(unhyphen(c["text"]) for c in cc)
        if rl != cl:
            problem["lines_indesign_only"] = [t for t in rl if t not in cl][:8]
            problem["lines_paged_only"] = [t for t in cl if t not in rl][:8]
    if problem:
        problem["page"] = page + 1
        problems.append(problem)
# A marker the engine printed that no line of InDesign's opens with is
# reported, not failed: the marker is dropped on both sides, so this is the
# one place the gate sees a list number InDesign does not print.
notes = [{"page": p + 1, "markers_paged_only": sorted(unmatched[p].elements())[:8]}
         for p in compared if p in unmatched]
result = {"fixture": name, "mode": mode, "passed": not problems,
          "pages_compared": len(compared), "pages_beyond_reference": len(beyond),
          "lines_indesign": len(ref), "lines_paged": len(cand), "problems": problems,
          "notes": notes, "leader_notes": leader_notes}
json.dump(result, open(out_path, "w"), indent=2)
if problems:
    print(f"[{name}] TEXT MISMATCH ({mode}) on {len(problems)} page(s)")
    for p in problems[:6]:
        print(f"  page {p['page']}: " + json.dumps({k: v for k, v in p.items() if k != 'page'}, ensure_ascii=False)[:300])
    sys.exit(1)
extra = f"; {len(beyond)} page(s) past the reference not compared" if beyond else ""
print(f"[{name}] text ok ({len(ref)} lines, {result['pages_compared']} pages{extra})")
for n in notes:
    print(f"  note, page {n['page']}: list marker(s) InDesign does not print: "
          + " ".join(n["markers_paged_only"]))
for n in leader_notes:
    print(f"  note, page {n['page']}: leader copies differ (one per leader at most): "
          + json.dumps(n["leader_copies"], ensure_ascii=False))
PY
    rc=$?
    set -e
    [ $rc -ne 0 ] && fail=1
done

if [ "$fail" -ne 0 ] && [ "$GATE" != "advisory" ]; then
    exit 1
fi
exit 0
