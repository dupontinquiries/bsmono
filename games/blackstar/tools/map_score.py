#!/usr/bin/env python3
"""map_score.py -- a headless map-rating harness (inspired by games/soldier's mapeval).

Scores a map's collider layout (.col: cx cy cz hx hy hz per line) on design metrics so
we can iterate the generators by NUMBER instead of eyeballing screenshots:

  cover     -- how much shootable cover exists, and is it balanced low vs high
  spread    -- are cover pieces distributed (varied engagement ranges) or clumped
  vertical  -- verticality: height variation of structures + terrain (ramps/bumps)
  openness  -- how much of the arena is open floor vs blocked
  scale     -- is the playable area a sensible size

Each sub-score is 0..1; total is 0..100 with a letter grade. Run on one map or all:
  python3 tools/map_score.py engine_src/data/wastes.col
  python3 tools/map_score.py --all
"""
import sys, os, glob, math

def load_col(path):
    boxes = []
    for line in open(path):
        p = line.split()
        if len(p) >= 6:
            boxes.append(tuple(float(x) for x in p[:6]))
    return boxes

def score(boxes):
    if not boxes:
        return {"total": 0.0, "grade": "F", "cover": 0, "spread": 0, "vertical": 0, "openness": 0, "scale": 0, "n": 0}
    xs = [b[0] for b in boxes]; zs = [b[2] for b in boxes]
    ext_x = max(xs) - min(xs) + 1e-3; ext_z = max(zs) - min(zs) + 1e-3
    area = ext_x * ext_z
    n = len(boxes)

    # cover: count + low/high balance (eye height ~1.5). low cover (top<1.5) you shoot
    # over; high cover (reaches eye) breaks sightlines. want a mix.
    low = sum(1 for b in boxes if b[1] + b[4] < 1.5)
    high = n - low
    dens = min(n / (area / 900.0 + 1e-3), 1.5) / 1.5          # ~1 cover per 30x30 = full
    bal = 1.0 - abs((high / n) - 0.55) / 0.55                 # ideal ~55% sightline-breaking
    cover = max(0.0, min(1.0, 0.55 * dens + 0.45 * max(0.0, bal)))

    # spread: coefficient of variation of nearest-neighbour distances -- clumped maps
    # score low, evenly-varied maps score high.
    nn = []
    for i, a in enumerate(boxes):
        best = 1e9
        for j, b in enumerate(boxes):
            if i == j: continue
            d = math.hypot(a[0]-b[0], a[2]-b[2])
            if d < best: best = d
        if best < 1e8: nn.append(best)
    if nn:
        mean = sum(nn)/len(nn)
        var = sum((d-mean)**2 for d in nn)/len(nn)
        cv = (math.sqrt(var)/mean) if mean > 0 else 0
        spread = max(0.0, min(1.0, cv * 1.6))                 # some variance is good
    else:
        spread = 0.0

    # verticality: spread of collider TOP heights (structures + ramps + terrain bumps).
    tops = [b[1] + b[4] for b in boxes]
    tmean = sum(tops)/len(tops)
    tvar = sum((t-tmean)**2 for t in tops)/len(tops)
    vertical = max(0.0, min(1.0, math.sqrt(tvar) / 6.0))     # ~6u height spread = full

    # openness: fraction of the arena footprint NOT covered by collider top-down area.
    covered = sum(4*b[3]*b[5] for b in boxes)
    openness = max(0.0, min(1.0, 1.0 - covered / area))
    openness = 1.0 - abs(openness - 0.82) / 0.82             # want ~82% open, not empty/maze

    # scale: playable extent (want ~120..320 across)
    span = max(ext_x, ext_z)
    scale = 1.0 - abs(span - 200.0) / 200.0
    scale = max(0.0, min(1.0, scale))

    sub = {"cover": cover, "spread": spread, "vertical": vertical,
           "openness": max(0.0, openness), "scale": scale}
    total = (sub["cover"]*26 + sub["spread"]*18 + sub["vertical"]*22 +
             sub["openness"]*18 + sub["scale"]*16)
    grade = "S" if total>=90 else "A" if total>=80 else "B" if total>=70 else \
            "C" if total>=60 else "D" if total>=50 else "F"
    return {"total": total, "grade": grade, "n": n, **sub}

def report(path):
    s = score(load_col(path))
    name = os.path.basename(path).replace(".col","")
    return "%-12s %5.1f  %s   cover %.2f  spread %.2f  vert %.2f  open %.2f  scale %.2f  (%d boxes)" % (
        name, s["total"], s["grade"], s["cover"], s["spread"], s["vertical"], s["openness"], s["scale"], s["n"])

def main():
    args = sys.argv[1:]
    if "--check" in args:
        # gate: the terrain maps must stay well-formed + above a quality floor
        want = ["wastes","frostline","outer_rim","reclaim","trench","mars","beach","mining"]
        bad = []
        for m in want:
            path = "engine_src/data/%s.col" % m
            if not os.path.exists(path): bad.append((m,"missing")); continue
            s = score(load_col(path))
            if s["n"] == 0 or s["total"] < 58.0: bad.append((m, "%.1f/%s"%(s["total"],s["grade"])))
        if bad:
            print("map_score CHECK FAILED:", bad); return 1
        print("map_score: all %d terrain maps above quality floor" % len(want)); return 0
    if "--all" in args:
        cols = sorted(glob.glob("engine_src/data/*.col"))
        print("map            score  grade   cover     spread    vert      open      scale")
        tot = 0.0
        for c in cols:
            print(report(c)); tot += score(load_col(c))["total"]
        print("-- mean score: %.1f over %d maps --" % (tot/max(1,len(cols)), len(cols)))
    elif args:
        print(report(args[0]))
    else:
        print(__doc__)

if __name__ == "__main__":
    main()
