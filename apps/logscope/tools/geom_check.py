#!/usr/bin/env python3
"""geom_check.py — geometric self-check harness for graph<->scene fit.

Reads the JSON written by scene-view with LOGSCOPE_GEOM_DUMP=1
(/tmp/scene_geom_<theme>.json): scene object AABBs + graph node positions +
scene bounds. Runs bounding-box overlap + range tests so graph<->scene matching
is measurable (into-the-ground, into-the-ceiling, object clipping, out-of-bounds,
spine/axis alignment) instead of eyeballed.

Usage:
  python3 tools/geom_check.py /tmp/scene_geom_Atlantis.json
  python3 tools/geom_check.py --all          # every /tmp/scene_geom_*.json
  python3 tools/geom_check.py <f> --json      # machine-readable
"""
import sys, json, glob, math

NODE_R = 0.5          # half-extent of a graph-node body for overlap tests
CEIL_FRAC = 0.55      # nodes shouldn't sit above floor + CEIL_FRAC*room_height


def aabb_overlap(alo, ahi, blo, bhi):
    return all(alo[k] <= bhi[k] and ahi[k] >= blo[k] for k in range(3))


def hext(o):  # max horizontal extent of an object
    return max(o["hi"][0] - o["lo"][0], o["hi"][2] - o["lo"][2])


def vext(o):
    return o["hi"][1] - o["lo"][1]


def analyze(d):
    objs, graph = d["objects"], d["graph"]
    blo, bhi = d["bounds"]["lo"], d["bounds"]["hi"]
    room_h = bhi[1] - blo[1]
    room_ext = max(bhi[0] - blo[0], bhi[2] - blo[2])

    # floor_y: the WALKABLE floor = the level the MOST solid objects rest on (mode of obstacle base-y, binned).
    # Robust to a deep sea-bed, a tall dome, or a scatter of raised props that would skew min/median.
    cand = [o for o in objs if hext(o) < room_ext * 0.5 and vext(o) > 1.5]
    if cand:
        from collections import Counter
        binned = Counter(round(o["lo"][1] * 2) / 2 for o in cand)  # 0.5u bins
        floor_y = binned.most_common(1)[0][0]
    else:
        wide = [o for o in objs if hext(o) > room_ext * 0.4 and vext(o) < 0.6]
        floor_y = min((o["lo"][1] for o in wide), default=blo[1])
    room_h = min(bhi[1] - floor_y, room_ext * 1.2)

    # OBSTACLES: solid mid-size objects the strand should weave AROUND (columns, shelves,
    # centerpieces) — exclude room-spanning shells (floor/walls/dome) and floor-flat inlay.
    obstacles = [o for o in objs
                 if hext(o) < room_ext * 0.5 and vext(o) > 1.5
                 and min(o["hi"][0] - o["lo"][0], o["hi"][2] - o["lo"][2]) > 0.4
                 and o["lo"][1] < floor_y + room_h * 0.7]

    gy = [g["p"][1] for g in graph]
    gmin, gmax = (min(gy), max(gy)) if gy else (0.0, 0.0)

    below = sum(1 for y in gy if y < floor_y - 0.1)
    safe_ceiling = floor_y + CEIL_FRAC * room_h
    too_high = sum(1 for y in gy if y > safe_ceiling)

    # node<->obstacle bbox overlap (clipping through scenery)
    clip = 0
    for g in graph:
        p = g["p"]
        nlo = [p[0] - NODE_R, p[1] - NODE_R, p[2] - NODE_R]
        nhi = [p[0] + NODE_R, p[1] + NODE_R, p[2] + NODE_R]
        if any(aabb_overlap(nlo, nhi, o["lo"], o["hi"]) for o in obstacles):
            clip += 1

    # out of horizontal bounds (a small margin in)
    m = room_ext * 0.02
    outb = sum(1 for g in graph
               if g["p"][0] < blo[0] + m or g["p"][0] > bhi[0] - m
               or g["p"][2] < blo[2] + m or g["p"][2] > bhi[2] - m)

    # spine axis: principal horizontal direction (first->last is noisy; use endpoints span)
    spine_deg = None
    if len(graph) >= 2:
        a, b = graph[0]["p"], graph[-1]["p"]
        dx, dz = b[0] - a[0], b[2] - a[2]
        if abs(dx) + abs(dz) > 1e-3:
            ang = math.degrees(math.atan2(abs(dz), abs(dx)))  # 0 = x-axis, 90 = z-axis
            spine_deg = min(ang, 90 - ang)  # distance to nearest principal axis

    # strand aspect: how strand-like the graph is = (x-extent / z-extent). High = a strand running along x (good for
    # hall scenes where the conversation should follow the corridor); ~1 = a blob filling the room.
    gx = [g["p"][0] for g in graph]
    gz = [g["p"][2] for g in graph]
    ex = (max(gx) - min(gx)) if gx else 0.0
    ez = (max(gz) - min(gz)) if gz else 1e-6
    strand_aspect = ex / max(ez, 1e-6)

    n = max(len(graph), 1)
    return {
        "theme": d["theme"], "nodes": d["nodes"], "objects": len(objs), "obstacles": len(obstacles),
        "floor_y": round(floor_y, 2), "room_h": round(room_h, 1),
        "graph_y": [round(gmin, 2), round(gmax, 2)], "safe_ceiling": round(safe_ceiling, 2),
        "strand_aspect": round(strand_aspect, 2),
        "below_floor": [below, round(100 * below / n, 1)],
        "above_ceiling": [too_high, round(100 * too_high / n, 1)],
        "obj_clip": [clip, round(100 * clip / n, 1)],
        "out_bounds": [outb, round(100 * outb / n, 1)],
        "spine_axis_dev_deg": round(spine_deg, 1) if spine_deg is not None else None,
    }


