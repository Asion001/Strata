#!/usr/bin/env python3
"""Check Criterion results (target/criterion) against the targets, and
compare two saved baselines of the same machine.

  bench-check.py targets BASELINE           the targets, on one run
  bench-check.py compare BASE HEAD          fail on a regression over 10%

Both print a Markdown table (for the job summary) and exit 1 on a failure.
"""
import json
import os
import sys
from pathlib import Path

ROOT = Path(os.environ.get("CRITERION_DIR", "target/criterion"))
# A regression past this share of the base median fails the comparison.
THRESHOLD = 0.10

# id: (what, limit, kind). "p95_ms": the 95th percentile of per-iteration
# time across Criterion's samples, under `limit` ms. "gbps": bytes per second
# at the median, at least `limit` GB/s (10^9).
TARGETS = {
    "db/list_10k_entries": ("listing 10,000 entries, server time p95", 100, "p95_ms"),
    "db/move_10k_node_subtree": ("moving a 10,000-node subtree, p95", 200, "p95_ms"),
    "blake3/1MiB": ("BLAKE3, one core, warm (1 MiB)", 1.0, "gbps"),
    "blake3/64MiB": ("BLAKE3, one core, cold (64 MiB)", 1.0, "gbps"),
}


def bench_ids():
    """Every bench id (a path under ROOT with a benchmark.json somewhere)."""
    ids = set()
    for f in ROOT.glob("**/benchmark.json"):
        # <id...>/<baseline>/benchmark.json
        ids.add(str(f.parent.parent.relative_to(ROOT)))
    return sorted(ids)


def load(bench, baseline, name):
    p = ROOT / bench / baseline / name
    return json.loads(p.read_text()) if p.exists() else None


def median_ns(bench, baseline):
    e = load(bench, baseline, "estimates.json")
    return e["median"]["point_estimate"] if e else None


def p95_ns(bench, baseline):
    s = load(bench, baseline, "sample.json")
    if not s:
        return None
    per = sorted(t / n for t, n in zip(s["times"], s["iters"]))
    return per[min(len(per) - 1, int(round(0.95 * (len(per) - 1))))]


def throughput_bytes(bench, baseline):
    b = load(bench, baseline, "benchmark.json")
    t = (b or {}).get("throughput") or {}
    return t.get("Bytes")


def fmt_ns(ns):
    for unit, d in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
        if ns >= d:
            return f"{ns / d:.3g} {unit}"
    return f"{ns:.3g} ns"


def targets(baseline):
    rows, failed = [], []
    for bench, (what, limit, kind) in TARGETS.items():
        if kind == "p95_ms":
            v = p95_ns(bench, baseline)
            ok = v is not None and v / 1e6 < limit
            shown = "missing" if v is None else fmt_ns(v)
            want = f"< {limit} ms"
        else:
            m, n = median_ns(bench, baseline), throughput_bytes(bench, baseline)
            v = None if not m or not n else n / (m / 1e9) / 1e9
            ok = v is not None and v >= limit
            shown = "missing" if v is None else f"{v:.2f} GB/s"
            want = f"≥ {limit} GB/s"
        rows.append(f"| {what} | `{bench}` | {shown} | {want} | {'✅' if ok else '❌'} |")
        if not ok:
            failed.append(bench)
    print("### Performance targets\n")
    print("| What | Bench | Result | Target | |\n|---|---|---|---|---|")
    print("\n".join(rows))
    if failed:
        print(f"\n**Missed:** {', '.join(failed)}")
    return not failed


def compare(base, head):
    rows, failed, new = [], [], []
    for bench in bench_ids():
        b, h = median_ns(bench, base), median_ns(bench, head)
        if h is None:
            continue
        if b is None:
            new.append(bench)
            continue
        delta = (h - b) / b
        bad = delta > THRESHOLD
        mark = "❌" if bad else ("🟢" if delta < -THRESHOLD else "")
        rows.append(f"| `{bench}` | {fmt_ns(b)} | {fmt_ns(h)} | {delta:+.1%} | {mark} |")
        if bad:
            failed.append(bench)
    print(f"### Benchmarks: this change against its base (same runner, median)\n")
    if rows:
        print("| Bench | Base | Head | Change | |\n|---|---|---|---|---|")
        print("\n".join(rows))
    else:
        print("No bench ran on both (the base has no benches yet).")
    if new:
        print(f"\nNew (no base to compare): {', '.join(f'`{n}`' for n in new)}")
    if failed:
        print(f"\n**Regressed over {THRESHOLD:.0%}:** {', '.join(failed)}")
    return not failed


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "targets":
        sys.exit(0 if targets(sys.argv[2]) else 1)
    if len(sys.argv) == 4 and sys.argv[1] == "compare":
        sys.exit(0 if compare(sys.argv[2], sys.argv[3]) else 1)
    print(__doc__, file=sys.stderr)
    sys.exit(2)
