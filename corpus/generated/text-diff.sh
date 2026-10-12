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
# normalised, generated list markers stripped). Two modes, per fixture:
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
    MODE="$(python3 -c "
import json
spec = next((f for f in json.load(open('$THRESHOLDS'))['fixtures'] if f['name'] == '$NAME'), {})
print('lines' if spec.get('text_gate') == 'lines' else 'chars')
")"
    set +e
    python3 - "$dir/ref.html" "$dir/cand.jsonl" "$NAME" "$dir/text-gate.json" "$MODE" <<'PY'
import json, re, sys, html
from collections import Counter
from xml.etree import ElementTree as ET

ref_path, cand_path, name, out_path = sys.argv[1:5]
MARKER = re.compile(r"^(?:[•◦▪‣–—\-*]|\(?[0-9ivxlcdmIVXLCDMa-zA-Z]{1,6}[.)])\s+")

def norm(s):
    s = s.replace(" ", " ").replace("­", "").replace("\t", " ")
    s = re.sub(r"\s+", " ", s).strip()
    s = MARKER.sub("", s)
    return "" if re.fullmatch(r"[•◦▪‣–—*]|\(?[0-9ivxlcdmIVXLCDMa-zA-Z]{1,6}[.)]", s) else s

# Reference: <page><flow><block><line><word xMin yMin xMax yMax>w</word>…
raw = open(ref_path, encoding="utf-8").read()
raw = raw[raw.index("<body"):raw.rindex("</body>") + 7]
body = ET.fromstring(re.sub(r"&(?!amp;|lt;|gt;|quot;|apos;)", "&amp;", raw))
ref = []
pages = [p for p in body.iter() if p.tag.endswith("page")]
for pi, page in enumerate(pages):
    for line in [l for l in page.iter() if l.tag.endswith("line")]:
        words = [w for w in line if w.tag.endswith("word")]
        if not words:
            continue
        text = norm(" ".join(html.unescape(w.text or "") for w in words))
        if not text:
            continue
        ymax = max(float(w.get("yMax")) for w in words)
        ref.append({"page": pi, "text": text, "y": ymax})

cand = []
for l in open(cand_path, encoding="utf-8"):
    r = json.loads(l)
    text = norm(r.get("source_text", ""))
    if text:
        cand.append({"page": r["page_idx"], "text": text, "y": r["baseline_y_pt"]})

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

problems = []
for page in sorted(set(R) | set(C)):
    rr, cc = R.get(page, []), C.get(page, [])
    rc, ccnt = chars(rr), chars(cc)
    problem = {}
    if rc != ccnt:
        problem["indesign_only"] = "".join(sorted((rc - ccnt).elements()))[:120]
        problem["paged_only"] = "".join(sorted((ccnt - rc).elements()))[:120]
    if mode == "lines":
        rl = sorted(unhyphen(r["text"]) for r in rr)
        cl = sorted(unhyphen(c["text"]) for c in cc)
        if rl != cl:
            problem["lines_indesign_only"] = [t for t in rl if t not in cl][:8]
            problem["lines_paged_only"] = [t for t in cl if t not in rl][:8]
    if problem:
        problem["page"] = page + 1
        problems.append(problem)
result = {"fixture": name, "mode": mode, "passed": not problems,
          "pages_compared": len(set(R) | set(C)),
          "lines_indesign": len(ref), "lines_paged": len(cand), "problems": problems}
json.dump(result, open(out_path, "w"), indent=2)
if problems:
    print(f"[{name}] TEXT MISMATCH ({mode}) on {len(problems)} page(s)")
    for p in problems[:6]:
        print(f"  page {p['page']}: " + json.dumps({k: v for k, v in p.items() if k != 'page'}, ensure_ascii=False)[:300])
    sys.exit(1)
print(f"[{name}] text ok ({len(ref)} lines, {result['pages_compared']} pages)")
PY
    rc=$?
    set -e
    [ $rc -ne 0 ] && fail=1
done

if [ "$fail" -ne 0 ] && [ "$GATE" != "advisory" ]; then
    exit 1
fi
exit 0
