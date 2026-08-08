#!/usr/bin/env python3
"""part_harvest.py -- find the reusable PART LIBRARY hiding inside detailed models.

Scorponok's "detailed" look is not unique sculpting. Measured: 19 baked submeshes
resolve to 9 unique part types placed by rigid transforms (one leg segment appears
6x, mirrored/rotated around a radial body). That is a part library with placement
rules -- which is exactly what a procedural detail system needs, and also where any
remaining compression lives (store each part once, place it many times).

This tool:
  1. splits each model into connected components (real structural pieces)
  2. computes a rotation/mirror/translation-INVARIANT signature per component
  3. clusters components into part TYPES, within and ACROSS models
  4. reports reuse, and can emit a library + per-model placement manifests

The cross-model number is the one that matters: if parts recur across vehicles, a
shared library pays for every model added afterwards.

Usage:
  python3 tools/part_harvest.py MODELS_DIR                     # measure only
  python3 tools/part_harvest.py MODELS_DIR --out partlib       # write the library
  python3 tools/part_harvest.py MODELS_DIR --min-tris 200 --tol 0    # part FAMILIES
  python3 tools/part_harvest.py vehicles/tanks --out tanklib --tol 0  # one class at a time
"""
import sys, os, json, argparse, hashlib
from collections import defaultdict

def human(n):
    for u in ("B", "K", "M", "G"):
        if n < 1024 or u == "G": return f"{n:.0f}{u}" if u == "B" else f"{n:.1f}{u}"
        n /= 1024.0

def components(V, F, min_tris=1, weld=True, tol=1e-5):
    """Split a mesh into connected components.

    Meshes are very often exported UNWELDED (every triangle carries its own three
    vertices), in which case nothing shares a vertex, every triangle becomes its
    own island, and no real parts are found -- a human-skeleton reference produced
    115,386 one-triangle components this way. So weld by position first, then
    label. Set weld=False only if the mesh is known to be welded already.

    Vectorised: labels via scipy connected_components, grouping via one argsort,
    and a single reusable remap buffer touched only at each component's own
    vertices."""
    import numpy as np
    V = np.asarray(V, "f4"); F = np.asarray(F, np.int64)
    if weld and len(V):
        key = np.round(V / tol).astype(np.int64)
        _u, inv = np.unique(key, axis=0, return_inverse=True)
        nV = np.zeros((_u.shape[0], 3), "f4")
        nV[inv] = V                      # representative position per welded vertex
        F = inv[F]
        good = (F[:, 0] != F[:, 1]) & (F[:, 1] != F[:, 2]) & (F[:, 0] != F[:, 2])
        F = F[good]
        V = nV
        if len(F) == 0: return []
    n = len(V)
    try:
        from scipy.sparse import coo_matrix
        from scipy.sparse.csgraph import connected_components as _cc
        rows = np.concatenate([F[:, 0], F[:, 1], F[:, 2]])
        cols = np.concatenate([F[:, 1], F[:, 2], F[:, 0]])
        g = coo_matrix((np.ones(len(rows), np.int8), (rows, cols)), shape=(n, n))
        ncomp, labels = _cc(g, directed=False)
    except Exception:
        labels, ncomp = _labels_unionfind(F, n)

    # group vertices and faces by label with argsort instead of per-component scans
    vorder = np.argsort(labels, kind="stable")
    vb = np.searchsorted(labels[vorder], np.arange(ncomp + 1))
    flab = labels[F[:, 0]]
    forder = np.argsort(flab, kind="stable")
    fb = np.searchsorted(flab[forder], np.arange(ncomp + 1))

    remap = np.empty(n, np.int64)
    out = []
    for c in range(ncomp):
        fidx = forder[fb[c]:fb[c + 1]]
        if len(fidx) < min_tris: continue
        vidx = vorder[vb[c]:vb[c + 1]]
        if len(vidx) == 0: continue
        remap[vidx] = np.arange(len(vidx))      # only this component's vertices
        out.append((V[vidx], remap[F[fidx]]))
    return out

