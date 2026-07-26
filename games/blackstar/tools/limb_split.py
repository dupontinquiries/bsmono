#!/usr/bin/env python3
"""limb_split.py -- cut a bot into head / torso / arms / legs for animation.

Everything so far animates the WHOLE body, because the model is one mesh (or a set
of material groups that ignore left/right). A walk cycle needs the left leg to go
forward while the right goes back, so the mesh has to be segmented first.

Segmentation is by CONNECTED COMPONENT, not by cutting triangles: each component is
assigned to a limb from where its centroid sits, so parts stay whole and nothing is
torn in half. Components straddling the centre line stay with the torso.

Emitted manifest carries a limb tag per part, which the engine uses to drive the
walk cycle (atag 5..9 = arm_l, arm_r, leg_l, leg_r, head).

Usage:
  python3 tools/limb_split.py --target MODEL.gltf --name jazz --out limbs
  python3 tools/install_models.py --parts-dir limbs --engine engine_src
"""
import sys, os, re, json, math, argparse

# manifest atag codes the engine understands
LIMB_CODE = {"body": 0, "tire": 1, "fan": 2, "door": 3, "armor": 4,
             "arm_l": 5, "arm_r": 6, "leg_l": 7, "leg_r": 8, "head": 9,
             # lower segments -- elbow/knee. Each is a CHILD of its upper segment,
             # so the engine composes its transform from the parent.
             "arm_l_lo": 10, "arm_r_lo": 11, "leg_l_lo": 12, "leg_r_lo": 13,
             # feet: a third segment that counter-rotates to stay flat to the ground
             "leg_l_ft": 14, "leg_r_ft": 15}
UPPER_OF = {"arm_l_lo": "arm_l", "arm_r_lo": "arm_r",
            "leg_l_lo": "leg_l", "leg_r_lo": "leg_r",
            "leg_l_ft": "leg_l_lo", "leg_r_ft": "leg_r_lo"}

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

def components_uv(V, UV, F, ph, min_tris=20):
    """Connected components that carry their UVs through.

    ph.components() remaps indices and drops everything except positions, so using
    it alone is what forced every limb to a placeholder texture."""
    import numpy as np
    V = np.asarray(V, "f4"); F = np.asarray(F, np.int64)
    key = np.round(V / 1e-5).astype(np.int64)
    _u, inv = np.unique(key, axis=0, return_inverse=True)
    try:
        from scipy.sparse import coo_matrix
        from scipy.sparse.csgraph import connected_components as _cc
        n = len(_u)
        Fw = inv[F]
        rows = np.concatenate([Fw[:, 0], Fw[:, 1], Fw[:, 2]])
        cols = np.concatenate([Fw[:, 1], Fw[:, 2], Fw[:, 0]])
        ncomp, lab = _cc(coo_matrix((np.ones(len(rows), np.int8), (rows, cols)),
                                    shape=(n, n)), directed=False)
    except Exception:
        return [(cv, np.zeros((len(cv), 2), "f4"), cf)
                for cv, cf in ph.components(V, F, min_tris=min_tris)]
    flab = lab[inv[F[:, 0]]]
    out = []
    for c in range(ncomp):
        fm = flab == c
        if fm.sum() < min_tris: continue
        Fc = F[fm]
        vids = np.unique(Fc)
        remap = -np.ones(len(V), np.int64); remap[vids] = np.arange(len(vids))
        out.append((V[vids], UV[vids] if len(UV) == len(V) else
                    np.zeros((len(vids), 2), "f4"), remap[Fc].astype("u4")))
    return out

def mirror_axis(V, samples=1200, seed=0):
    """Which axis is the body's left/right?

    Not always X. Models are authored facing different ways -- Zeta Prime mirrors
    about Z -- and assuming X put its left limbs on the right and produced limbs at
    different heights. Measure it: mirror the cloud about each candidate axis and
    see which reconstructs the model better."""
    import numpy as np
    rng = np.random.default_rng(seed)
    idx = rng.choice(len(V), min(samples, len(V)), replace=False)
    best, best_err = 0, None
    for ax in (0, 2):
        M = V.copy(); M[:, ax] *= -1
        err = float(np.mean([np.min(np.sum((V - q) ** 2, axis=1)) for q in M[idx]]))
        if best_err is None or err < best_err: best, best_err = ax, err
    return best

