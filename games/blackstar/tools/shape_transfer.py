#!/usr/bin/env python3
"""shape_transfer.py -- swap low-detail parts for shape-matched high-detail ones.

The retrieval test showed similar blobs really do have similar details (-61% L1 on
a random split). This applies that directly: for each component of a target model,
find the donor component whose COARSE SHAPE matches best, and transplant the
donor's actual mesh, fitted to the target component's own box.

Why transplant geometry rather than decode a generated depth map: the depth
encoding loses concavities and thin features, and reconstructing a mesh from six
views reintroduces exactly the blobbiness we are trying to remove. The donor mesh
is already correct geometry; the only question is which one and how to place it.

This is not generative -- it cannot invent detail the donors lack. But it needs no
GPU, it produces real Transformers geometry, and it is the baseline any trained
model has to beat.

Usage:
  python3 tools/shape_transfer.py --donors donors/ --target M.gltf \
      --name upgraded --out upgraded
  python3 tools/install_models.py --parts-dir upgraded --engine engine_src
"""
import sys, os, json, argparse

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

def descriptor(V, F, bins=16):
    """Coarse shape signature, rotation-aligned by principal axes.

    Components arrive in arbitrary orientation, so raw coordinates do not compare.
    Aligning to the principal axes first makes a strut match a strut regardless of
    which way it was modelled."""
    import numpy as np
    C = V - V.mean(0)
    try:
        _u, _s, vt = np.linalg.svd(C - C.mean(0), full_matrices=False)
        C = C @ vt.T
    except Exception:
        pass
    ext = np.maximum(C.max(0) - C.min(0), 1e-6)
    C = C / ext.max()
    # occupancy in a coarse 4x4x4 grid + normalised extents = gross form
    g = np.clip(((C - C.min(0)) / np.maximum(C.max(0) - C.min(0), 1e-6) * 3.99).astype(int), 0, 3)
    occ = np.zeros((4, 4, 4), "f4")
    np.add.at(occ, (g[:, 0], g[:, 1], g[:, 2]), 1.0)
    occ = occ.reshape(-1)
    occ /= (np.linalg.norm(occ) + 1e-6)
    aspect = np.sort(ext / ext.max())
    return np.concatenate([occ, aspect]).astype("f4")

