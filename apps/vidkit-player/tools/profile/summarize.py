#!/usr/bin/env python3
"""Summarise vidkit-player profile TSVs (VIDKIT_PROFILE) into markdown.

    summarize.py OUT_DIR        # every *.tsv in it, one row per file

The first 10 seconds of each run are skipped (startup, first-frame, cache
warm-up). Medians, so a hiccup does not move the result. CPU is percent of
one core; "cpu/frame" is total app+children CPU per frame SHOWN -- the
comparable figure when a run is CPU-bound and drops frames.
"""
import csv, glob, os, statistics, sys
from collections import Counter

SKIP_S = 10.0
GROUPS = ["ui", "video", "audio", "gl", "transcribe", "other", "children"]


def load(path):
    with open(path) as f:
        rows = [r for r in csv.DictReader(f, delimiter="\t") if r.get("t_s") and r["t_s"] != "t_s"]
    return [r for r in rows if float(r["t_s"]) >= SKIP_S]


def num(r, k):
    try:
        return float(r[k])
    except (KeyError, ValueError):
        return None


def med(rows, k):
    v = [x for x in (num(r, k) for r in rows) if x is not None]
    return statistics.median(v) if v else None


def fmt(x, d=1):
    return "-" if x is None else f"{x:.{d}f}"


def summarise(path):
    rows = load(path)
    if not rows:
        return None
    s = {"name": os.path.splitext(os.path.basename(path))[0], "seconds": len(rows)}
    for k in ["shown", "dropped", "repaints", "upload_kb", "damaged_pct", "conv_kpx", "rss_mb"]:
        s[k] = med(rows, k)
    for g in GROUPS:
        s["cpu_" + g] = med(rows, "cpu_" + g)
    total = [sum(num(r, "cpu_" + g) or 0 for g in GROUPS) for r in rows]
    s["cpu_total"] = statistics.median(total)
    per = [t / num(r, "shown") for t, r in zip(total, rows) if (num(r, "shown") or 0) > 0]
    s["cpu_per_frame"] = statistics.median(per) if per else None
    lods = Counter(r.get("lod_max", "-") for r in rows)
    s["lods"] = " ".join(f"L{k}:{v}" for k, v in sorted(lods.items()))
    s["presenter"] = Counter(r.get("presenter", "") for r in rows).most_common(1)[0][0]
    s["decoder"] = Counter(r.get("decoder", "") for r in rows).most_common(1)[0][0]
    # Presentation feedback (Wayland): the LAST row -- the log is cumulative
    # over a rolling window, and the latency compensation has settled by then.
    s["display"] = rows[-1].get("display") or "-"
    return s


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    runs = [s for s in (summarise(p) for p in sorted(glob.glob(os.path.join(out, "*.tsv")))) if s]
    if not runs:
        print("no profile data in", out)
        return 1
    title = open(os.path.join(out, "title.txt")).read().strip() if os.path.exists(os.path.join(out, "title.txt")) else ""
    print(f"# vidkit-player profile{': ' + title if title else ''}\n")
    print("| run | s | shown/s | dropped/s | cpu total % | cpu/frame % | ui | video | gl | children | upload KB/s | damaged % | conv kpx/s | LOD (s at) | rss MB |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for s in runs:
        print("| " + " | ".join([
            s["name"], str(s["seconds"]), fmt(s["shown"]), fmt(s["dropped"]), fmt(s["cpu_total"]), fmt(s["cpu_per_frame"], 2),
            fmt(s["cpu_ui"]), fmt(s["cpu_video"]), fmt(s["cpu_gl"]), fmt(s["cpu_children"]),
            fmt(s["upload_kb"], 0), fmt(s["damaged_pct"]), fmt(s["conv_kpx"], 0), s["lods"], fmt(s["rss_mb"], 0),
        ]) + " |")
    print()
    for s in runs:
        print(f"- **{s['name']}**: presenter `{s['presenter']}`, decoder `{s['decoder']}`, {s['display']}")
    base = next((s for s in runs if s["name"] == "cpu-whole"), None)
    if base and base["cpu_per_frame"]:
        print()
        for s in runs:
            if s is not base and s["cpu_per_frame"]:
                print(f"- {s['name']}: {base['cpu_per_frame'] / s['cpu_per_frame']:.2f}x less CPU per frame than cpu-whole")
    return 0


if __name__ == "__main__":
    sys.exit(main())
