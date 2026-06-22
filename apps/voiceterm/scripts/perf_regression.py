#!/usr/bin/env python3
"""v0.1.749: performance regression test. Runs perf_audit.py and fails (exit 1)
if any idle/in-use cost metric exceeds its locked threshold. Catches a future
change that adds an always-on fast timer or fans a hot signal out further —
the "feature mix clobbering a fine app" the user wants guarded against.

Thresholds are deliberately a little above the current measured values so
normal work doesn't trip them, but a real regression (new always-on poll,
doubled fan-out) does. Bump a threshold ONLY with a documented reason.
"""
import json, subprocess, sys, os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
QML = os.path.join(ROOT, "qml", "main.qml")
OUT = "/tmp/perf_metrics.json"

# (metric, max_allowed, why)
THRESHOLDS = [
    ("always_on_timers", 2,
     "genuinely always-on repeating timers (idle CPU wakeups). Currently 1 (_pagerPoll)."),
    ("idle_wakeups_per_sec", 6.0,
     "sum of always-on timer Hz. Currently 2.5. A new 4Hz idle poll would trip this."),
    ("fast_idle_timers", 1,
     "always-on timers <=300ms. Currently 0 after gating the wake polls."),
    ("max_signal_fanout", 9,
     "handlers on the hottest signal (term_output_changed=7). Adding 2+ more cascades trips it."),
]

def main():
    subprocess.run([sys.executable, os.path.join(ROOT, "scripts", "perf_audit.py"), QML, OUT],
                   stdout=subprocess.DEVNULL, check=True)
    m = json.load(open(OUT))
    failed = []
    print("perf_regression: checking idle/in-use cost thresholds")
    for metric, limit, why in THRESHOLDS:
        val = m.get(metric, 0)
        ok = val <= limit
        print(f"  [{'OK ' if ok else 'FAIL'}] {metric} = {val} (limit {limit}) — {why}")
        if not ok:
            failed.append((metric, val, limit))
    if failed:
        print("\nPERF REGRESSION: " + ", ".join(f"{k}={v}>{l}" for k, v, l in failed))
        print("A change added idle/in-use cost. Event-drive or gate the new work, "
              "or justify + bump the threshold here.")
        sys.exit(1)
    print("\nperf_regression: all thresholds OK")

if __name__ == "__main__":
    main()
