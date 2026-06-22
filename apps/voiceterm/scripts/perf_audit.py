#!/usr/bin/env python3
"""v0.1.749: performance audit harness. Static analysis of the QML for the
idle/in-use power-draw suspects the user flagged (device heating with the app
open): always-on timers, their cadence, whether they're gated by visibility/
focus, and hot-signal handler fan-out. Produces a "performance matrix" (feature
area → wakeup cost) and a prioritized list of offenders, plus machine-readable
counts that perf_regression.py locks in.

Pure static scan — no run needed. Re-run after changes to see the matrix move.
"""
import re, sys, json
from collections import defaultdict

QML = sys.argv[1] if len(sys.argv) > 1 else "qml/main.qml"

def load(path):
    return open(path, encoding="utf-8", errors="replace").read().splitlines()

lines = load(QML)
text = "\n".join(lines)

# ── Timer audit ──────────────────────────────────────────────────────────
# Find each `Timer {` and capture a window of its properties.
timers = []
for i, ln in enumerate(lines):
    if re.search(r'\bTimer\s*\{', ln):
        # gather the block (brace-balanced, capped at 40 lines)
        depth = ln.count("{") - ln.count("}")
        block = [ln]
        j = i + 1
        while j < len(lines) and depth > 0 and (j - i) < 40:
            block.append(lines[j])
            depth += lines[j].count("{") - lines[j].count("}")
            j += 1
        b = "\n".join(block)
        interval = re.search(r'interval:\s*(\d+)', b)
        interval = int(interval.group(1)) if interval else None
        repeat = bool(re.search(r'repeat:\s*true', b))
        running = re.search(r'running:\s*(.+)', b)
        running_expr = running.group(1).strip() if running else None
        # heuristics for gating
        # A gate that only pauses during a transient popup/capture window
        # (e.g. `!voicePopup.visible`) still runs during NORMAL idle use — it's
        # an idle drain, not truly gated. Only real visibility/focus of the
        # owning view counts as a gate.
        gated_visible = bool(running_expr and re.search(
            r'\bvisible\b|\bactive\b|Window\.active|expanded|hovered', running_expr)
            and not re.search(r'!\s*\w+\.(visible|open)', running_expr or ""))
        idn = re.search(r'id:\s*(\w+)', b)
        tid = idn.group(1) if idn else None
        # A timer is a TRUE idle drain only if it can't be stopped by app logic
        # (no `<id>.stop()` anywhere) AND is either default-on/explicitly-on or
        # gated only on backend-existence. Start/stop-managed timers (key
        # repeat, async polls that stop on completion) are transient, not idle
        # drains — exclude them to avoid false positives.
        has_stop = bool(tid and re.search(rf'\b{re.escape(tid)}\.stop\s*\(', text))
        # bare stop() inside the timer's own onTriggered also makes it transient
        has_stop = has_stop or bool(re.search(r'(?<![.\w])stop\s*\(\s*\)', b))
        # started-on-demand (only .start()ed, never running:true) is also not an
        # idle drain unless it never stops; combined with has_stop above.
        started_on_demand = bool(tid and re.search(rf'\b{re.escape(tid)}\.start\s*\(', text)
                                 and not (running_expr in ("true",) or running_expr is None))
        always_on = (repeat
                     and not has_stop
                     and not started_on_demand
                     and (running_expr is None
                          or running_expr == "true"
                          or (not gated_visible and "backend" in (running_expr or "")
                              and "!" not in (running_expr or ""))))
        timers.append({
            "line": i + 1,
            "id": tid or "?",
            "interval": interval,
            "repeat": repeat,
            "running": running_expr,
            "gated": gated_visible,
            "transient": has_stop,
            "always_on": always_on,
        })

# wakeups/sec for an always-on repeating timer
def hz(t):
    if t["interval"] and t["repeat"]:
        return round(1000.0 / t["interval"], 2)
    return 0.0

always = [t for t in timers if t["always_on"] and t["interval"]]
always.sort(key=lambda t: -hz(t))
idle_wakeups = sum(hz(t) for t in always)

