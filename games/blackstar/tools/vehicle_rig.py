#!/usr/bin/env python3
"""vehicle_rig.py -- split a vehicle so its wheels actually turn.

limb_split deliberately skips vehicles (a car has no legs). But vehicles were then
left completely rigid -- they slide along the ground with painted-on wheels. This
is their counterpart: find the wheels, give each its own pivot, and tag them so the
engine's existing tire animation rolls them with movement.

Wheel detection is by SHAPE and PLACEMENT, not by name, since these meshes rarely
label anything:
  - roundness   : the component's cross-section about its thin axis is circular
  - height      : it sits in the lower part of the body
  - pairing     : a matching component exists mirrored across the centre line
The pairing test is what rejects round-but-not-wheel parts like exhausts or lights,
which have no mirror twin at the same height and radius.

Usage:
  python3 tools/vehicle_rig.py --target CAR.gltf --name mycar --out rigged
  python3 tools/vehicle_rig.py --batch vehicles/ --out rigged
  python3 tools/install_models.py --parts-dir rigged --engine engine_src
"""
import sys, os, re, argparse

TAG = {"body": 0, "tire": 1}

def _helpers():
    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path: sys.path.insert(0, here)
    import importlib.util, glob
    def imp(stem):
        try: return __import__(stem)
        except Exception: pass
        best, bv = None, -1
        for c in glob.glob(os.path.join(here, stem + "*.py")):
            b = os.path.basename(c)[:-3]
            m = re.search(r"_v(\d+)$", b); v = int(m.group(1)) if m else 0
            if v > bv: bv, best = v, b
        if not best: return None
        sp = importlib.util.spec_from_file_location(best, os.path.join(here, best + ".py"))
        mod = importlib.util.module_from_spec(sp); sys.modules[best] = mod
        sp.loader.exec_module(mod); return mod
    return imp("process_models"), imp("texture_export"), imp("part_harvest")

def wheel_score(V, lo, hi):
    """(is_round, radius, axis, centroid) for one component."""
    import numpy as np
    ext = V.max(0) - V.min(0)
    ax = int(np.argmin(ext))                     # thin axis = candidate axle
    other = [i for i in (0, 1, 2) if i != ax]
    c = V.mean(0)
    d = V[:, other] - c[other]
    r = np.linalg.norm(d, axis=1)
    if r.mean() < 1e-6: return False, 0.0, ax, c
    roundness = float(r.std() / r.mean())        # low = circular rim
    flat = float(ext[ax] / max(1e-6, max(ext)))  # thin relative to its diameter
    fy = (c[1] - lo[1]) / max(1e-6, hi[1] - lo[1])
    ok = roundness < 0.28 and flat < 0.55 and fy < 0.55
    return ok, float(r.mean()), ax, c

