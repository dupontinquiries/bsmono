#!/usr/bin/env python3
"""solid_parts.py -- generate SOLID mechanical parts, because harvested ones aren't.

Measured on the harvested catalog: 0 of 120 exemplars are closed, median 31% open
edges. They are fragments of a shell, so a "rib" built from one is a curved sheet
with no thickness -- it reads as a shard, not a strut. Filtering cannot fix that;
there is nothing closed to filter to.

So generate them instead. Every primitive here is watertight by construction and
sized in real proportions, which also makes them controllable: a rib can be asked
for at a specific length and thickness rather than whatever a fragment happened to
be.

Shapes: box, plate, strut (tapered), cylinder (capped), ibeam, ring segment, bolt.

Usage:
  python3 tools/solid_parts.py --out solidcat --count 120
  python3 tools/detail_apply.py --target M.gltf --catalog solidcat ...
"""
import sys, os, json, math, argparse

def _box(w, h, d):
    import numpy as np
    x, y, z = w * .5, h * .5, d * .5
    V = np.array([(-x,-y,-z),(x,-y,-z),(x,y,-z),(-x,y,-z),
                  (-x,-y, z),(x,-y, z),(x,y, z),(-x,y, z)], "f4")
    F = np.array([(0,2,1),(0,3,2),(4,5,6),(4,6,7),(0,1,5),(0,5,4),
                  (1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,0,4),(3,4,7)], "u4")
    return V, F

def _taper(w, h, d, top=0.55):
    """A strut: box that narrows toward the top. Reads as a structural member."""
    import numpy as np
    V, F = _box(w, h, d)
    up = V[:, 1] > 0
    V[up, 0] *= top; V[up, 2] *= top
    return V, F

def _cyl(r, h, seg=12, cap=True):
    import numpy as np
    V = []; F = []
    for i in range(seg):
        a = 2 * math.pi * i / seg
        V.append((math.cos(a) * r, -h * .5, math.sin(a) * r))
        V.append((math.cos(a) * r,  h * .5, math.sin(a) * r))
    for i in range(seg):
        a0, a1 = 2 * i, 2 * ((i + 1) % seg)
        F += [(a0, a1, a0 + 1), (a1, a1 + 1, a0 + 1)]
    if cap:
        base = len(V); V.append((0, -h * .5, 0)); V.append((0, h * .5, 0))
        for i in range(seg):
            a0, a1 = 2 * i, 2 * ((i + 1) % seg)
            F.append((base, a1, a0))
            F.append((base + 1, a0 + 1, a1 + 1))
    return np.array(V, "f4"), np.array(F, "u4")

def _ibeam(w, h, d, t=0.28):
    """Two flanges and a web -- three boxes fused; the classic structural profile."""
    import numpy as np
    parts = [_box(w, h * t, d), _box(w, h * t, d), _box(w * t, h * (1 - 2 * t), d * .9)]
    offs = [(0, -h * .5 + h * t * .5, 0), (0, h * .5 - h * t * .5, 0), (0, 0, 0)]
    V = []; F = []; off = 0
    for (v, f), o in zip(parts, offs):
        V.append(v + np.array(o, "f4")); F.append(f + off); off += len(v)
    return np.vstack(V).astype("f4"), np.vstack(F).astype("u4")

def _ring(r, thick, h, arc=math.pi * 0.55, seg=10):
    """A rib: a curved bar with real cross-section, not a sheet."""
    import numpy as np
    V = []; F = []
    for i in range(seg + 1):
        a = -arc * .5 + arc * i / seg
        ca, sa = math.cos(a), math.sin(a)
        for (dr, dy) in ((-thick * .5, -h * .5), (thick * .5, -h * .5),
                         (thick * .5,  h * .5), (-thick * .5,  h * .5)):
            V.append(((r + dr) * ca, dy, (r + dr) * sa))
    for i in range(seg):
        b0, b1 = i * 4, (i + 1) * 4
        for k in range(4):
            k2 = (k + 1) % 4
            F += [(b0 + k, b1 + k, b0 + k2), (b0 + k2, b1 + k, b1 + k2)]
    for b, flip in ((0, False), (seg * 4, True)):
        q = [b, b + 1, b + 2, b + 3]
        F += ([(q[0], q[2], q[1]), (q[0], q[3], q[2])] if flip
              else [(q[0], q[1], q[2]), (q[0], q[2], q[3])])
    return np.array(V, "f4"), np.array(F, "u4")