def _labels_unionfind(F, n):
    """Fallback labelling when scipy is unavailable (vectorised pointer-jumping)."""
    import numpy as np
    parent = np.arange(n, dtype=np.int64)
    for a, b in ((0, 1), (1, 2), (2, 0)):
        for _ in range(64):
            pa, pb = parent[F[:, a]], parent[F[:, b]]
            lo, hi = np.minimum(pa, pb), np.maximum(pa, pb)
            if not np.any(lo != hi): break
            np.minimum.at(parent, hi, lo)
            parent = parent[parent]
    for _ in range(64):
        nxt = parent[parent]
        if np.array_equal(nxt, parent): break
        parent = nxt
    uniq, labels = np.unique(parent, return_inverse=True)
    return labels, len(uniq)

def describe(V, F, model_bounds=None, repeat=1):
    """Measurable shape/placement features for one component."""
    import numpy as np
    ext = V.max(0) - V.min(0)
    order = np.argsort(ext)               # thin .. long
    thin, mid, long_ = ext[order[0]], ext[order[1]], ext[order[2]]
    C = V - V.mean(0)
    d = np.linalg.norm(C[:, [order[1], order[2]]], axis=1)
    roundness = float(d.std() / max(1e-6, d.mean()))     # low = circular
    flat = float(thin / max(1e-6, long_))                # low = plate-like
    elong = float(long_ / max(1e-6, mid))                # high = beam-like
    rel = None
    if model_bounds is not None:
        lo, hi = model_bounds
        span = np.maximum(hi - lo, 1e-6)
        rel = (V.mean(0) - lo) / span                    # 0..1 within the model
    return {"ext": ext, "axis": int(order[2]), "round": roundness, "flat": flat,
            "elong": elong, "rel": rel, "tris": len(F), "repeat": repeat}

def name_part(d):
    """Semantic name from the features. Deliberately conservative: falls back to
    'block' rather than inventing a specific label it cannot justify."""
    r, flat, elong, rep = d["round"], d["flat"], d["elong"], d["repeat"]
    y = float(d["rel"][1]) if d["rel"] is not None else 0.5
    circular = r < 0.35
    if circular and flat < 0.55:
        if rep >= 4: return "wheel"          # several identical discs = running gear
        if y < 0.4:  return "roadwheel"
        return "disc"
    if elong > 3.5 and flat < 0.5:
        return "barrel" if y > 0.55 else "strut"
    if flat < 0.12:
        return "panel"
    if circular and elong > 2.0:
        return "cylinder"
    if rep >= 6:
        return "tread_link"                  # many identical small pieces low down
    if y > 0.75: return "turret_greeble"
    if y < 0.25: return "underside_greeble"
    return "block"

def residual(Va, Vb, bins=24):
    """How far instance Vb deviates from family exemplar Va, after normalising
    position/scale (0 = identical family member, higher = more unique detail).
    Uses the same rotation/mirror-invariant radial profile the clustering uses,
    so it measures exactly what the family signature throws away."""
    import numpy as np
    def prof(V):
        C = V - V.mean(0); dd = np.linalg.norm(C, axis=1)
        s = dd.max() or 1.0
        return np.quantile(dd / s, np.linspace(0, 1, bins))
    pa, pb = prof(Va), prof(Vb)
    return float(np.abs(pa - pb).mean())

def signature(V, bins=24, tol=0.08):
    """Rotation/mirror/translation-invariant shape signature: quantiles of the
    radial distance distribution about the centroid, scale-normalised, then
    quantised to a step of `tol`.

    tol is a CONTINUOUS dial on how alike two parts must be to count as the same
    type: ~0.01 = near-identical, ~0.08 = same part with variations (a family),
    ~0.25 = same rough silhouette. (The old integer-decimal rounding had no usable
    setting between 'identical' and 'everything merges'.)"""
    import numpy as np
    C = V - V.mean(0)
    d = np.linalg.norm(C, axis=1)
    s = d.max() or 1.0
    q = np.quantile(d / s, np.linspace(0, 1, bins))
    step = max(1e-4, float(tol))
    qz = np.round(q / step) * step
    return hashlib.sha1(np.round(qz, 6).tobytes()).hexdigest()[:16]

