#!/usr/bin/env python3
"""bot_generate.py -- build a robot from harvested catalog parts.

Takes the part catalog produced by part_harvest.py and assembles it into a bot
using a skeleton template, then writes the result as per-part IQMs + a manifest
that install_models.py can put straight into the game.

The skeleton templates follow plan.md Rule 3 (vehicle topology chooses skeleton):
    jet / car      -> human            tank -> heavy knight
    spider tank    -> multi-legged     bird -> digitigrade

Slot filling uses the catalog's own naming: limbs want struts, torso/pelvis want
panels or blocks, joints want discs or wheels. Where a category is missing the
generator falls back rather than failing, and reports what it substituted.

This is a FIRST PASS. It produces a correctly-proportioned assembly of real
harvested geometry -- not a sculpted character. Judge it on whether the parts read
as mechanical and sit in the right places; refinement comes from what looks wrong.

Usage:
  python3 tools/bot_generate.py --catalog greeblecat --name mybot --out generated
  python3 tools/bot_generate.py --catalog greeblecat --name tankbot \
      --skeleton knight --seed 7 --out generated
  python3 tools/install_models.py --parts-dir generated --engine engine_src
"""
import sys, os, json, math, random, argparse

# slot = (name, centre x,y,z, size x,y,z, preferred categories)
# coordinates are in a 0..1 body box; y is up, z is forward
SKELETONS = {
    "human": [
        ("head",      0.00, 0.90, 0.00, 0.16, 0.14, 0.16, ("block", "disc")),
        ("torso",     0.00, 0.68, 0.00, 0.34, 0.26, 0.22, ("panel", "block")),
        ("pelvis",    0.00, 0.50, 0.00, 0.26, 0.12, 0.20, ("block", "panel")),
        ("shoulderL",-0.22, 0.78, 0.00, 0.12, 0.12, 0.14, ("disc", "wheel")),
        ("shoulderR", 0.22, 0.78, 0.00, 0.12, 0.12, 0.14, ("disc", "wheel")),
        ("uarmL",    -0.26, 0.66, 0.00, 0.10, 0.20, 0.10, ("strut", "barrel")),
        ("uarmR",     0.26, 0.66, 0.00, 0.10, 0.20, 0.10, ("strut", "barrel")),
        ("larmL",    -0.28, 0.46, 0.00, 0.09, 0.20, 0.09, ("strut", "barrel")),
        ("larmR",     0.28, 0.46, 0.00, 0.09, 0.20, 0.09, ("strut", "barrel")),
        ("hipL",     -0.11, 0.46, 0.00, 0.11, 0.11, 0.12, ("disc", "wheel")),
        ("hipR",      0.11, 0.46, 0.00, 0.11, 0.11, 0.12, ("disc", "wheel")),
        ("thighL",   -0.11, 0.32, 0.00, 0.13, 0.22, 0.13, ("strut", "panel")),
        ("thighR",    0.11, 0.32, 0.00, 0.13, 0.22, 0.13, ("strut", "panel")),
        ("shinL",    -0.11, 0.12, 0.00, 0.11, 0.22, 0.11, ("strut", "panel")),
        ("shinR",     0.11, 0.12, 0.00, 0.11, 0.22, 0.11, ("strut", "panel")),
        ("footL",    -0.11, 0.02, 0.04, 0.13, 0.06, 0.20, ("panel", "block")),
        ("footR",     0.11, 0.02, 0.04, 0.13, 0.06, 0.20, ("panel", "block")),
    ],
}

def _mirror(slots):
    return slots

def make_knight(base):
    """Heavier, wider, shorter -- plan.md's tank skeleton."""
    out = []
    for (n, x, y, z, sx, sy, sz, cats) in base:
        out.append((n, x * 1.25, 0.06 + y * 0.92, z,
                    sx * 1.35, sy * 0.95, sz * 1.3, cats))
    return out

def make_spider(base):
    """Multi-legged: keep the torso, replace legs with a radial ring of struts."""
    out = [s for s in base if not any(k in s[0] for k in ("thigh", "shin", "foot", "hip"))]
    for i in range(6):
        a = (i / 6.0) * math.tau
        out.append((f"leg{i}", 0.30 * math.cos(a), 0.34, 0.30 * math.sin(a),
                    0.09, 0.26, 0.09, ("strut", "panel")))
        out.append((f"legtip{i}", 0.44 * math.cos(a), 0.10, 0.44 * math.sin(a),
                    0.08, 0.20, 0.08, ("strut", "panel")))
    return out

def build_skeleton(kind):
    base = SKELETONS["human"]
    if kind == "human":  return base
    if kind == "knight": return make_knight(base)
    if kind == "spider": return make_spider(base)
    raise SystemExit(f"unknown skeleton '{kind}' (human|knight|spider)")

