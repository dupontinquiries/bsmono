#!/usr/bin/env python3
"""Idle profile of the phone: where the CPU goes, what the clocks are, how hot, how chatty the log.

Run ON the phone (python3 is there):  ssh user@phone python3 - [seconds] < idle-profile.py
Percentages are of ONE core, so 100.0 is a whole core; the total is out of 800 on the FP5.
Measured on 2026-10-01 by hand with this same method: idle total ~12%, all cores on `performance`.
"""
import glob, os, subprocess, sys, time

secs = float(sys.argv[1]) if len(sys.argv) > 1 else 10.0


def snap():
    d = {}
    for p in glob.glob("/proc/[0-9]*"):
        try:
            st = open(p + "/stat").read()
            comm = st[st.index("(") + 1:st.rindex(")")]
            f = st[st.rindex(")") + 2:].split()
            d[p[6:]] = (comm, int(f[11]) + int(f[12]))  # utime + stime, clock ticks
        except Exception:
            pass
    return d


def read(path):
    try:
        return open(path).read().strip()
    except Exception:
        return "?"


a = snap()
t0 = time.time()
time.sleep(secs)
b = snap()
dt = time.time() - t0
hz = os.sysconf("SC_CLK_TCK")
rows = sorted(((b[p][1] - a[p][1]) / hz / dt * 100, b[p][0], p) for p in b if p in a)
print(f"total {sum(r[0] for r in rows):.1f}% of one core over {dt:.0f}s (800% = every core)")
for pct, comm, pid in rows[::-1][:12]:
    if pct >= 0.2:
        print(f"{pct:6.1f}%  {comm} (pid {pid})")
govs = {}
for g in glob.glob("/sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_governor"):
    govs[read(g)] = govs.get(read(g), 0) + 1
print("governors:", govs)
print("cur MHz:", [int(read(f)) // 1000 for f in sorted(glob.glob("/sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_cur_freq")) if read(f).isdigit()])
temps = sorted(((int(read(z + "/temp")), read(z + "/type")) for z in glob.glob("/sys/class/thermal/thermal_zone*") if read(z + "/temp").lstrip("-").isdigit()), reverse=True)
print("hottest:", ", ".join(f"{t/1000:.0f}C {n}" for t, n in temps[:4]))
try:
    n = subprocess.run(["journalctl", "--user", "-b", "--since", "-30sec", "--no-pager"], capture_output=True, text=True).stdout.count("\n")
    print(f"user journal lines in the last 30 s: {n}")
except Exception:
    pass
print("cosmic-panel RssAnon:", [l.split()[1] + " kB" for p in glob.glob("/proc/[0-9]*") if read(p + "/comm") == "cosmic-panel" for l in open(p + "/status") if l.startswith("RssAnon")])
