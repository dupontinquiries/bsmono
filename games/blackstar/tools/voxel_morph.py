#!/usr/bin/env python3
"""voxel_morph.py -- reassembly-swarm transformation.

The user's point: individual shared parts look too basic on their own, so never
show them on their own. Break a model into many small clusters that fly apart and
REASSEMBLE, and the eye reads density and motion rather than the shape of any one
piece. It also sidesteps what has dogged the procedural detail work -- a cluster
does not have to look like anything, because it is never seen alone.

Each cluster becomes an engine part carrying two positions:
  ASSEMBLED  where it belongs in the finished model
  DISPERSED  where it starts, pushed out along its own direction with a swirl
Clusters are cut from the model's own surface, so the assembled state IS the
original mesh -- nothing is approximated once the transformation completes.
"""
import sys, os, math, argparse

MORPH_TAG = 20

def _helpers():
    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path: sys.path.insert(0, here)
    import importlib.util, glob, re
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

def cluster_faces(V, F, n_clusters, seed=0):
    """Partition by triangle centroid. Compactness matters: a stringy cluster reads
    as debris when it moves, a compact one reads as a panel of plating."""
    import numpy as np
    rng = np.random.default_rng(seed)
    c = V[F].mean(1)
    k = int(min(n_clusters, max(1, len(c) // 4)))
    cen = c[rng.choice(len(c), k, replace=False)].copy()
    lab = np.zeros(len(c), np.int64)
    for _ in range(8):
        lab = np.argmin(((c[:, None, :] - cen[None, :, :]) ** 2).sum(-1), axis=1)
        for j in range(k):
            m = lab == j
            if m.any(): cen[j] = c[m].mean(0)
    return lab, k

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True)
    ap.add_argument("--name", default=None)
    ap.add_argument("--out", default="morph")
    ap.add_argument("--clusters", type=int, default=96)
    ap.add_argument("--spread", type=float, default=1.6)
    ap.add_argument("--swirl", type=float, default=0.8)
    ap.add_argument("--max-tris", type=int, default=60000)
    ap.add_argument("--texture", default="plate_blue.png")
    a = ap.parse_args()
    pm, te, ph = _helpers()
    if not (pm and te and ph): raise SystemExit("need process_models / texture_export / part_harvest")
    if not os.path.isfile(a.target):
        print("ERROR: target not found: %r" % a.target); sys.exit(2)
    name = a.name or os.path.basename(os.path.dirname(os.path.abspath(a.target)))
    geo = pm.load_geometry(a.target)
    if not geo: raise SystemExit("could not read " + a.target)
    V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
    print("%s: %d tris" % (name, len(F)))
    if len(F) > a.max_tris:
        V2, F2 = pm._decimate(V, F, a.max_tris)
        V, F = np.asarray(V2, "f4"), np.asarray(F2, "u4")
        print("  decimated to %d tris" % len(F))
    lo, hi = V.min(0), V.max(0); ctr = (lo + hi) * 0.5
    span = float((hi - lo).max()) or 1.0
    Vn = ((V - ctr) / span * 2.0).astype("f4")
    lab, k = cluster_faces(Vn, F, min(a.clusters, 126))
    print("  %d clusters" % k)
    os.makedirs(a.out, exist_ok=True)
    manifest = []; rng = np.random.default_rng(1); written = 0; sizes = []
    for j in range(k):
        m = lab == j
        if not m.any(): continue
        Fc = F[m]; vids = np.unique(Fc)
        remap = -np.ones(len(Vn), np.int64); remap[vids] = np.arange(len(vids))
        Vc = Vn[vids]; Fl = remap[Fc].astype("u4")
        if len(Fl) < 2: continue
        pivot = Vc.mean(0).astype("f4")
        Vloc = (Vc - pivot).astype("f4")
        r = pivot.copy(); r[1] = 0.0
        nn = float(np.linalg.norm(r))
        dirv = (r / nn) if nn > 1e-5 else np.array([1.0, 0.0, 0.0], "f4")
        tang = np.array([-dirv[2], 0.0, dirv[0]], "f4")
        disp = (pivot + dirv * a.spread + tang * a.swirl * float(rng.uniform(-1, 1))
                + np.array([0.0, float(rng.uniform(-0.4, 0.4)), 0.0], "f4")).astype("f4")
        Vu, UV, Fu = ph.box_uvs(Vloc, Fl)
        out = os.path.join(a.out, "imported_%s_part%d.iqm" % (name, written))
        te.write_iqm(out, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), a.texture)
        manifest.append("%s %s morph 0 %d %.5f %.5f %.5f -1 %.5f %.5f %.5f"
                        % (os.path.basename(out), a.texture, MORPH_TAG,
                           pivot[0], pivot[1], pivot[2], disp[0], disp[1], disp[2]))
        sizes.append(len(Fl)); written += 1
    man = os.path.join(a.out, "imported_%s_parts.txt" % name)
    open(man, "w").write("\n".join(manifest))
    import statistics as st
    print("\n  wrote %d clusters -> %s" % (written, man))
    if sizes:
        print("  cluster size: min %d median %d max %d tris" % (min(sizes), int(st.median(sizes)), max(sizes)))
    print("  each carries assembled + dispersed positions (atag %d)" % MORPH_TAG)

if __name__ == "__main__":
    main()