def classify(centroid, lo, hi, torso_hw, ax=0):
    """Which limb does a component belong to, from where its centroid sits."""
    fy = (centroid[1] - lo[1]) / max(1e-6, hi[1] - lo[1])     # 0 feet .. 1 head
    x  = centroid[ax] - (lo[ax] + hi[ax]) * 0.5               # ax = the mirror axis
    outboard = abs(x) > torso_hw
    if fy > 0.86:                      return "head"
    if fy < 0.46:                      return "leg_l" if x < 0 else "leg_r"
    if outboard:                       return "arm_l" if x < 0 else "arm_r"
    return "body"                      # torso: stays put, everything hangs off it

def find_models(root):
    """Every model under root, with names disambiguated by parent directory.
    Truncating names by hand collides silently -- war_for_cybertron_arcee and
    war_for_cybertron_autobot_soldier both shorten to the same thing, and one
    overwrites the other."""
    found = []
    for dp, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in {"parts", "iqm", "build", "limbs"}]
        gl = [f for f in files if f.lower().endswith((".gltf", ".glb"))]
        ob = [f for f in files if f.lower().endswith(".obj")]
        cand = gl or ob
        if not cand: continue
        def sz(f):
            try: return os.path.getsize(os.path.join(dp, f))
            except OSError: return 0
        base = os.path.basename(dp.rstrip("/")) or "model"
        parent = os.path.basename(os.path.dirname(dp.rstrip("/")))
        found.append([base, parent, os.path.join(dp, max(cand, key=sz))])
    from collections import Counter
    dupes = {n for n, c in Counter(b for b, _p, _g in found).items() if c > 1}
    for rec in found:
        if rec[0] in dupes and rec[1]:
            rec[0] = f"{rec[1]}_{rec[0]}"
    # guarantee uniqueness even after that
    seen = {}
    out = []
    for base, _p, g in found:
        nm = re.sub(r"[^a-z0-9]", "", base.lower())[:26] or "model"
        if nm in seen:
            seen[nm] += 1; nm = f"{nm}{seen[nm]}"
        else:
            seen[nm] = 0
        out.append((nm, g))
    return out

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", default=None)
    ap.add_argument("--batch", default=None,
                    help="split every model under this directory, with names made "
                         "unique automatically (safer than truncating by hand)")
    ap.add_argument("--skip-vehicles", action="store_true", default=True,
                    help="skip models whose name/path says vehicle -- limb splitting "
                         "a car produces meaningless 'legs'")
    ap.add_argument("--name", default=None)
    ap.add_argument("--out", default="limbs")
    ap.add_argument("--min-tris", type=int, default=20)
    ap.add_argument("--max-gap", type=float, default=0.12,
                    help="reject the split if a limb sits further than this from the "
                         "body (model normalised to 2 units). Prevents shipping a rig "
                         "whose limbs visibly detach when animated.")
    ap.add_argument("--torso-frac", type=float, default=0.28,
                    help="torso half-width as a fraction of body half-width; "
                         "components beyond it at chest height are arms")
    ap.add_argument("--feet", action="store_true", default=True,
                    help="split a foot off each shin so it can stay flat to the ground")
    ap.add_argument("--joints", action="store_true", default=True,
                    help="split arms/legs into upper+lower so knees and elbows bend")
    ap.add_argument("--detail", default=None,
                    help="a part_harvest catalog; adds rib banding to the TORSO while "
                         "keeping the limb hierarchy, so one model is both detailed "
                         "and articulated")
    ap.add_argument("--ribs", type=int, default=12)
    ap.add_argument("--rib-stretch", type=float, default=5.0)
    ap.add_argument("--rib-segments", type=int, default=18)
    ap.add_argument("--greebles", type=int, default=0,
                    help="surface detail pieces added alongside the rib bands")
    ap.add_argument("--greeble-size", type=float, default=0.09)
    ap.add_argument("--cap", type=int, default=1024,
                    help="texture cap when copying the source model's maps out")
    ap.add_argument("--texture", default="plate_blue.png")
    a = ap.parse_args()

    if a.batch:
        VEH = ("car", "tank", "flyer", "truck", "boat", "spaceship", "hover",
               "vehicle", "racer", "heli", "missile", "turret", "jet")
        todo = find_models(a.batch)
        print(f"{len(todo)} model(s) under {a.batch}\n")
        ok = skipped = 0
        for nm, path in todo:
            if a.skip_vehicles and any(v in path.lower() for v in VEH):
                print(f"  {nm[:34]:34s} SKIP (vehicle)"); skipped += 1; continue
            extra = ""
            if a.detail:      # batch was dropping the detail pass entirely
                extra += ' --detail "%s" --ribs %d --rib-stretch %s' % (
                    a.detail, a.ribs, a.rib_stretch)
                extra += ' --rib-segments %d --greebles %d --greeble-size %s' % (
                    a.rib_segments, a.greebles, a.greeble_size)
            r = os.system("%s %s --target %s --name %s --out %s --min-tris %d%s >/dev/null 2>&1"
                          % (sys.executable, os.path.abspath(__file__),
                             '"%s"' % path, nm + "lmb", '"%s"' % a.out, a.min_tris, extra))
            man = os.path.join(a.out, "imported_%slmb_parts.txt" % nm)
            n = len(open(man).read().split("\n")) if os.path.exists(man) else 0
            print(f"  {nm[:34]:34s} {'%d limb parts' % n if n else 'FAIL'}")
            ok += 1 if n else 0
        print(f"\n{ok} split, {skipped} vehicles skipped -> {a.out}")
        return

    if not a.target:
        print("give --target MODEL or --batch DIR"); sys.exit(1)
    if not os.path.isfile(a.target):
        print("ERROR: target not found: %r" % a.target)
        print("  find ~ -ipath '*name*' \\( -name '*.gltf' -o -name '*.glb' -o -name '*.obj' \\)")
        sys.exit(2)

    pm, te, ph = _helpers()
    if not (pm and te and ph): raise SystemExit("need process_models / texture_export / part_harvest")
    name = a.name or os.path.basename(os.path.dirname(os.path.abspath(a.target)))

    # Load BY MATERIAL so textures survive. Loading positions-only wrote every
    # limb as plate_blue, i.e. a limb-split model lost all its texturing and
    # rendered flat placeholder blue -- a large visual regression that had nothing
    # to do with the rig.
    mat_groups = None
    try:
        if not a.target.lower().endswith(".obj"):
            g0, gj, gbase = te.load_by_material(a.target)
            if g0:
                mat_groups = []
                for (mname, mi), (mv, muv, mf) in g0.items():
                    tex = None
                    try:
                        t = te.base_color_texture(gj, gbase, mi)
                        if t:
                            # copy the texture out with the model, capped, or the
                            # engine references a filename it does not have
                            os.makedirs(os.path.join(a.out, "textures"), exist_ok=True)
                            tex = "%s_%s" % (name, os.path.basename(t))
                            dst = os.path.join(a.out, "textures", tex)
                            try:
                                tex = te.convert_texture(t, dst, a.cap, "auto", 92)
                            except Exception:
                                import shutil as _sh; _sh.copy2(t, dst)
                    except Exception: pass
                    mat_groups.append((mname, tex, np.asarray(mv, "f4"),
                                       np.asarray(muv, "f4"), np.asarray(mf, "u4")))
    except Exception as e:
        print("  (per-material load failed: %s -- falling back to untextured)" % type(e).__name__)

    geo = pm.load_geometry(a.target)
    if not geo: raise SystemExit("could not read " + a.target)
    V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
    lo, hi = V.min(0), V.max(0); ctr = (lo + hi) * 0.5; span = float((hi - lo).max()) or 1.0
    Vn = ((V - ctr) / span * 2.0).astype("f4")
    nlo, nhi = Vn.min(0), Vn.max(0)
    max_ = mirror_axis(Vn)
    torso_hw = (nhi[max_] - nlo[max_]) * 0.5 * a.torso_frac
    print(f"  mirror axis: {'XYZ'[max_]}  (torso half-width {torso_hw:.2f})")

    comps = ph.components(Vn, F.astype(np.int64), min_tris=a.min_tris)
    print(f"{name}: {len(F):,} tris -> {len(comps)} components (>={a.min_tris} tris)")

    groups = {}
    tex_of = {}          # (limb, texture) -> pieces, filled AFTER the joint split
    comp_tex = {}        # component -> its source texture
    comp_uv = {}         # component -> its UVs
    if mat_groups:
        # keep each component's material so the limb renders with its real texture
        for (mname, tex, mv, muv, mf) in mat_groups:
            mvn = ((mv - ctr) / span * 2.0).astype("f4")
            for (cv, cuv, cf) in components_uv(mvn, muv, mf, ph, a.min_tris):
                comp_uv[id(cv)] = cuv
                limb = classify(cv.mean(0), nlo, nhi, torso_hw, max_)
                groups.setdefault(limb, []).append((cv, cf))
                # remember each component's texture; the limb it belongs to is not
                # final yet (the joint split reassigns some to _lo/_ft), so key on
                # the component itself and resolve after splitting.
                comp_tex[id(cv)] = tex or a.texture
    else:
        for cv, cf in comps:
            limb = classify(cv.mean(0), nlo, nhi, torso_hw, max_)
            groups.setdefault(limb, []).append((cv, cf))

    # VALIDATE the split before trusting it. Centroid classification fails badly on
    # some models -- Zeta Prime put 4,147 triangles in one arm and 72 in the other,
    # with pieces sitting a full body-width from their own pivot. Animating that
    # produces limbs flying off, which is exactly what was reported. A static but
    # correct model is better than a broken articulated one, so if the two sides
    # disagree wildly, refuse to split.
    def _bulk(l):
        return sum(len(cf) for _cv, cf in groups.get(l, []))
    bad_pairs = []
    for l, r in (("arm_l", "arm_r"), ("leg_l", "leg_r")):
        a_, b_ = _bulk(l), _bulk(r)
        if max(a_, b_) > 0 and min(a_, b_) < 0.25 * max(a_, b_):
            bad_pairs.append((l, r, a_, b_))
    # Direct check: is each limb actually ADJACENT to the one it hangs from? The
    # triangle-count test is only a proxy; this measures the thing that goes wrong.
    if not bad_pairs:
        import numpy as _np3
        def _cloud(l):
            its = groups.get(l, [])
            if not its: return None
            return _np3.vstack([cv for cv, _cf in its]).astype("f4")
        for child, parent in (("arm_l", "body"), ("arm_r", "body"),
                              ("leg_l", "body"), ("leg_r", "body")):
            C, P_ = _cloud(child), _cloud(parent)
            if C is None or P_ is None: continue
            sub = P_[::max(1, len(P_) // 2000)]
            top = C[C[:, 1] >= _np3.percentile(C[:, 1], 80)]
            if len(top) == 0: continue
            gap = float(_np3.sqrt(((top[:, None, :] - sub[None, :, :]) ** 2)
                                  .sum(-1).min(axis=1)).min())
            if gap > a.max_gap:
                bad_pairs.append((child, parent, int(gap * 1000), 0))
                print("  !! %s sits %.3f from the body -- not attached" % (child, gap))
                break

    if bad_pairs:
        for l, r, a_, b_ in bad_pairs:
            print("  !! %s/%s split %d vs %d triangles -- too asymmetric to rig" % (l, r, a_, b_))
        print("  falling back to an UNSPLIT model (static, but not broken)")
        allc = [it for its in groups.values() for it in its]
        groups = {"body": allc}

    # split each limb into upper/lower at its mid-height so knees and elbows exist
    if a.joints:
        for limb in ("arm_l", "arm_r", "leg_l", "leg_r"):
            items = groups.get(limb)
            if not items or len(items) < 2: continue
            ys = np.array([cv[:, 1].mean() for cv, _cf in items])
            mid = float(np.median(ys))
            up = [it for it, y in zip(items, ys) if y >= mid]
            lo_ = [it for it, y in zip(items, ys) if y < mid]
            if not up or not lo_: continue
            groups[limb] = up
            groups[limb + "_lo"] = lo_
        # legs get a third cut: the foot, taken as the lowest slice of the shin.
        # Without it the foot points wherever the shin does and the bot walks on
        # its toes; as its own segment it can counter-rotate and stay flat.
        if a.feet:
            for leg in ("leg_l", "leg_r"):
                shin = groups.get(leg + "_lo")
                if not shin or len(shin) < 2: continue
                ys = np.array([cv[:, 1].mean() for cv, _cf in shin])
                cut = float(np.percentile(ys, 35))
                sh = [it for it, y in zip(shin, ys) if y >= cut]
                ft = [it for it, y in zip(shin, ys) if y < cut]
                if not sh or not ft: continue
                groups[leg + "_lo"] = sh
                groups[leg + "_ft"] = ft

    os.makedirs(a.out, exist_ok=True)
    manifest = []
    order = ["body", "head", "arm_l", "arm_l_lo", "arm_r", "arm_r_lo",
             "leg_l", "leg_l_lo", "leg_l_ft", "leg_r", "leg_r_lo", "leg_r_ft"]
    # now that limbs are final (upper/lower/foot), bucket them by texture
    if comp_tex:
        tex_of = {}
        for limb, items in groups.items():
            for (cv, cf) in items:
                tx = comp_tex.get(id(cv), a.texture)
                tex_of.setdefault((limb, tx), []).append((cv, comp_uv.get(id(cv)), cf))

    present = [l for l in order if l in groups]
    index_of = {l: i for i, l in enumerate(present)}

    # Pre-compute pivots, then SYMMETRISE left/right pairs.
    # Component assignment is a heuristic and splits unevenly on models that are
    # not cleanly T-posed, which left a hip at -0.24 on one side and -0.04 on the
    # other. The mesh split can stay uneven; the RIG should not. Averaging the
    # height and mirroring the position gives a joint that articulates evenly even
    # when the geometry underneath is lopsided.
    pivots = {}
    for limb in present:
        Vm = np.vstack([cv for cv, _cf in groups[limb]]).astype("f4")
        pv = Vm.mean(0) if limb != "body" else np.zeros(3, "f4")
        if limb in ("arm_l", "arm_r", "leg_l", "leg_r") or limb.endswith(("_lo", "_ft")):
            pv[1] = Vm[:, 1].max()
        elif limb == "head":
            pv[1] = Vm[:, 1].min()
        pivots[limb] = pv
    # Reject OUTLIER components: a limb should be a compact cluster, but
    # centroid-based classification sometimes sweeps in geometry from across the
    # model, leaving pieces of a "leg" a body-length away. Measured gaps reached
    # 1.30 units on a 2-unit model. Anything far from the limb's own centre goes
    # back to the body, where it is static and harmless.
    import numpy as _np2
    for limb in [l for l in list(groups) if l != "body"]:
        items = groups[limb]
        if len(items) < 3: continue
        cents = _np2.array([cv.mean(0) for cv, _cf in items])
        med = _np2.median(cents, axis=0)
        dist = _np2.linalg.norm(cents - med, axis=1)
        thresh = max(0.35, float(_np2.percentile(dist, 75)) * 2.0)
        keep = [it for it, dd in zip(items, dist) if dd <= thresh]
        drop = [it for it, dd in zip(items, dist) if dd > thresh]
        if drop and keep:
            groups[limb] = keep
            groups.setdefault("body", []).extend(drop)

    # Snap each child's pivot onto its PARENT's surface. Taking the child's own
    # top leaves a gap wherever the two do not meet exactly, and rotating about a
    # pivot that is not on the joint opens that gap further as the limb swings
    # (measured: 0.048 at rest growing to 0.074 through the walk cycle).
    import numpy as _np
    for child, parent in (("arm_l_lo", "arm_l"), ("arm_r_lo", "arm_r"),
                          ("leg_l_lo", "leg_l"), ("leg_r_lo", "leg_r"),
                          ("leg_l_ft", "leg_l_lo"), ("leg_r_ft", "leg_r_lo")):
        if child not in groups or parent not in groups: continue
        PV = _np.vstack([cv for cv, _cf in groups[parent]]).astype("f4")
        pv0 = pivots[child]
        d2 = ((PV - pv0) ** 2).sum(1)
        near = PV[int(_np.argmin(d2))]
        # move most of the way to the parent surface, keeping the joint height
        pivots[child] = (pv0 * 0.25 + near * 0.75).astype("f4")

    for l, r in (("arm_l", "arm_r"), ("arm_l_lo", "arm_r_lo"),
                 ("leg_l", "leg_r"), ("leg_l_lo", "leg_r_lo"),
                 ("leg_l_ft", "leg_r_ft")):
        if l in pivots and r in pivots:
            pl, pr = pivots[l], pivots[r]
            my = float((pl[1] + pr[1]) * 0.5)          # shared joint height
            mag = float((abs(pl[max_]) + abs(pr[max_])) * 0.5)
            sl = -1.0 if pl[max_] <= pr[max_] else 1.0
            md = float((pl[2] + pr[2]) * 0.5) if max_ != 2 else float((pl[0] + pr[0]) * 0.5)
            for part, sign in ((l, sl), (r, -sl)):
                pv = pivots[part]
                pv[1] = my
                pv[max_] = sign * mag
                if max_ != 2: pv[2] = md
                else:         pv[0] = md

    # Emit one part per (limb, texture) so each limb keeps its real materials.
    # A limb usually spans 1-3 materials, so this stays well inside the part limit.
    emit_list = []
    if tex_of:
        for limb in present:
            for (lb, tx), pieces in tex_of.items():
                if lb != limb: continue
                emit_list.append((limb, tx, pieces))
    else:
        emit_list = [(limb, a.texture, [(cv, None, cf) for cv, cf in groups[limb]])
                     for limb in present]

    # a child attaches to the FIRST emitted part of its parent limb
    first_of = {}
    for i, (lb, _tx, _pc) in enumerate(emit_list):
        first_of.setdefault(lb, i)

    for pi, (limb, tx, pieces) in enumerate(emit_list):
        Vs, Us, Fs, off = [], [], [], 0
        for (cv, cuv, cf) in pieces:
            Vs.append(cv)
            Us.append(cuv if cuv is not None and len(cuv) == len(cv)
                      else np.zeros((len(cv), 2), "f4"))
            Fs.append(cf + off); off += len(cv)
        Vm = np.vstack(Vs).astype("f4"); Fm = np.vstack(Fs).astype("u4")
        UVm = np.vstack(Us).astype("f4")
        has_uv = float(np.abs(UVm).max()) > 1e-6
        Vu, UV, Fu = (Vm, UVm, Fm) if has_uv else (
            ph.box_uvs(Vm, Fm) if hasattr(ph, "box_uvs")
            else (Vm, np.zeros((len(Vm), 2), "f4"), Fm))
        out = os.path.join(a.out, "imported_%s_part%d.iqm" % (name, pi))
        te.write_iqm(out, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), tx)
        # pivot: limbs rotate about their attachment, symmetrised above
        piv = pivots[limb].copy()
        if limb != "body":
            Vc = (Vm - piv).astype("f4")
            if has_uv:
                te.write_iqm(out, Vc, UV.astype("f4"), Fm.astype("u4"), tx)
            else:
                Vu2, UV2, Fu2 = ph.box_uvs(Vc, Fm) if hasattr(ph, "box_uvs") else (Vc, UV, Fm)
                te.write_iqm(out, Vu2.astype("f4"), UV2.astype("f4"), Fu2.astype("u4"), tx)
        up = UPPER_OF.get(limb, "")
        parent = first_of.get(up, -1) if up else -1
        manifest.append("%s %s %s %d %d %.5f %.5f %.5f %d" %
                        (os.path.basename(out), tx, limb,
                         0 if limb != "body" else -1,
                         LIMB_CODE[limb], piv[0], piv[1], piv[2], parent))
        print("  %-9s %3d comps, %6d tris, %-30s pivot (%+.2f %+.2f %+.2f)%s"
              % (limb, len(pieces), len(Fm), (tx or "-")[:30], piv[0], piv[1], piv[2],
                 "  child of " + UPPER_OF[limb] if limb in UPPER_OF else ""))

    # optional: band the TORSO with ribs from a catalog. Done after splitting so
    # the ribs attach to the body part and the limb hierarchy is untouched --
    # previously a model could be detailed OR articulated, never both.
    if a.detail:
        try:
            import importlib.util as _il
            _here = os.path.dirname(os.path.abspath(__file__))
            _da = None
            for c in sorted(__import__("glob").glob(os.path.join(_here, "detail_apply*.py")), reverse=True):
                b = os.path.basename(c)[:-3]
                sp = _il.spec_from_file_location(b, c); m2 = _il.module_from_spec(sp)
                sys.modules[b] = m2; sp.loader.exec_module(m2); _da = m2; break
            if _da is None: raise ImportError("detail_apply not found")
            body = groups.get("body")
            if body:
                Vb = np.vstack([cv for cv, _ in body]).astype("f4")
                blo, bhi = Vn.min(0), Vn.max(0)
                y0 = blo[1] + (bhi[1] - blo[1]) * 0.45
                y1 = blo[1] + (bhi[1] - blo[1]) * 0.80
                tor = _da.torso_extent(Vn, y0, y1)
                cat = json.load(open(os.path.join(a.detail, "partlib.json")))
                mdir = os.path.join(a.detail, "meshes")
                pool = [q for q in cat["parts"] if q.get("mesh") and
                        os.path.exists(os.path.join(mdir, q["mesh"] + ".obj"))]
                if pool and tor:
                    import random as _rnd
                    rng = _rnd.Random(7)
                    tcx, thw, tcz, thd = tor
                    segh = (y1 - y0) / max(1, a.ribs)
                    Vs, Fs, off = [], [], 0
                    for i in range(a.ribs):
                        yy = y0 + (y1 - y0) * (i + 0.5) / a.ribs
                        ring = _da.surface_ring(Vn, yy, segh * 0.55, tcx, tcz,
                                                n_ang=a.rib_segments,
                                                max_r=thw * 1.6)
                        if not ring: continue
                        for ent in ring:
                            if ent is None: continue
                            ang, rad = ent
                            q = rng.choice(pool)
                            Vp, Fp = _da.read_obj(os.path.join(mdir, q["mesh"] + ".obj"))
                            w = (2 * math.pi * rad / a.rib_segments) * 1.25
                            Pw = _da.fit_part(Vp, tcx + math.cos(ang) * rad * 1.06, yy,
                                              tcz + math.sin(ang) * rad * 1.06,
                                              w, segh * 0.62, w,
                                              rot_y=-ang, stretch=a.rib_stretch)
                            Vs.append(Pw); Fs.append(Fp + off); off += len(Pw)
                    if Vs:
                        Vm = np.vstack(Vs).astype("f4"); Fm = np.vstack(Fs).astype("u4")
                        Vu, UV, Fu = ph.box_uvs(Vm, Fm)
                        out = os.path.join(a.out, "imported_%s_part%d.iqm" % (name, len(manifest)))
                        te.write_iqm(out, Vu.astype("f4"), UV.astype("f4"), Fu.astype("u4"), a.texture)
                        manifest.append("%s %s %s %d %d %.5f %.5f %.5f %d" %
                                        (os.path.basename(out), a.texture, "body", -1, 0,
                                         0.0, 0.0, 0.0, -1))
                        print("  %-9s %3d bands, %6d tris  (torso detail)" % ("ribs", a.ribs, len(Fm)))
        except Exception as e:
            print("  (detail pass skipped: %s: %s)" % (type(e).__name__, e))

    man = os.path.join(a.out, "imported_%s_parts.txt" % name)
    open(man, "w").write("\n".join(manifest))
    print(f"\nwrote {len(manifest)} parts -> {man}")
    missing = [l for l in ("arm_l","arm_r","leg_l","leg_r") if l not in groups]
    if missing:
        print(f"  !! no components landed in: {', '.join(missing)}")
        print(f"     try --torso-frac lower (arms) or check the model is upright")

if __name__ == "__main__":
    main()