# ── Hot-signal fan-out ───────────────────────────────────────────────────
# Count Connections handlers per backend signal (hot ones cascade work).
sig_handlers = defaultdict(int)
for m in re.finditer(r'function\s+on([A-Z]\w+)\s*\(', text):
    name = m.group(1)
    # snake the QML on<Signal> back to signal name
    snake = re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()
    sig_handlers[snake] += 1
hot = sorted(sig_handlers.items(), key=lambda kv: -kv[1])[:8]

# ── Report ─────────────────────────────────────────────────────────────────
print("=" * 74)
print("PERFORMANCE MATRIX — idle/always-on timer cost")
print("=" * 74)
print(f"  total Timer blocks: {len(timers)}")
print(f"  always-on repeating timers: {len(always)}")
print(f"  estimated IDLE wakeups/sec (sum of always-on Hz): {round(idle_wakeups,1)}")
print()
print("  worst always-on timers (Hz | id | interval | running-gate):")
for t in always[:12]:
    print(f"    {hz(t):>6}Hz | {t['id'][:22]:22} | {str(t['interval'])+'ms':>7} | {t['running'] or '(default on)'}")

print()
print("  hot signal handler fan-out (signal → #handlers):")
for sig, n in hot:
    flag = "  ← cascades" if n >= 6 else ""
    print(f"    {n:>3}  on_{sig}{flag}")

# ── Lessons ──────────────────────────────────────────────────────────────
print()
print("=" * 74)
print("FINDINGS")
print("=" * 74)
findings = []
fast_idle = [t for t in always if t["interval"] and t["interval"] <= 300]
if fast_idle:
    ids = ", ".join(f"{t['id']}@{t['interval']}ms" for t in fast_idle[:6])
    findings.append(("HIGH", f"{len(fast_idle)} always-on timers tick <=300ms while idle "
        f"({ids}). Each wakes the CPU ~{round(1000/min(t['interval'] for t in fast_idle))}+x/s. "
        f"These run even when nothing changes — the heat source when the app sits open. "
        f"Fix: event-drive them, or back off the interval when idle/unfocused (adaptive poll), "
        f"or gate on Window.active for the non-urgent ones."))
ungated = [t for t in always if not t["gated"]]
if ungated:
    findings.append(("MED", f"{len(ungated)} always-on timers have no visibility/active gate — "
        f"they keep ticking when the window is backgrounded. A Window.active (or app-focus) "
        f"gate stops the drain when the user isn't looking, with zero in-use cost."))
big_fan = [(s, n) for s, n in hot if n >= 8]
if big_fan:
    s, n = big_fan[0]
    findings.append(("MED", f"on_{s} has {n} handlers — every fire cascades {n} JS callbacks. If "
        f"this signal is hot (e.g. term_output_changed on each PTY chunk), debounce/coalesce at "
        f"the source so a burst of output triggers one repaint, not {n}xN."))
findings.append(("INFO", "General 'better even in use' levers: coalesce per-chunk terminal work "
    "into one rAF-style repaint; batch voice/audio polling behind a VAD energy gate so the "
    "transcription chain only wakes on actual speech, not every tick; widen idle poll intervals "
    "adaptively (fast when recently active, slow after N seconds quiet)."))

order = {"HIGH": 0, "MED": 1, "INFO": 2}
findings.sort(key=lambda x: order[x[0]])
for i, (sev, t) in enumerate(findings, 1):
    print(f"\n{i}. [{sev}] {t}")

# ── machine-readable for regression test ───────────────────────────────────
metrics = {
    "total_timers": len(timers),
    "always_on_timers": len(always),
    "idle_wakeups_per_sec": round(idle_wakeups, 1),
    "fast_idle_timers": len(fast_idle),
    "ungated_always_on": len(ungated),
    "max_signal_fanout": hot[0][1] if hot else 0,
}
out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/perf_metrics.json"
json.dump(metrics, open(out, "w"), indent=2)
print(f"\n  metrics → {out}")
print(f"  {json.dumps(metrics)}")
