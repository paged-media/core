#!/usr/bin/env python3
"""Aggregate a Chrome trace's V8 CPU samples by function.

    python3 scripts/perf/aggregate-trace.py trace.json [TOP]

Reads the `Profile` / `ProfileChunk` events a trace recorded with the
`disabled-by-default-v8.cpu_profiler` category (worker threads included),
drops idle samples, strips Rust symbol hashes, and prints the busiest
functions by SELF samples. With a named wasm (scripts/perf/profile-wasm.sh)
the engine's functions appear by name.
"""
import collections
import json
import re
import sys


def main() -> None:
    path = sys.argv[1]
    top = int(sys.argv[2]) if len(sys.argv) > 2 else 30
    trace = json.load(open(path))
    events = trace["traceEvents"] if isinstance(trace, dict) else trace
    nodes, samples = {}, []
    for e in events:
        if e.get("name") not in ("Profile", "ProfileChunk"):
            continue
        profile = e.get("args", {}).get("data", {}).get("cpuProfile", {})
        for n in profile.get("nodes", []) or []:
            nodes[(e["id"], n["id"])] = n
        samples += [(e["id"], s) for s in profile.get("samples", []) or []]
    counts = collections.Counter()
    for pid, s in samples:
        node = nodes.get((pid, s))
        if not node:
            continue
        name = node["callFrame"]["functionName"] or "(anonymous)"
        if name in ("(idle)", "(program)", "(garbage collector)"):
            continue
        counts[re.sub(r"::h[0-9a-f]{16}$", "", name)[:120]] += 1
    busy = sum(counts.values())
    print(f"busy samples: {busy}")
    for name, n in counts.most_common(top):
        print(f"{n:7d} {100 * n / busy:5.1f}%  {name}")


if __name__ == "__main__":
    main()