def fit_to(Vd, target_V, keep_aspect=True):
    """Place a donor component into the target component's box."""
    import numpy as np
    tl, th = target_V.min(0), target_V.max(0)
    tc, tspan = (tl + th) * 0.5, np.maximum(th - tl, 1e-6)
    dl, dh = Vd.min(0), Vd.max(0)
    dc, dspan = (dl + dh) * 0.5, np.maximum(dh - dl, 1e-6)
    if keep_aspect:
        k = float((tspan / dspan).min())
        s = np.array([k, k, k], "f4")
    else:
        s = (tspan / dspan).astype("f4")
    return ((Vd - dc) * s + tc).astype("f4")

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--donors", required=True)
    ap.add_argument("--target", required=True)
    ap.add_argument("--name", default=None)
    ap.add_argument("--out", default="upgraded")
    ap.add_argument("--min-tris", type=int, default=40)
    ap.add_argument("--donor-min-tris", type=int, default=120,
                    help="donor components must be detailed enough to be worth swapping in")
    ap.add_argument("--min-sim", type=float, default=0.0,
                    help="only swap when the best donor match scores at least this "
                         "(0..1). Higher keeps the original silhouette; lower adds "
                         "more detail but drifts off the character's shape.")
    ap.add_argument("--gain", type=float, default=2.0,
                    help="only swap when the donor has at least this many times the tris")
    ap.add_argument("--texture", default="plate_blue.png")
    a = ap.parse_args()

    pm, te, ph = _helpers()
    if not (pm and te and ph): raise SystemExit("need process_models / texture_export / part_harvest")
    if not os.path.isfile(a.target):
        print("ERROR: target not found: %r" % a.target); sys.exit(2)
    name = a.name or os.path.basename(os.path.dirname(os.path.abspath(a.target)))

    # ---- build the donor bank
    bank_V, bank_F, bank_D, bank_src = [], [], [], []
    for dp, dirs, files in os.walk(a.donors):
        dirs[:] = [d for d in dirs if d not in {"parts", "iqm", "build"}]
        cand = [f for f in files if f.lower().endswith((".gltf", ".glb", ".obj"))]
        if not cand: continue
        def sz(f):
            try: return os.path.getsize(os.path.join(dp, f))
            except OSError: return 0
        path = os.path.join(dp, max(cand, key=sz))
        geo = pm.load_geometry(path)
        if not geo: continue
        V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
        n = 0
        for (cv, cf) in ph.components(V, F.astype(np.int64), min_tris=a.donor_min_tris):
            bank_V.append(cv); bank_F.append(cf)
            bank_D.append(descriptor(cv, cf))
            bank_src.append(os.path.basename(dp))
            n += 1
        print("  donor %-34s %5d components" % (os.path.basename(dp)[:34], n))
    if not bank_D: raise SystemExit("no donor components found")
    BD = np.stack(bank_D)
    BD /= (np.linalg.norm(BD, axis=1, keepdims=True) + 1e-6)
    print("  donor bank: %d components from %d models\n" % (len(BD), len(set(bank_src))))

    # ---- match every target component
    geo = pm.load_geometry(a.target)
    if not geo: raise SystemExit("could not read " + a.target)
    V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
    lo, hi = V.min(0), V.max(0); ctr = (lo + hi) * 0.5; span = float((hi - lo).max()) or 1.0
    Vn = ((V - ctr) / span * 2.0).astype("f4")
    comps = ph.components(Vn, F.astype(np.int64), min_tris=a.min_tris)
    print("target %s: %d tris, %d components" % (name, len(F), len(comps)))

    os.makedirs(a.out, exist_ok=True)
    kept_V, kept_F, off = [], [], 0
    swapped = 0; tris_before = 0; tris_after = 0; sim_all = []
    from collections import Counter
    used = Counter()
    for (cv, cf) in comps:
        tris_before += len(cf)
        q = descriptor(cv, cf)
        q /= (np.linalg.norm(q) + 1e-6)
        sims_q = BD @ q
        j = int(np.argmax(sims_q))
        sim = float(sims_q[j])
        sim_all.append(sim)
        dv, df = bank_V[j], bank_F[j]
        # Swap only when the donor is BOTH more detailed AND a confident shape
        # match. Without the confidence test the nearest donor is used no matter
        # how poor the fit, which is what pushed the silhouette IoU down to 0.67 --
        # the donors were partly overwriting the character's outline.
        if len(df) >= len(cf) * a.gain and sim >= a.min_sim:
            fitted = fit_to(dv, cv)
            kept_V.append(fitted); kept_F.append(df + off); off += len(fitted)
            tris_after += len(df); swapped += 1
            used[bank_src[j]] += 1
        else:
            kept_V.append(cv); kept_F.append(cf + off); off += len(cv)
            tris_after += len(cf)

    Vm = np.vstack(kept_V).astype("f4"); Fm = np.vstack(kept_F).astype("u4")
    Vu, UV, Fu = ph.box_uvs(Vm, Fm)
    out = os.path.join(a.out, "imported_%s_part0.iqm" % name)
    te.write_iqm(out, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), a.texture)
    open(os.path.join(a.out, "imported_%s_parts.txt" % name), "w").write(
        "%s %s body -1 0 0.00000 0.00000 0.00000 -1" % (os.path.basename(out), a.texture))

    st = len([1 for _cv, cf in ph.components(Vm, Fm.astype(np.int64), min_tris=50)])
    if sim_all:
        q = np.percentile(sim_all, [10, 50, 90])
        print("\n  match confidence: p10 %.3f  median %.3f  p90 %.3f" % tuple(q))
    print("  swapped %d of %d components" % (swapped, len(comps)))
    print("  triangles %d -> %d  (%.1fx)" % (tris_before, tris_after,
                                             tris_after / max(1, tris_before)))
    print("  structural members now %d  (Scorponok reference 479)" % st)
    if used:
        print("  donors drawn from:", dict(used.most_common(5)))
        if len(used) == 1:
            print("    -> every swap came from ONE donor. That is the memorisation")
            print("       failure; the bank needs more variety.")
    print("\n  install:\n    python3 tools/install_models.py --parts-dir %s --engine engine_src" % a.out)

if __name__ == "__main__":
    main()