def load_model(path):
    """(V, F) via the shared loaders."""
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
        spec = importlib.util.spec_from_file_location(best, os.path.join(here, best + ".py"))
        mod = importlib.util.module_from_spec(spec); sys.modules[best] = mod
        spec.loader.exec_module(mod); return mod
    pm = imp("process_models")
    if pm is None: return None
    try: return pm.load_geometry(path)
    except Exception: return None

def _harvest_one(job):
    """Picklable worker: split one model, keep only components above min_tris.
    Returns (name, error, total_components_estimate, [(V, F, model_bounds), ...])."""
    import numpy as np
    name, path, min_tris = job
    geo = load_model(path)
    if not geo:
        return name, "no geometry", 0, []
    V, F = geo
    try:
        V = np.asarray(V, "f4"); F = np.asarray(F, "i8")
        mb = (V.min(0), V.max(0))
        kept = [(cv, cf, mb) for cv, cf in components(V, F, min_tris=min_tris)]
        # cheap total-component count for reporting, without building them
        try:
            from scipy.sparse import coo_matrix
            from scipy.sparse.csgraph import connected_components as _cc
            n = len(V)
            rows = np.concatenate([F[:, 0], F[:, 1], F[:, 2]])
            cols = np.concatenate([F[:, 1], F[:, 2], F[:, 0]])
            ncomp, _l = _cc(coo_matrix((np.ones(len(rows), np.int8), (rows, cols)),
                                       shape=(n, n)), directed=False)
        except Exception:
            ncomp = len(kept)
        return name, None, ncomp, kept
    except Exception as e:
        return name, f"{type(e).__name__}", 0, []

def box_uvs(V, F, scale=1.0):
    """Box-projection UVs. Emitted parts have no authored unwrap, so without this
    any texture collapses to one texel. Each face projects onto the plane of its
    dominant normal axis; vertices are duplicated per face so seams are clean."""
    import numpy as np
    V = np.asarray(V, "f4"); F = np.asarray(F, "u4")
    tri = V[F]
    n = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
    ax = np.argmax(np.abs(n), axis=1)                  # dominant axis per face
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    Vn = (V - lo) / span                               # 0..1 in model space
    # per-face vertex duplication keeps projections independent across seams
    corner = Vn[F]                                     # (nf, 3, 3)
    uv = np.zeros((len(F), 3, 2), "f4")
    for a, (u, v) in enumerate(((1, 2), (0, 2), (0, 1))):   # x->yz, y->xz, z->xy
        m = ax == a
        if m.any():
            uv[m, :, 0] = corner[m, :, u]
            uv[m, :, 1] = corner[m, :, v]
    Vo = tri.reshape(-1, 3).astype("f4")
    UVo = (uv.reshape(-1, 2) * scale).astype("f4")
    Fo = np.arange(len(Vo), dtype="u4").reshape(-1, 3)
    return Vo, UVo, Fo

def write_part_obj(path, V, F):
    """Portable exemplar mesh (no material, no UVs -- this is a shape catalog)."""
    lines = ["# part exemplar emitted by part_harvest.py"]
    lines += ["v %.6f %.6f %.6f" % tuple(p) for p in V]
    lines += ["f %d %d %d" % (a + 1, b + 1, c + 1) for a, b, c in F]
    open(path, "w").write("\n".join(lines) + "\n")

