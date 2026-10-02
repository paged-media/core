#!/usr/bin/env python3
"""Summarise a criterion run and flag regressions against a previous one.

    python3 scripts/perf/criterion-summary.py OUT.json [PREVIOUS.json]

Reads every `target/criterion/<group>/<bench>/new/estimates.json`, writes
one record per bench to OUT.json ({bench, median_ns, lower_ns, upper_ns,
commit}), and prints a Markdown table. With PREVIOUS.json, a bench whose
median is more than 20 % slower AND whose confidence interval clears the
previous one is flagged. Flags are a signal, not a gate: exit status is
always 0 until the runner's noise is known.
"""
import glob
import json
import os
import subprocess
import sys

THRESHOLD = 1.20


def fmt(ns: float) -> str:
    return f"{ns / 1e9:.2f} s" if ns >= 1e9 else f"{ns / 1e6:.1f} ms"


def main() -> None:
    out = sys.argv[1]
    previous = {}
    if len(sys.argv) > 2 and os.path.exists(sys.argv[2]):
        previous = {r["bench"]: r for r in json.load(open(sys.argv[2]))}
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    rows = []
    for path in sorted(glob.glob("target/criterion/*/*/new/estimates.json")):
        bench = "/".join(path.split("/")[2:4])
        median = json.load(open(path))["median"]
        ci = median["confidence_interval"]
        rows.append({
            "bench": bench,
            "median_ns": median["point_estimate"],
            "lower_ns": ci["lower_bound"],
            "upper_ns": ci["upper_bound"],
            "commit": commit,
        })
    json.dump(rows, open(out, "w"), indent=1)
    print(f"| bench | median | 95% CI | vs previous |\n|---|---|---|---|")
    flagged = 0
    for r in rows:
        p = previous.get(r["bench"])
        change = ""
        if p:
            ratio = r["median_ns"] / p["median_ns"]
            change = f"{(ratio - 1) * 100:+.0f}%"
            if ratio > THRESHOLD and r["lower_ns"] > p["upper_ns"]:
                change += " **REGRESSION?**"
                flagged += 1
        print(f"| {r['bench']} | {fmt(r['median_ns'])} | {fmt(r['lower_ns'])}–{fmt(r['upper_ns'])} | {change} |")
    if flagged:
        print(f"\n{flagged} bench(es) more than {int((THRESHOLD - 1) * 100)}% slower than the previous run.")


if __name__ == "__main__":
    main()