# range tests: (label, key->value, predicate, target text)
def checks(r):
    out = []
    def pct(k): return r[k][1]
    out.append(("below_floor",   pct("below_floor") == 0,            "want 0%  (strand not in the ground)"))
    out.append(("above_ceiling", pct("above_ceiling") <= 5,         "want <=5% (not floating into the roof)"))
    out.append(("obj_clip",      pct("obj_clip") <= 12,             "want <=12% (strand weaves around scenery)"))
    out.append(("out_bounds",    pct("out_bounds") <= 2,            "want <=2% (inside the room)"))
    if r["spine_axis_dev_deg"] is not None:
        out.append(("spine_axis", r["spine_axis_dev_deg"] <= 20,    "want <=20deg from a principal axis (hall-aligned)"))
    return out


def report(d):
    r = analyze(d)
    cks = checks(r)
    npass = sum(1 for _, ok, _ in cks if ok)
    print(f"=== GEOM CHECK: {r['theme']}  ({r['nodes']} nodes, {r['objects']} objs, {r['obstacles']} obstacles) ===")
    print(f"  floor_y={r['floor_y']}  room_h={r['room_h']}  graph_y={r['graph_y']}  strand_aspect={r['strand_aspect']} (x/z; >2 = strand, ~1 = blob)")
    for label, ok, target in cks:
        val = r.get(label)
        if isinstance(val, list):
            val = f"{val[0]} ({val[1]}%)"
        elif label == "spine_axis":
            val = f"{r['spine_axis_dev_deg']}deg"
        print(f"  [{'PASS' if ok else 'FAIL'}] {label:14s} = {val:18s} {target}")
    print(f"  --> {npass}/{len(cks)} checks pass")
    return r, npass, len(cks)


def main():
    args = [a for a in sys.argv[1:]]
    as_json = "--json" in args
    args = [a for a in args if a != "--json"]
    if "--all" in args:
        files = sorted(glob.glob("/tmp/scene_geom_*.json"))
    else:
        files = args or sorted(glob.glob("/tmp/scene_geom_*.json"))
    results = []
    for f in files:
        try:
            d = json.load(open(f))
        except Exception as e:
            print(f"skip {f}: {e}"); continue
        if as_json:
            results.append(analyze(d))
        else:
            report(d)
            print()
    if as_json:
        print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