def write_part_iqm(path, V, F):
    """Engine-ready exemplar via the shared IQM writer, if available."""
    try:
        import numpy as np
        here = os.path.dirname(os.path.abspath(__file__))
        if here not in sys.path: sys.path.insert(0, here)
        import importlib.util, glob, re
        best, bv = None, -1
        for c in glob.glob(os.path.join(here, "process_models*.py")):
            b = os.path.basename(c)[:-3]
            m = re.search(r"_v(\d+)$", b); v = int(m.group(1)) if m else 0
            if v > bv: bv, best = v, b
        if not best: return False
        spec = importlib.util.spec_from_file_location(best, os.path.join(here, best + ".py"))
        mod = importlib.util.module_from_spec(spec); sys.modules[best] = mod
        spec.loader.exec_module(mod)
        # box-project UVs first (emitted parts have no authored unwrap)
        Vu, UV, Fu = box_uvs(V, F)
        # normalise into the engine's unit slot so parts are comparable
        c = (Vu.min(0) + Vu.max(0)) * 0.5
        sc = float((Vu.max(0) - Vu.min(0)).max()) or 1.0
        Vn = ((Vu - c) / sc * 2.0).astype("f4")
        # prefer the UV-aware writer from texture_export; fall back to the plain one
        te = None
        for cand in sorted(glob.glob(os.path.join(here, "texture_export*.py")), reverse=True):
            b = os.path.basename(cand)[:-3]
            try:
                sp = importlib.util.spec_from_file_location(b, cand)
                tm = importlib.util.module_from_spec(sp); sys.modules[b] = tm
                sp.loader.exec_module(tm); te = tm; break
            except Exception: continue
        if te is not None and hasattr(te, "write_iqm"):
            te.write_iqm(path, Vn, UV, Fu.astype("u4"), "plate_blue.png")
        else:
            mod._write_iqm(path, Vn, Fu.astype("u4"))
        return True
    except Exception:
        return False

def placement_rules(lib, named, geom):
    """Where each kind of part actually sits, learned from the placements.
    These are the rules a generator needs: wheels low, turret greebles high, etc."""
    import numpy as np
    from collections import defaultdict
    by = defaultdict(list)
    for sg, insts in geom.items():
        nm = named.get(sg, {}).get("name", "block")
        for (_m, V, _F, mb) in insts:
            lo, hi = mb
            span = np.maximum(hi - lo, 1e-6)
            rel = (V.mean(0) - lo) / span
            by[nm].append(rel)
    out = {}
    for nm, rels in by.items():
        R = np.array(rels)
        out[nm] = {"n": len(R),
                   "x": [float(np.percentile(R[:, 0], 25)), float(np.percentile(R[:, 0], 75))],
                   "y": [float(np.percentile(R[:, 1], 25)), float(np.percentile(R[:, 1], 75))],
                   "z": [float(np.percentile(R[:, 2], 25)), float(np.percentile(R[:, 2], 75))],
                   "y_median": float(np.median(R[:, 1]))}
    return out