def find_wheels(comps, lo, hi, pair_tol=0.18):
    """Indices of components that look like wheels AND have a mirror twin."""
    import numpy as np
    cands = []
    for i, (cv, cf) in enumerate(comps):
        ok, rad, ax, c = wheel_score(cv, lo, hi)
        if ok: cands.append((i, rad, ax, c))
    cx = (lo[0] + hi[0]) * 0.5
    keep = []
    for i, rad, ax, c in cands:
        mirrored = False
        for j, rad2, ax2, c2 in cands:
            if i == j: continue
            if (abs((c[0] - cx) + (c2[0] - cx)) < pair_tol * max(1e-6, hi[0] - lo[0])
                    and abs(c[1] - c2[1]) < pair_tol
                    and abs(rad - rad2) < pair_tol * max(rad, rad2, 1e-6) * 3):
                mirrored = True; break
        if mirrored: keep.append((i, rad, ax, c))
    if not keep: return keep
    # Real wheels on one vehicle share a radius (or two: steer vs drive). Round
    # pipes, lights and vents also pass the mirror test, so keep only the dominant
    # radius cluster -- without this a truck reported 36 "wheels".
    import numpy as np
    rads = np.array([r for _i, r, _a, _c in keep])
    best, best_n = None, 0
    for r0 in rads:
        n = int((np.abs(rads - r0) < r0 * 0.35).sum())
        if n > best_n or (n == best_n and best is not None and r0 > best):
            best, best_n = float(r0), n
    keep = [k for k in keep if abs(k[1] - best) < best * 0.35]
    # a vehicle has an even number of wheels; drop the odd one out if unpaired
    if len(keep) % 2 == 1 and len(keep) > 1:
        cxm = (lo[0] + hi[0]) * 0.5
        keep.sort(key=lambda k: abs(abs(k[3][0] - cxm) - np.median(
            [abs(x[3][0] - cxm) for x in keep])))
        keep = keep[:len(keep) - 1]
    return keep

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", default=None)
    ap.add_argument("--batch", default=None)
    ap.add_argument("--name", default=None)
    ap.add_argument("--out", default="rigged")
    ap.add_argument("--min-tris", type=int, default=20)
    ap.add_argument("--texture", default="plate_blue.png")
    a = ap.parse_args()

    pm, te, ph = _helpers()
    if not (pm and te and ph): raise SystemExit("need process_models / texture_export / part_harvest")

    if a.batch:
        found = []
        for dp, dirs, files in os.walk(a.batch):
            dirs[:] = [d for d in dirs if d not in {"parts", "iqm", "build", "rigged"}]
            gl = [f for f in files if f.lower().endswith((".gltf", ".glb"))]
            ob = [f for f in files if f.lower().endswith(".obj")]
            cand = gl or ob
            if not cand: continue
            def sz(f):
                try: return os.path.getsize(os.path.join(dp, f))
                except OSError: return 0
            found.append((os.path.basename(dp.rstrip("/")), os.path.join(dp, max(cand, key=sz))))
        seen = {}
        print(f"{len(found)} model(s) under {a.batch}\n")
        for base, path in found:
            nm = re.sub(r"[^a-z0-9]", "", base.lower())[:26] or "model"
            if nm in seen: seen[nm] += 1; nm = f"{nm}{seen[nm]}"
            else: seen[nm] = 0
            os.system('%s %s --target "%s" --name %sveh --out "%s" --min-tris %d >/dev/null 2>&1'
                      % (sys.executable, os.path.abspath(__file__), path, nm, a.out, a.min_tris))
            man = os.path.join(a.out, "imported_%sveh_parts.txt" % nm)
            if os.path.exists(man):
                rows = [l for l in open(man).read().split("\n") if l.strip()]
                w = sum(1 for r in rows if " tire " in r)
                print(f"  {nm[:34]:34s} {len(rows)} parts, {w} wheels")
            else:
                print(f"  {nm[:34]:34s} FAIL")
        return

    if not a.target or not os.path.isfile(a.target):
        print("ERROR: give --target MODEL or --batch DIR (target must exist)")
        sys.exit(2)
    name = a.name or os.path.basename(os.path.dirname(os.path.abspath(a.target)))

    geo = pm.load_geometry(a.target)
    if not geo: raise SystemExit("could not read " + a.target)
    V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
    lo, hi = V.min(0), V.max(0); ctr = (lo + hi) * 0.5; span = float((hi - lo).max()) or 1.0
    Vn = ((V - ctr) / span * 2.0).astype("f4")
    nlo, nhi = Vn.min(0), Vn.max(0)

    comps = ph.components(Vn, F.astype(np.int64), min_tris=a.min_tris)
    wheels = find_wheels(comps, nlo, nhi)
    widx = {i for i, _r, _a, _c in wheels}
    print(f"{name}: {len(F):,} tris, {len(comps)} components -> {len(wheels)} wheels")

    os.makedirs(a.out, exist_ok=True)
    manifest = []

    def write(pi, items, tag, pivot, axis):
        Vs, Fs, off = [], [], 0
        for (cv, cf) in items:
            Vs.append(cv); Fs.append(cf + off); off += len(cv)
        Vm = np.vstack(Vs).astype("f4"); Fm = np.vstack(Fs).astype("u4")
        if tag == "tire":
            Vm = (Vm - pivot).astype("f4")          # spin about its own hub
        Vu, UV, Fu = ph.box_uvs(Vm, Fm)
        out = os.path.join(a.out, "imported_%s_part%d.iqm" % (name, pi))
        te.write_iqm(out, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), a.texture)
        manifest.append("%s %s %s %d %d %.5f %.5f %.5f %d" %
                        (os.path.basename(out), a.texture, tag, axis, TAG[tag],
                         pivot[0], pivot[1], pivot[2], -1))
        return len(Fm)

    body = [c for i, c in enumerate(comps) if i not in widx]
    pi = 0
    if body:
        n = write(pi, body, "body", np.zeros(3, "f4"), -1); pi += 1
        print(f"  body   {len(body):3d} comps, {n:6d} tris")
    for (i, rad, ax, c) in wheels:
        cv, cf = comps[i]
        n = write(pi, [(cv, cf)], "tire", cv.mean(0).astype("f4"), ax)
        print(f"  wheel  r={rad:.3f} axis={'XYZ'[ax]} at (%+.2f %+.2f %+.2f), {n} tris"
              % (c[0], c[1], c[2]))
        pi += 1

    man = os.path.join(a.out, "imported_%s_parts.txt" % name)
    open(man, "w").write("\n".join(manifest))
    print(f"\nwrote {len(manifest)} parts -> {man}")
    if not wheels:
        print("  !! no wheels detected -- the model may have them merged into the body,")
        print("     or they are not round/low/paired enough to pass the test")

if __name__ == "__main__":
    main()
