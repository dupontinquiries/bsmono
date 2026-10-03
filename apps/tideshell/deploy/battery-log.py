#!/usr/bin/env python3
"""Battery telemetry for the phone: one JSON line a minute, enough to answer "where did the charge go".

Runs ON the phone as a user service (`fp5_setup::battery_log_plan`), writes
`~/.local/share/tideshell/battery.jsonl`, and is read by `battery-report.py` on the laptop.

Each line: time, battery (capacity, status, current, voltage, temperature), whether the screen is lit
(backlight level, and the panel's own state), how many times the system has SUSPENDED since boot
(`/sys/power/suspend_stats`), the CPU governor and mean frequency, the busiest processes over the last
interval (CPU seconds), and the busiest interrupts (counts) -- the three things that keep a phone
awake. Cheap by construction: a handful of small sysfs reads and one /proc walk per minute.

Kept deliberately free of anything that wakes hardware (no camera, no sensors, no D-Bus).
"""
import glob
import json
import os
import sys
import time

OUT = os.path.expanduser("~/.local/share/tideshell/battery.jsonl")
INTERVAL_S = 60
MAX_BYTES = 8 * 1024 * 1024  # ~ 5 months of minutes; trimmed to half when exceeded
HZ = os.sysconf("SC_CLK_TCK")


def read(path, default=""):
    try:
        with open(path) as f:
            return f.read().strip()
    except Exception:
        return default


def num(path):
    v = read(path)
    try:
        return int(v)
    except ValueError:
        return None


def battery():
    base = next((d for d in glob.glob("/sys/class/power_supply/*") if read(d + "/type") == "Battery"), None)
    if not base:
        return {}
    t = num(base + "/temp")
    return {
        "pct": num(base + "/capacity"),
        "status": read(base + "/status"),
        "ua": num(base + "/current_now"),  # negative = discharging on this driver
        "uv": num(base + "/voltage_now"),
        "temp_c": None if t is None else t / 10.0,
    }


def screen():
    bl = next(iter(glob.glob("/sys/class/backlight/*")), None)
    level = num(bl + "/brightness") if bl else None
    return {"bl": level, "bl_max": num(bl + "/max_brightness") if bl else None}


def cpu():
    govs = {read(g) for g in glob.glob("/sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_governor")}
    freqs = [num(f) for f in glob.glob("/sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_cur_freq")]
    freqs = [f for f in freqs if f]
    return {"gov": "/".join(sorted(g for g in govs if g)), "mhz": round(sum(freqs) / len(freqs) / 1000) if freqs else None}


def proc_ticks():
    """pid -> (comm, utime + stime ticks)."""
    out = {}
    for p in glob.glob("/proc/[0-9]*"):
        try:
            st = open(p + "/stat").read()
            comm = st[st.index("(") + 1:st.rindex(")")]
            f = st[st.rindex(")") + 2:].split()
            out[int(p[6:])] = (comm, int(f[11]) + int(f[12]))
        except Exception:
            pass
    return out


def irq_counts():
    """irq label -> total count across CPUs."""
    out = {}
    for line in read("/proc/interrupts").splitlines()[1:]:
        parts = line.split()
        if len(parts) < 3 or not parts[0].endswith(":"):
            continue
        counts = []
        for tok in parts[1:]:
            if tok.isdigit():
                counts.append(int(tok))
            else:
                break
        label = " ".join(parts[1 + len(counts):])[:40] or parts[0]
        out[parts[0] + label] = sum(counts)
    return out


def top(deltas, n=4, scale=1.0):
    items = sorted(((v * scale, k) for k, v in deltas.items() if v > 0), reverse=True)[:n]
    return [[round(v, 2), k] for v, k in items]


def sample(prev, now):
    cur_p, cur_i = proc_ticks(), irq_counts()
    rec = {"t": int(now), **battery(), **screen(), **cpu()}
    rec["suspends"] = num("/sys/power/suspend_stats/success")
    rec["suspend_fail"] = num("/sys/power/suspend_stats/fail")
    rec["load"] = read("/proc/loadavg").split()[0:1]
    if prev:
        pp, pi, pt = prev
        dt = max(now - pt, 1)
        by_comm = {}
        for pid, (comm, ticks) in cur_p.items():
            if pid in pp:
                by_comm[comm] = by_comm.get(comm, 0) + max(ticks - pp[pid][1], 0)
        rec["dt"] = round(dt, 1)
        rec["cpu_s"] = round(sum(by_comm.values()) / HZ, 2)
        rec["top_cpu"] = top(by_comm, 4, 1.0 / HZ)
        rec["top_irq"] = top({k: cur_i[k] - pi.get(k, 0) for k in cur_i}, 4)
    return rec, (cur_p, cur_i, now)


def trim(path):
    try:
        if os.path.getsize(path) > MAX_BYTES:
            with open(path, "rb") as f:
                data = f.read()
            data = data[len(data) // 2:]
            data = data[data.index(b"\n") + 1:]
            with open(path, "wb") as f:
                f.write(data)
    except Exception:
        pass


def main():
    once = "--once" in sys.argv
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    prev = None
    while True:
        rec, prev = sample(prev, time.time())
        line = json.dumps(rec, separators=(",", ":"))
        if once:
            print(line)
            return
        with open(OUT, "a") as f:
            f.write(line + "\n")
        trim(OUT)
        time.sleep(INTERVAL_S)


if __name__ == "__main__":
    main()