def _bolt(r, h):
    import numpy as np
    return _cyl(r, h, seg=6)

def openness(F):
    from collections import Counter
    ec = Counter()
    for t in F:
        for a, b in ((t[0], t[1]), (t[1], t[2]), (t[2], t[0])):
            ec[(min(int(a), int(b)), max(int(a), int(b)))] += 1
    return sum(1 for v in ec.values() if v == 1) / max(1, len(ec))

def build(rng):
    """One random part, with its catalog category."""
    kind = rng.choice(["strut", "strut", "block", "panel", "barrel",
                       "disc", "tread_link", "turret_greeble"])
    if kind == "strut":
        return _taper(rng.uniform(.10, .22), rng.uniform(.55, 1.0),
                      rng.uniform(.10, .22), rng.uniform(.45, .8)), kind
    if kind == "block":
        return _box(rng.uniform(.25, .6), rng.uniform(.25, .6), rng.uniform(.25, .6)), kind
    if kind == "panel":
        return _box(rng.uniform(.5, 1.0), rng.uniform(.06, .14), rng.uniform(.35, .8)), kind
    if kind == "barrel":
        return _cyl(rng.uniform(.10, .20), rng.uniform(.6, 1.1)), kind
    if kind == "disc":
        return _cyl(rng.uniform(.25, .45), rng.uniform(.08, .18)), kind
    if kind == "tread_link":
        return _ring(rng.uniform(.35, .6), rng.uniform(.08, .16),
                     rng.uniform(.12, .26), arc=rng.uniform(.5, 1.1)), kind
    return _ibeam(rng.uniform(.25, .5), rng.uniform(.4, .8), rng.uniform(.2, .4)), kind

def main():
    import numpy as np, random
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--count", type=int, default=120)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()

    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path: sys.path.insert(0, here)
    import importlib.util, glob, re
    te = ph = None
    for stem, tgt in (("texture_export", "te"), ("part_harvest", "ph")):
        best, bv = None, -1
        for c in glob.glob(os.path.join(here, stem + "*.py")):
            b = os.path.basename(c)[:-3]
            m = re.search(r"_v(\d+)$", b); v = int(m.group(1)) if m else 0
            if v > bv: bv, best = v, b
        if best:
            sp = importlib.util.spec_from_file_location(best, os.path.join(here, best + ".py"))
            mod = importlib.util.module_from_spec(sp); sys.modules[best] = mod
            sp.loader.exec_module(mod)
            if tgt == "te": te = mod
            else: ph = mod
    if not te: raise SystemExit("need texture_export.py beside this tool")

    mesh_dir = os.path.join(a.out, "meshes")
    os.makedirs(mesh_dir, exist_ok=True)
    rng = random.Random(a.seed)
    parts = []; worst = 0.0
    for i in range(a.count):
        (V, F), kind = build(rng)
        o = openness(F); worst = max(worst, o)
        stem = "solid_%s_%03d" % (kind, i)
        lines = ["# solid primitive"]
        lines += ["v %.6f %.6f %.6f" % tuple(p) for p in V]
        lines += ["f %d %d %d" % (x + 1, y + 1, z + 1) for x, y, z in F]
        open(os.path.join(mesh_dir, stem + ".obj"), "w").write("\n".join(lines) + "\n")
        if ph and hasattr(ph, "box_uvs"):
            Vu, UV, Fu = ph.box_uvs(V, F)
        else:
            Vu, UV, Fu = V, np.zeros((len(V), 2), "f4"), F
        te.write_iqm(os.path.join(mesh_dir, stem + ".iqm"),
                     Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), "plate_blue.png")
        parts.append({"sig": stem, "tris": int(len(F)), "size": 1.0, "from": "generated",
                      "name": kind, "instances": 1, "residual_mean": 0.0,
                      "residual_max": 0.0, "mesh": stem})
    json.dump({"parts": parts, "placements": {}, "rules": {}},
              open(os.path.join(a.out, "partlib.json"), "w"), indent=1)
    from collections import Counter
    print("generated %d solid parts -> %s" % (len(parts), a.out))
    print("  by category:", dict(Counter(p["name"] for p in parts)))
    print("  worst openness across all parts: %.1f%%  (harvested median was 31%%)" % (100 * worst))

if __name__ == "__main__":
    main()
