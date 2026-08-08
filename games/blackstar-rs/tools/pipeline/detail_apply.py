#!/usr/bin/env python3
"""detail_apply.py -- add internal structure to an EXISTING bot.

The from-scratch generator produced a mannequin because assembling primitives has
no silhouette. This works the other way round: keep the real model's shell, and
build structure INSIDE it -- spine, ribs, and surface greebles drawn from a
harvested catalog.

Measured target (Scorponok, the reference for "detailed"):
    479 structural members (>=50 tris), 75% of triangles structural
For comparison Jazz has 45 members at 52%. So the job is roughly "add ~400
members", not "add polygons to the shell".

Cavity finding is geometric, not guessed: the chest band of the model is sliced,
and for each slice the interior gap between the left and right shell walls is
measured. Ribs are sized to that gap so they sit inside the body instead of
poking through it.

Usage:
  python3 tools/detail_apply.py --target MODEL.gltf --catalog roboskelcat \
      --name jazz_ribbed --out ribbed
  python3 tools/detail_apply.py --target MODEL.gltf --catalog roboskelcat \
      --ribs 14 --greebles 120 --out ribbed --seed 5

  python3 tools/install_models.py --parts-dir ribbed --engine engine_src
"""
import sys, os, json, math, random, argparse

def _load_helpers():
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
    return imp("process_models"), imp("texture_export"), imp("part_harvest")

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

def cavity_profile(V, y0, y1, slices=12):
    """For each horizontal slice of the chest band, measure the interior gap
    between the left and right shell walls, and the body's depth there.
    Returns [(y, half_width, half_depth, cx, cz), ...] -- ribs are sized to this
    so they stay inside the body."""
    import numpy as np
    out = []
    for i in range(slices):
        a = y0 + (y1 - y0) * (i + 0.5) / slices
        h = (y1 - y0) / slices * 0.7
        m = np.abs(V[:, 1] - a) < h
        if m.sum() < 12: continue
        S = V[m]
        hw = float(np.percentile(np.abs(S[:, 0] - S[:, 0].mean()), 85))
        hd = float(np.percentile(np.abs(S[:, 2] - S[:, 2].mean()), 85))
        out.append((a, hw, hd, float(S[:, 0].mean()), float(S[:, 2].mean())))
    return out

def gut_shell(V, F, y0, y1, front_only=True, thresh=0.35):
    """Remove the shell panel covering the chest cavity.

    Keeping the shell 100% intact means anything built inside it is INVISIBLE --
    ribs behind an opaque chest just look like the original model with debris
    around it. Cut the outward-facing faces over the cavity so the interior reads.
    Returns (F_kept, n_removed)."""
    import numpy as np
    tri = V[F]
    c = tri.mean(1)
    n = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
    ln = np.linalg.norm(n, axis=1, keepdims=True); ln[ln == 0] = 1; n = n / ln
    band = (c[:, 1] > y0) & (c[:, 1] < y1)
    if front_only:
        cut = band & (n[:, 2] > thresh) & (c[:, 2] > 0)      # front panel only
    else:
        cut = band & (np.abs(n[:, 2]) > thresh)              # front and back
    return F[~cut], int(cut.sum())