def find_models(root):
    out = []
    for dp, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in {"parts", "iqm", "build", "partlib"}]
        gl = [f for f in files if f.lower().endswith((".gltf", ".glb"))]
        ob = [f for f in files if f.lower().endswith(".obj")]
        cand = gl or ob
        if not cand: continue
        def sz(f):
            try: return os.path.getsize(os.path.join(dp, f))
            except OSError: return 0
        out.append((os.path.basename(dp), os.path.join(dp, max(cand, key=sz))))
    return out

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--out", default=None, help="write the part library here")
    ap.add_argument("--min-tris", type=int, default=120, help="ignore slivers below this")
    ap.add_argument("--tol", type=float, default=0.08,
                    help="shape-match tolerance (continuous): 0.01=near-identical, "
                         "0.08=same part with variations / FAMILY (default), 0.25=same rough "
                         "silhouette. Exact cross-model reuse measures ~0, so family level is "
                         "the useful granularity for a variation-based generator.")
    ap.add_argument("--sweep", action="store_true",
                    help="report reuse across several tolerances instead of one")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--emit-per-category", type=int, default=0, metavar="N",
                    help="emit up to N exemplars of EACH category instead of the global "
                         "top-N. Ranking by placement count alone yields almost only "
                         "wheels and tread_links; a generator needs blocks, discs and "
                         "panels too.")
    ap.add_argument("--emit", type=int, default=0, metavar="N",
                    help="also write the exemplar MESH for the top N part types "
                         "(by placement count) as .iqm + .obj -- this is what turns "
                         "the index into a usable catalog. 0 = metadata only.")
    ap.add_argument("-j", "--jobs", type=int, default=0,
                    help="parallel workers (default: all cores; 1 = serial)")
    a = ap.parse_args()

    if a.jobs <= 0: a.jobs = max(1, os.cpu_count() or 4)
    models = find_models(a.root)
    if a.limit: models = models[:a.limit]
    if not models: print("no models found"); sys.exit(1)
    print(f"{len(models)} model(s)\n")

    lib = defaultdict(list)     # signature -> [(model, comp_idx, ntris, centroid, scale)]
    per_model = {}; raw_comps = {}; geom = {}
    jobs = [(name, path, a.min_tris) for name, path in models]
    results = []
    if a.jobs > 1:
        try:
            from concurrent.futures import ProcessPoolExecutor, as_completed
            with ProcessPoolExecutor(max_workers=a.jobs) as ex:
                futs = {ex.submit(_harvest_one, j): j[0] for j in jobs}
                for fut in as_completed(futs):
                    results.append(fut.result())
        except Exception as e:
            print(f"  (parallel pool unavailable: {type(e).__name__}; serial)")
            results = [_harvest_one(j) for j in jobs]
    else:
        results = [_harvest_one(j) for j in jobs]

    for name, err, ncomp, kept in results:
        if err:
            print(f"  {name[:44]:44s} FAIL: {err}"); continue
        raw_comps[name] = [cv for cv, _cf, _mb in kept]
        sigs = []
        for ci, (cv, cf, mb) in enumerate(kept):
            sg = signature(cv, tol=a.tol)
            lib[sg].append((name, ci, len(cf), cv.mean(0),
                            float(np.linalg.norm(cv.max(0)-cv.min(0)))))
            geom.setdefault(sg, []).append((name, cv, cf, mb))
            sigs.append(sg)
        per_model[name] = sigs
        print(f"  {name[:44]:44s} {ncomp:6d} comps, {len(kept):4d} kept, "
              f"{len(set(sigs)):4d} unique types")

    if a.sweep:
        print("\n  tolerance sweep (pick where families form without over-merging):")
        print("    %8s %8s %8s %10s" % ("tol", "types", "shared", "shared%"))
        allcomps = [(m, cv) for m, cvs in raw_comps.items() for cv in cvs]
        for t in (0.01, 0.03, 0.05, 0.08, 0.12, 0.20, 0.30):
            tmp = defaultdict(set)
            for m, cv in allcomps: tmp[signature(cv, tol=t)].add(m)
            sh = sum(1 for _s, ms in tmp.items() if len(ms) > 1)
            print("    %8.2f %8d %8d %9.0f%%" % (t, len(tmp), sh, 100.0*sh/max(1, len(tmp))))
        return

    total_comps = sum(len(v) for v in lib.values())
    print(f"\n{total_comps} components -> {len(lib)} unique PART TYPES")

    # within vs across model reuse
    across = {s: {m for m, _c, _t, _q, _z in v} for s, v in lib.items()}
    shared = {s: ms for s, ms in across.items() if len(ms) > 1}
    reused_in_model = sum(1 for s, v in lib.items() if len(v) > len(across[s]))
    print(f"  part types reused WITHIN a model : {reused_in_model}")
    print(f"  part types shared ACROSS models  : {len(shared)}")
    if shared:
        print("\n  most widely shared parts:")
        for s, ms in sorted(shared.items(), key=lambda kv: -len(kv[1]))[:10]:
            inst = len(lib[s]); tris = lib[s][0][2]
            print(f"    {len(ms):2d} models, {inst:3d} placements, ~{tris:6d} tris   sig {s[:10]}")

    # ---- name each family and measure how much its members vary
    named = {}
    for sg, insts in geom.items():
        m0, V0, F0, mb0 = insts[0]
        d = describe(V0, F0, mb0, repeat=len(insts))
        nm = name_part(d)
        res = [residual(V0, Vi) for (_m, Vi, _f, _b) in insts[1:]]
        named[sg] = {"name": nm, "n": len(insts), "tris": int(d["tris"]),
                     "res_mean": float(np.mean(res)) if res else 0.0,
                     "res_max": float(np.max(res)) if res else 0.0}
    from collections import Counter
    cnt = Counter(v["name"] for v in named.values())
    print("\n  part types by NAME:")
    for nm, c in cnt.most_common():
        fam = [v for v in named.values() if v["name"] == nm]
        inst = sum(v["n"] for v in fam)
        rm = np.mean([v["res_mean"] for v in fam])
        print(f"    {nm:18s} {c:4d} types, {inst:5d} placements, mean residual {rm:.3f}")
    varied = [v for v in named.values() if v["res_mean"] > 0.02]
    print(f"\n  families whose members genuinely differ (residual > 0.02): "
          f"{len(varied)}/{len(named)}")
    print("    -> those need a per-instance residual stored; the rest are true repeats.")

    tri_total = sum(t for v in lib.values() for _m, _c, t, _q, _z in v)
    tri_uniq  = sum(v[0][2] for v in lib.values())
    print(f"\n  triangles across all placements : {tri_total:,}")
    print(f"  triangles if stored once each   : {tri_uniq:,}  "
          f"({100*(1-tri_uniq/max(1,tri_total)):.0f}% is repeated geometry)")

    if a.out:
        os.makedirs(a.out, exist_ok=True)
        rules = placement_rules(lib, named, geom)
        print("\n  PLACEMENT RULES learned from the data (relative position in model, 25-75%):")
        print("    %-18s %5s %12s %12s" % ("name", "n", "height y", "along z"))
        for nm, r in sorted(rules.items(), key=lambda kv: kv[1]["y_median"]):
            print("    %-18s %5d  %.2f-%.2f    %.2f-%.2f" %
                  (nm, r["n"], r["y"][0], r["y"][1], r["z"][0], r["z"][1]))

        emitted = {}
        if a.emit or a.emit_per_category:
            mesh_dir = os.path.join(a.out, "meshes"); os.makedirs(mesh_dir, exist_ok=True)
            ordered = sorted(lib.items(), key=lambda kv: -len(kv[1]))
            if a.emit_per_category:
                percat, ranked = {}, []
                for sg, insts in ordered:
                    nm = named.get(sg, {}).get("name", "block")
                    if percat.get(nm, 0) >= a.emit_per_category: continue
                    percat[nm] = percat.get(nm, 0) + 1
                    ranked.append((sg, insts))
                print("  emitting per category:", dict(sorted(percat.items())))
            else:
                ranked = ordered[:a.emit]
            n_iqm = n_obj = 0
            for sg, _insts in ranked:
                g0 = geom.get(sg)
                if not g0: continue
                _m, V0, F0, _mb = g0[0]
                nm = named.get(sg, {}).get("name", "block")
                stem = f"part_{nm}_{sg[:8]}"
                try:
                    write_part_obj(os.path.join(mesh_dir, stem + ".obj"), V0, F0); n_obj += 1
                except Exception: pass
                if write_part_iqm(os.path.join(mesh_dir, stem + ".iqm"), V0, F0): n_iqm += 1
                emitted[sg] = stem
            print(f"\n  emitted {n_obj} .obj and {n_iqm} .iqm exemplars to {mesh_dir}")

        index = {"parts": [], "placements": defaultdict(list), "rules": rules}
        for s, v in lib.items():
            m0, c0, t0, q0, z0 = v[0]
            info = named.get(s, {})
            index["parts"].append({"sig": s, "tris": int(t0), "size": z0, "from": m0,
                                   "name": info.get("name", "block"),
                                   "instances": info.get("n", 1),
                                   "residual_mean": round(info.get("res_mean", 0.0), 4),
                                   "residual_max": round(info.get("res_max", 0.0), 4),
                                   "mesh": emitted.get(s)})
            for (m, ci, t, q, z) in v:
                index["placements"][m].append({"sig": s, "centroid": [float(x) for x in q],
                                               "size": z})
        index["placements"] = dict(index["placements"])
        json.dump(index, open(os.path.join(a.out, "partlib.json"), "w"), indent=1)
        print(f"\nwrote {os.path.join(a.out,'partlib.json')} "
              f"({len(index['parts'])} part types, {total_comps} placements)")

if __name__ == "__main__":
    main()
