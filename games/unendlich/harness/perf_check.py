#!/usr/bin/env python3
"""perf_check.py — perf-budget regression watch (R17), integration tier.

Parses an engine run log (stdout or run_summaries.txt) for the GPU 'total' timer
line and the tick_apply line, and compares to a baseline JSON. Flags regressions:
total p50 up >20%, the over-budget fraction up >1.0 pp, or tick_apply p50 up >20%.
First run with no baseline writes the current numbers as the baseline.
Usage: perf_check.py <run_log> [baseline.json]
"""
import json, os, re, sys
def parse(log):
    out = {}
    m = re.search(r'^\s*total\s.*?p50=\s*([\d.]+).*?p99=\s*([\d.]+).*?over_[\d.]+ms=\d+\s*\(([\d.]+)%\)', log, re.M)
    if m: out.update(total_p50=float(m[1]), total_p99=float(m[2]), total_over_pct=float(m[3]))
    t = re.search(r'tick_apply:.*?p50_ms=([\d.]+)', log)
    if t: out["tick_apply_p50"] = float(t[1])
    return out
def main(argv):
    if len(argv) < 2:
        print("usage: perf_check.py <run_log> [baseline.json]"); return 2
    cur = parse(open(argv[1]).read())
    if not cur: print("no timer summary found in log"); return 2
    base_path = argv[2] if len(argv) > 2 else "harness/perf_baseline.json"
    if not os.path.exists(base_path):
        json.dump(cur, open(base_path, "w"), indent=2)
        print(f"baseline written -> {base_path}\n  {cur}"); return 0
    base = json.load(open(base_path))
    regressed = 0
    def chk(k, label, frac=0.20, pp=None):
        nonlocal regressed
        if k not in cur or k not in base: return
        c, b = cur[k], base[k]
        worse = (c > b + pp) if pp is not None else (c > b * (1 + frac))
        flag = "  <- REGRESSED" if worse else ""
        if worse: regressed += 1
        print(f"  {label:16} base={b:7.2f}  now={c:7.2f}{flag}")
    chk("total_p50", "GPU total p50")
    chk("total_p99", "GPU total p99")
    chk("total_over_pct", "over-budget %", pp=1.0)
    chk("tick_apply_p50", "tick_apply p50")
    print("PERF OK" if not regressed else "PERF REGRESSED")
    return 1 if regressed else 0
if __name__ == "__main__": sys.exit(main(sys.argv))