def fit_part(V, cx, cy, cz, sx, sy, sz, rot_z=0.0):
    """Uniformly scale a part into a box and place it (never distort a part)."""
    import numpy as np
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    k = float((np.array([sx, sy, sz], "f4") / span).min())
    P = (V - (lo + hi) * 0.5) * k
    if rot_z:
        c, s = math.cos(rot_z), math.sin(rot_z)
        x, y = P[:, 0].copy(), P[:, 1].copy()
        P[:, 0] = x * c - y * s; P[:, 1] = x * s + y * c
    return (P + np.array([cx, cy, cz], "f4")).astype("f4")

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True, help="the bot to add structure to (.gltf/.glb/.obj)")
    ap.add_argument("--catalog", required=True, help="a part_harvest --out dir with meshes/")
    ap.add_argument("--name", default=None)
    ap.add_argument("--out", default="detailed")
    ap.add_argument("--ribs", type=int, default=12, help="rib PAIRS in the chest cavity")
    ap.add_argument("--greebles", type=int, default=0,
                    help="surface detail pieces. Default 0: unoriented scatter reads "
                         "as floating debris, so turn it on deliberately once the "
                         "cavity work looks right.")
    ap.add_argument("--chest", default="0.45,0.80", help="chest band as y-fractions of the body")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--texture", default="plate_blue.png")
    ap.add_argument("--keep-shell", action="store_true", default=True)
    ap.add_argument("--gut", choices=("none", "front", "both"), default="front",
                    help="cut the shell panel over the chest cavity so the ribs are "
                         "VISIBLE. 'front' opens the chest, 'both' opens front and "
                         "back, 'none' leaves the model closed (ribs hidden).")
    a = ap.parse_args()

    pm, te, ph = _load_helpers()
    if not (pm and te and ph):
        raise SystemExit("need process_models.py, texture_export.py and part_harvest.py beside this tool")

    # Fail helpfully: an empty shell variable turns "$X/scene.gltf" into
    # "/scene.gltf", which otherwise surfaces as a traceback from deep in the loader.
    if not os.path.isfile(a.target):
        print("ERROR: target model not found: %r" % a.target)
        if a.target.startswith("/scene.") or a.target.startswith("/"):
            print("  That looks like an empty shell variable -- the find returned nothing.")
        d = os.path.dirname(a.target) or "."
        print("\n  Find the model first, then pass the file it prints.")
        print("  Use -ipath (matches the whole path) -- the model file is usually")
        print("  called scene.gltf, so -iname on the character name finds nothing:")
        print("      find ~ -ipath '*jazz*' \\( -name '*.gltf' -o -name '*.glb' -o -name '*.obj' \\)")
        if os.path.isdir(d):
            near = [f for f in os.listdir(d) if f.lower().endswith((".gltf", ".glb", ".obj"))]
            if near: print("\n  Model files beside that path: %s" % ", ".join(near[:6]))
        sys.exit(2)

    name = a.name or os.path.basename(os.path.dirname(os.path.abspath(a.target))) or "detailed"
    rng = random.Random(a.seed)

    geo = pm.load_geometry(a.target)
    if not geo: raise SystemExit("could not read " + a.target)
    V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
    lo, hi = V.min(0), V.max(0); ctr = (lo + hi) * 0.5; span = float((hi - lo).max()) or 1.0
    Vn = ((V - ctr) / span * 2.0).astype("f4")        # engine unit slot
    nlo, nhi = Vn.min(0), Vn.max(0)

    before = len([1 for _cv, cf in ph.components(Vn, F.astype(np.int64), min_tris=50)])
    print(f"target: {name}  {len(F):,} tris, {before} structural members (>=50 tris)")
    print(f"        Scorponok reference: 479 members, 75% structural\n")

    # catalog
    idx = json.load(open(os.path.join(a.catalog, "partlib.json")))
    mesh_dir = os.path.join(a.catalog, "meshes")
    cat = [p for p in idx["parts"] if p.get("mesh") and
           os.path.exists(os.path.join(mesh_dir, p["mesh"] + ".obj"))]
    by = {}
    for p in cat: by.setdefault(p["name"], []).append(p)
    if not cat: raise SystemExit("catalog has no emitted meshes (re-run part_harvest --emit-per-category)")
    print(f"catalog: {len(cat)} exemplars, categories {sorted(by)}\n")

    def pick(*cats):
        for c in cats:
            if by.get(c): return rng.choice(by[c])
        return rng.choice(cat)

    y0 = nlo[1] + (nhi[1] - nlo[1]) * float(a.chest.split(",")[0])
    y1 = nlo[1] + (nhi[1] - nlo[1]) * float(a.chest.split(",")[1])
    prof = cavity_profile(Vn, y0, y1, slices=max(6, a.ribs))
    if not prof: raise SystemExit("could not find a chest cavity in that band -- try --chest")
    print(f"chest cavity: {len(prof)} slices, half-width {min(p[1] for p in prof):.2f}"
          f"..{max(p[1] for p in prof):.2f}")

    os.makedirs(a.out, exist_ok=True)
    parts = []
    buckets = {}      # bucket -> [(V, F), ...]; merged at the end
    added = 0

    def emit(Vp, Fp, bucket="detail"):
        """Collect into a bucket. Hundreds of separate models would blow the
        engine's per-model part limit and cost a draw call each; everything here
        shares one texture, so batching is both necessary and free."""
        nonlocal added
        buckets.setdefault(bucket, []).append((np.asarray(Vp, "f4"), np.asarray(Fp, "u4")))
        added += 1

    def flush():
        """Merge each bucket into a single mesh and write it."""
        n_out = 0
        for bname, items in buckets.items():
            if not items: continue
            # split a bucket if it would exceed a sane vertex budget per mesh
            chunks, cur, cur_v = [], [], 0
            for (Vp, Fp) in items:
                if cur and cur_v + len(Vp) > 60000:
                    chunks.append(cur); cur, cur_v = [], 0
                cur.append((Vp, Fp)); cur_v += len(Vp)
            if cur: chunks.append(cur)
            for ci, chunk in enumerate(chunks):
                Vs, Fs, off = [], [], 0
                for (Vp, Fp) in chunk:
                    Vs.append(Vp); Fs.append(Fp + off); off += len(Vp)
                Vm = np.vstack(Vs).astype("f4"); Fm = np.vstack(Fs).astype("u4")
                Vu, UV, Fu = ph.box_uvs(Vm, Fm) if hasattr(ph, "box_uvs") \
                             else (Vm, np.zeros((len(Vm), 2), "f4"), Fm)
                out = os.path.join(a.out, "imported_%s_part%d.iqm" % (name, n_out))
                te.write_iqm(out, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), a.texture)
                parts.append("%s %s %s %d %d %.5f %.5f %.5f"
                             % (os.path.basename(out), a.texture, "body", -1, 0, 0.0, 0.0, 0.0))
                n_out += 1
        return n_out

    # 1. shell -- gutted over the cavity so the structure inside is actually visible
    Fshell = F.astype("u4")
    if a.gut:
        Fshell, n_cut = gut_shell(Vn, Fshell, y0, y1, front_only=(a.gut == "front"))
        print(f"gutted chest: removed {n_cut:,} of {len(F):,} shell faces "
              f"({100.0*n_cut/max(1,len(F)):.0f}%) -- cavity is now open")
    if a.keep_shell:
        emit(Vn, Fshell, "shell")

    # 2. spine: a column of small parts up the cavity centre
    spine_n = max(6, a.ribs)
    for i in range(spine_n):
        t = (i + 0.5) / spine_n
        y = y0 + (y1 - y0) * t
        s = prof[min(len(prof) - 1, int(t * len(prof)))]
        p = pick("block", "disc", "turret_greeble")
        Vp, Fp = read_obj(os.path.join(mesh_dir, p["mesh"] + ".obj"))
        seg = (y1 - y0) / spine_n * 0.85
        emit(fit_part(Vp, s[3], y, s[4], seg * 0.9, seg, seg * 0.9), Fp, "frame")

    # 3. ribs: mirrored pairs curving out from the spine, sized to the cavity
    for i in range(a.ribs):
        t = (i + 0.5) / a.ribs
        s = prof[min(len(prof) - 1, int(t * len(prof)))]
        y = s[0]
        curve = math.sin(t * math.pi) * 0.55 + 0.45      # widest mid-chest
        for side in (-1, 1):
            p = pick("strut", "tread_link", "block")
            Vp, Fp = read_obj(os.path.join(mesh_dir, p["mesh"] + ".obj"))
            rx = s[3] + side * s[1] * 0.55 * curve
            emit(fit_part(Vp, rx, y, s[4],
                          s[1] * 0.75 * curve, (y1 - y0) / a.ribs * 0.7, s[2] * 1.25,
                          rot_z=side * 0.28), Fp, "frame")

    # 4. surface greebles, placed by the catalog's own learned height rules
    rules = idx.get("rules", {})
    for i in range(a.greebles):
        nm = rng.choice([k for k in by] or ["block"])
        r = rules.get(nm)
        fy = rng.uniform(*(r["y"] if r else (0.1, 0.9)))
        y = nlo[1] + (nhi[1] - nlo[1]) * fy
        m = np.abs(Vn[:, 1] - y) < (nhi[1] - nlo[1]) * 0.06
        if m.sum() < 8: continue
        S = Vn[m]
        j = rng.randrange(len(S))
        anchor = S[j]
        p = rng.choice(by[nm])
        Vp, Fp = read_obj(os.path.join(mesh_dir, p["mesh"] + ".obj"))
        sz = (nhi - nlo).max() * rng.uniform(0.025, 0.06)
        # pull slightly toward the body centre so pieces sit ON the shell, not off it
        pos = anchor * 0.94
        emit(fit_part(Vp, pos[0], pos[1], pos[2], sz, sz, sz), Fp, "body")

    n_meshes = flush()
    man = os.path.join(a.out, "imported_%s_parts.txt" % name)
    open(man, "w").write("\n".join(parts))
    print(f"\nplaced {added} pieces, batched into {n_meshes} mesh(es) -> {man}")
    print(f"  shell 1 + spine {spine_n} + ribs {a.ribs*2} + greebles {added-1-spine_n-a.ribs*2}")
    print(f"  structural members: {before} -> ~{before + added - 1}  (target 479)")
    print(f"\ninstall:\n  python3 tools/install_models.py --parts-dir {a.out} --engine engine_src")

if __name__ == "__main__":
    main()