def load_catalog(cat_dir):
    idx = os.path.join(cat_dir, "partlib.json")
    if not os.path.isfile(idx):
        raise SystemExit(f"no partlib.json in {cat_dir}")
    d = json.load(open(idx))
    mesh_dir = os.path.join(cat_dir, "meshes")
    parts = [p for p in d["parts"] if p.get("mesh") and
             os.path.exists(os.path.join(mesh_dir, p["mesh"] + ".obj"))]
    if not parts:
        raise SystemExit(f"{cat_dir} has no emitted meshes -- re-run part_harvest with --emit N")
    by = {}
    for p in parts: by.setdefault(p["name"], []).append(p)
    return d, parts, by, mesh_dir

def read_obj(path):
    import numpy as np
    V, F = [], []
    for line in open(path, errors="ignore"):
        if line.startswith("v "):
            q = line.split(); V.append((float(q[1]), float(q[2]), float(q[3])))
        elif line.startswith("f "):
            idx = [int(t.split("/")[0]) for t in line.split()[1:]]
            for k in range(1, len(idx) - 1):
                F.append((idx[0] - 1, idx[k] - 1, idx[k + 1] - 1))
    return np.array(V, "f4"), np.array(F, "u4")

def fit_into(V, cx, cy, cz, sx, sy, sz):
    """Scale a part into a slot box and move it to the slot centre."""
    import numpy as np
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    s = np.array([sx, sy, sz], "f4") / span
    k = float(s.min())                      # uniform scale: never distort a part
    Vc = (V - (lo + hi) * 0.5) * k
    return (Vc + np.array([cx, cy, cz], "f4")).astype("f4")

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--catalog", required=True, help="a part_harvest --out dir with meshes/")
    ap.add_argument("--name", default="genbot")
    ap.add_argument("--skeleton", default="human", choices=("human", "knight", "spider"))
    ap.add_argument("--out", default="generated")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--texture", default="plate_blue.png")
    a = ap.parse_args()

    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path: sys.path.insert(0, here)
    import importlib.util, glob, re
    te = None
    for c in sorted(glob.glob(os.path.join(here, "texture_export*.py")), reverse=True):
        b = os.path.basename(c)[:-3]
        try:
            sp = importlib.util.spec_from_file_location(b, c)
            m = importlib.util.module_from_spec(sp); sys.modules[b] = m
            sp.loader.exec_module(m); te = m; break
        except Exception: continue
    if te is None or not hasattr(te, "write_iqm"):
        raise SystemExit("need texture_export.py beside this tool (for the IQM writer)")

    _d, parts, by_name, mesh_dir = load_catalog(a.catalog)
    rng = random.Random(a.seed)
    slots = build_skeleton(a.skeleton)
    os.makedirs(a.out, exist_ok=True)

    print(f"catalog: {len(parts)} usable exemplars across {len(by_name)} categories")
    print(f"skeleton '{a.skeleton}': {len(slots)} slots\n")

    manifest = []
    subs = 0
    staged = []          # (slot_name, V, F, pick) -- placed, not yet normalised
    for pi, (sname, cx, cy, cz, sx, sy, sz, cats) in enumerate(slots):
        pick = None
        for c in cats:
            if by_name.get(c):
                pick = rng.choice(by_name[c]); break
        if pick is None:                      # fall back to anything available
            pick = rng.choice(parts); subs += 1
        V, F = read_obj(os.path.join(mesh_dir, pick["mesh"] + ".obj"))
        staged.append((sname, fit_into(V, cx, cy, cz, sx, sy, sz), F, pick))
        print("  %-10s <- %-28s %5d tris" % (sname, pick["mesh"][:28], len(F)))

    # The engine expects a model CENTRED on the origin spanning about -1..1.
    # Parts were laid out in a 0..1 body box, so normalise the whole assembly as a
    # GROUP (preserving relative placement) -- otherwise the bot sits half-height
    # and off-centre, and the preview camera ends up inside it.
    allV = np.vstack([V for (_n, V, _F, _p) in staged])
    lo, hi = allV.min(0), allV.max(0)
    ctr = (lo + hi) * 0.5
    span = float((hi - lo).max()) or 1.0
    for pi, (sname, V, F, pick) in enumerate(staged):
        Vn = ((V - ctr) / span * 2.0).astype("f4")
        Vu, UV, Fu = te.box_uvs(Vn, F) if hasattr(te, "box_uvs") else (Vn, np.zeros((len(Vn), 2), "f4"), F)
        out_iqm = os.path.join(a.out, "imported_%s_part%d.iqm" % (a.name, pi))
        te.write_iqm(out_iqm, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), a.texture)
        manifest.append("%s %s %s %d %d %.5f %.5f %.5f" %
                        (os.path.basename(out_iqm), a.texture, "body", -1, 0, 0.0, 0.0, 0.0))

    man = os.path.join(a.out, "imported_%s_parts.txt" % a.name)
    open(man, "w").write("\n".join(manifest))
    print(f"\nwrote {len(manifest)} parts + {man}")
    if subs: print(f"  ({subs} slot(s) had no matching category and used a fallback part)")
    print(f"\ninstall it:\n  python3 tools/install_models.py --parts-dir {a.out} --engine engine_src")

if __name__ == "__main__":
    main()
