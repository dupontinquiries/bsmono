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

def torso_extent(V, y0, y1):
    """Find the TORSO in x/z, not the whole body slab.

    A T-posed bot is as wide as its arm span, so selecting everything in the chest
    height band grabs both arms -- sizing ribs to the arm span and cutting shell
    off the limbs. Take the densest cluster around the centre line instead."""
    import numpy as np
    band = (V[:, 1] > y0) & (V[:, 1] < y1)
    S = V[band]
    if len(S) < 20: return None
    cx = float(np.median(S[:, 0]))
    # torso half-width: where vertex density around the centre line drops off
    d = np.abs(S[:, 0] - cx)
    hw = float(np.percentile(d, 35))          # inner 35% is the body, not the arms
    hw = max(hw, (V[:, 0].max() - V[:, 0].min()) * 0.08)
    core = S[d < hw]
    if len(core) < 10: core = S
    cz = float(np.median(core[:, 2]))
    hd = float(np.percentile(np.abs(core[:, 2] - cz), 85))
    return cx, hw, cz, hd

def cavity_profile(V, y0, y1, slices=12, torso=None):
    """For each horizontal slice of the chest band, measure the interior gap
    between the left and right shell walls, and the body's depth there.
    Returns [(y, half_width, half_depth, cx, cz), ...] -- ribs are sized to this
    so they stay inside the body."""
    import numpy as np
    out = []
    tcx, thw, tcz, thd = torso if torso else (0.0, 0.3, 0.0, 0.2)
    for i in range(slices):
        a = y0 + (y1 - y0) * (i + 0.5) / slices
        h = (y1 - y0) / slices * 0.7
        m = (np.abs(V[:, 1] - a) < h) & (np.abs(V[:, 0] - tcx) < thw)   # torso only
        if m.sum() < 12: continue
        S = V[m]
        hw = float(np.percentile(np.abs(S[:, 0] - tcx), 85))
        hd = float(np.percentile(np.abs(S[:, 2] - tcz), 85))
        out.append((a, hw, hd, tcx, tcz))
    return out

def gut_shell(V, F, y0, y1, front_only=True, thresh=0.30, torso=None):
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
    if torso:
        tcx, thw, tcz, thd = torso
        band = band & (np.abs(c[:, 0] - tcx) < thw)          # torso, not the arms
    else:
        tcz = 0.0
    if front_only:
        cut = band & (n[:, 2] > thresh) & (c[:, 2] > tcz)    # outward front panel
    else:
        cut = band & (np.abs(n[:, 2]) > thresh)
    return F[~cut], int(cut.sum())

def surface_ring(V, y, half_h, cx, cz, n_ang=24, max_r=None):
    """Outer surface radius of the body at height y, per angle.

    Used to lay ribs ON the body. Placing them inside is pointless -- these models
    are layered panels with no cavity, so anything interior is occluded by the
    shell no matter how much of the outer skin is cut away (measured: shell stays
    frontmost over 97.6% of the torso window either way)."""
    import numpy as np
    m = np.abs(V[:, 1] - y) < half_h
    if m.sum() < 12: return None
    S = V[m]
    dx = S[:, 0] - cx; dz = S[:, 2] - cz
    # Keep only points within the TORSO radius. Without this the ring is measured
    # across everything at chest height -- which on a T-posed bot means the ARMS,
    # so ribs were placed out at the arm span (measured: rib frame reached x +/-1.03
    # on a torso 0.34 wide, 6x too far). That is the "weird arm plates".
    if max_r is not None:
        # Limit LATERALLY only. Arms sit far out in x; the chest is legitimately
        # deep in z. Clipping radially also clipped the depth, which pushed the
        # ribs inside the chest surface and hid them again.
        keep = np.abs(dx) <= max_r
        if keep.sum() >= 12:
            S = S[keep]; dx = dx[keep]; dz = dz[keep]
    ang = np.arctan2(dz, dx); rad = np.sqrt(dx*dx + dz*dz)
    out = []
    for k in range(n_ang):
        a0 = -math.pi + 2*math.pi*k/n_ang
        sel = np.abs(((ang - a0 + math.pi) % (2*math.pi)) - math.pi) < (math.pi/n_ang)
        if sel.sum() < 2: out.append(None); continue
        out.append((a0, float(np.percentile(rad[sel], 92))))
    return out

def fit_part(V, cx, cy, cz, sx, sy, sz, rot_z=0.0, rot_y=0.0, stretch=1.0):
    """Scale a part into a box and place it.

    stretch=1.0 is uniform (never distorts). Higher values allow anisotropic
    scaling up to that ratio, which structural bands need: fitting an elongated
    part into a flat rib box uniformly shrinks it by its SMALLEST axis ratio, so
    the rib ends up microscopic and invisible."""
    import numpy as np
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    want = np.array([sx, sy, sz], "f4")
    r = want / span
    k = float(r.min())
    if stretch > 1.0:
        k3 = np.clip(r, k, k * float(stretch))     # allow up to `stretch` anisotropy
    else:
        k3 = np.array([k, k, k], "f4")
    P = (V - (lo + hi) * 0.5) * k3
    if rot_z:
        c, s = math.cos(rot_z), math.sin(rot_z)
        x, y = P[:, 0].copy(), P[:, 1].copy()
        P[:, 0] = x * c - y * s; P[:, 1] = x * s + y * c
    if rot_y:
        c, s = math.cos(rot_y), math.sin(rot_y)
        x, z = P[:, 0].copy(), P[:, 2].copy()
        P[:, 0] = x * c + z * s; P[:, 2] = -x * s + z * c
    return (P + np.array([cx, cy, cz], "f4")).astype("f4")

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True, help="the bot to add structure to (.gltf/.glb/.obj)")
    ap.add_argument("--catalog", required=True, help="a part_harvest --out dir with meshes/")
    ap.add_argument("--name", default=None)
    ap.add_argument("--out", default="detailed")
    ap.add_argument("--ribs", type=int, default=12, help="rib bands up the torso")
    ap.add_argument("--rib-segments", type=int, default=14,
                    help="segments per rib band around the body")
    ap.add_argument("--rib-offset", type=float, default=1.02,
                    help="rib radius as a multiple of the measured surface radius "
                         "(>1 sits proud of the shell, which is what makes it visible)")
    ap.add_argument("--rib-stretch", type=float, default=4.0,
                    help="max anisotropy when fitting a part into a rib slot. 1 = "
                         "never distort, but then elongated parts shrink to nothing")
    ap.add_argument("--rib-arc", type=float, default=3.15,
                    help="limit ribs to a front arc in radians (3.15 = full ring)")
    ap.add_argument("--cluster-pitch", type=float, default=2.4,
                    help="spacing between cluster elements, in element widths. "
                         "Below ~2 they merge into a single lump.")
    ap.add_argument("--cluster", type=int, default=6,
                    help="greebles per cluster. Uniform scatter reads as noise; "
                         "aligned groups read as panels and vents.")
    ap.add_argument("--greeble-size", type=float, default=0.05,
                    help="greeble size as a fraction of body extent (was effectively "
                         "0.04 and read as scattered specks)")
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

    # Load the shell BY MATERIAL. Emitting it with the default texture made every
    # RIB model render untextured -- the same fault that made limb-split models
    # flat blue. The added ribs have no source texture and keep the placeholder;
    # the SHELL must not.
    shell_mats = []
    try:
        if not a.target.lower().endswith(".obj"):
            g0, gj, gbase = te.load_by_material(a.target)
            if g0:
                for (mn, mi), (mv, muv, mf) in g0.items():
                    tx = None
                    try:
                        t = te.base_color_texture(gj, gbase, mi)
                        if t:
                            os.makedirs(os.path.join(a.out, "textures"), exist_ok=True)
                            tx = "%s_%s" % (name if a.name else "shell", os.path.basename(t))
                            dst = os.path.join(a.out, "textures", tx)
                            try: tx = te.convert_texture(t, dst, 1024, "auto", 92)
                            except Exception:
                                import shutil as _sh; _sh.copy2(t, dst)
                    except Exception: pass
                    shell_mats.append((tx, np.asarray(mv, "f4"),
                                       np.asarray(muv, "f4"), np.asarray(mf, "u4")))
    except Exception:
        shell_mats = []

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
    tor = torso_extent(Vn, y0, y1)
    if tor: print(f"torso: centre x={tor[0]:+.2f} half-width {tor[1]:.2f} "
                  f"(body half-width {(nhi[0]-nlo[0])*0.5:.2f}) -- arms excluded")
    prof = cavity_profile(Vn, y0, y1, slices=max(6, a.ribs), torso=tor)
    if not prof: raise SystemExit("could not find a chest cavity in that band -- try --chest")
    print(f"chest cavity: {len(prof)} slices, half-width {min(p[1] for p in prof):.2f}"
          f"..{max(p[1] for p in prof):.2f}")

    os.makedirs(a.out, exist_ok=True)
    parts = []
    buckets = {}      # bucket -> [(V, F), ...]; merged at the end
    shell_tex = []    # (texture, V, UV, F) written separately so materials survive
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
        for (tx, mv, muv, mf) in shell_tex:          # textured shell pieces first
            out = os.path.join(a.out, "imported_%s_part%d.iqm" % (name, n_out))
            te.write_iqm(out, mv.astype("f4"), muv.astype("f4"), mf.astype("u4"), tx)
            parts.append("%s %s %s %d %d %.5f %.5f %.5f"
                         % (os.path.basename(out), tx, "body", -1, 0, 0.0, 0.0, 0.0))
            n_out += 1
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
        Fshell, n_cut = gut_shell(Vn, Fshell, y0, y1, front_only=(a.gut == "front"), torso=tor)
        print(f"gutted chest: removed {n_cut:,} of {len(F):,} shell faces "
              f"({100.0*n_cut/max(1,len(F)):.0f}%) -- cavity is now open")
    if a.keep_shell:
        if shell_mats:
            for (tx, mv, muv, mf) in shell_mats:
                mvn = ((mv - ctr) / span * 2.0).astype("f4")
                shell_tex.append((tx or a.texture, mvn, muv, mf))
        else:
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

    # 3. ribs: banded ON the body surface. Interior ribs are invisible on these
    #    models (layered panels, no cavity), so each rib is a ring of segments laid
    #    just proud of the measured surface radius -- visible by construction.
    tcx, thw, tcz, thd = tor if tor else (0.0, 0.3, 0.0, 0.2)
    seg_h = (y1 - y0) / max(1, a.ribs)
    n_seg = max(6, a.rib_segments)
    ribs_placed = 0
    for i in range(a.ribs):
        t = (i + 0.5) / a.ribs
        y = y0 + (y1 - y0) * t
        ring = surface_ring(Vn, y, seg_h * 0.55, tcx, tcz, n_ang=n_seg,
                            max_r=thw * 1.6)
        if not ring: continue
        taper = 0.80 + 0.20 * math.sin(t * math.pi)      # slightly fuller mid-chest
        for entry in ring:
            if entry is None: continue
            ang, rad = entry
            if a.rib_arc < 3.14 and abs(((ang + math.pi) % (2*math.pi)) - math.pi) > a.rib_arc:
                continue                                  # front arc only, if asked
            r = rad * a.rib_offset
            px = tcx + math.cos(ang) * r
            pz = tcz + math.sin(ang) * r
            p = pick("strut", "block", "tread_link")
            Vp, Fp = read_obj(os.path.join(mesh_dir, p["mesh"] + ".obj"))
            w = (2 * math.pi * rad / n_seg) * 1.25        # span the gap to its neighbour
            emit(fit_part(Vp, px, y, pz,
                          w, seg_h * 0.62 * taper, w,
                          rot_y=-ang, stretch=a.rib_stretch), Fp, "frame")
            ribs_placed += 1

    # 4. surface greebles. Two fixes over the first version, both reported from
    #    play: they were 2.5-6% of the body (too small to read as detail, so they
    #    sat in negative space) and were not oriented, so they lay at arbitrary
    #    angles instead of standing on the surface.
    rules = idx.get("rules", {})
    Nv = None
    if a.greebles:
        tri = Vn[F]; fn = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
        ln = np.linalg.norm(fn, axis=1, keepdims=True); ln[ln == 0] = 1; fn = fn / ln
        Nv = np.zeros_like(Vn)
        for k2 in range(3): np.add.at(Nv, F[:, k2], fn)
        ln = np.linalg.norm(Nv, axis=1, keepdims=True); ln[ln == 0] = 1; Nv = Nv / ln
    # Placed in CLUSTERS, not uniform random. Rendering the scatter showed exactly
    # what the user reported: specks with no structure. Real mechanical detail is
    # organised -- a panel of bolts, a bank of vents, a row of louvres -- so seed a
    # few sites and lay a small aligned grid at each, sharing one part type and
    # size per cluster.
    n_sites = max(1, a.greebles // max(2, a.cluster))
    placed_g = 0
    for _site in range(n_sites):
        nm = rng.choice([k for k in by] or ["block"])
        r = rules.get(nm)
        fy = rng.uniform(*(r["y"] if r else (0.15, 0.85)))
        y = nlo[1] + (nhi[1] - nlo[1]) * fy
        m = np.abs(Vn[:, 1] - y) < (nhi[1] - nlo[1]) * 0.05
        if m.sum() < 8: continue
        S = Vn[m]
        anchor = S[rng.randrange(len(S))]
        nrm = Nv[int(np.where(m)[0][rng.randrange(len(S))])] if Nv is not None \
              else np.array([0.0, 1.0, 0.0], "f4")
        p = rng.choice(by[nm])
        Vp, Fp = read_obj(os.path.join(mesh_dir, p["mesh"] + ".obj"))
        sz = (nhi - nlo).max() * rng.uniform(a.greeble_size * 0.8, a.greeble_size * 1.3)
        # build a local frame on the surface so the grid lies flat against it
        up = nrm / (np.linalg.norm(nrm) + 1e-6)
        ref = np.array([0.0, 1.0, 0.0], "f4")
        if abs(float(up[1])) > 0.9: ref = np.array([1.0, 0.0, 0.0], "f4")
        t1 = np.cross(up, ref); t1 /= (np.linalg.norm(t1) + 1e-6)
        t2 = np.cross(up, t1)
        cols = max(1, int(round(math.sqrt(a.cluster))))
        rows_ = max(1, a.cluster // cols)
        # spacing must exceed the element size or the cluster merges into one
        # mass -- which reads as a lump, not as panel detail
        pitch = sz * a.cluster_pitch
        yaw = math.atan2(up[0], up[2])
        pitch_a = math.asin(max(-1.0, min(1.0, float(up[1]))))
        for gi in range(cols):
            for gj in range(rows_):
                if placed_g >= a.greebles: break
                off_u = (gi - (cols - 1) * 0.5) * pitch
                off_v = (gj - (rows_ - 1) * 0.5) * pitch
                pos = anchor + up * sz * 0.35 + t1 * off_u + t2 * off_v
                emit(fit_part(Vp, pos[0], pos[1], pos[2], sz, sz * 1.3, sz,
                              rot_z=-pitch_a, rot_y=yaw, stretch=1.5), Fp, "greeble")
                placed_g += 1

    n_meshes = flush()
    man = os.path.join(a.out, "imported_%s_parts.txt" % name)
    open(man, "w").write("\n".join(parts))
    print(f"\nplaced {added} pieces, batched into {n_meshes} mesh(es) -> {man}")
    print(f"  shell 1 + spine {spine_n} + rib segments {ribs_placed} "
          f"+ greebles {max(0, added-1-spine_n-ribs_placed)}")
    print(f"  structural members: {before} -> ~{before + added - 1}  (target 479)")
    print(f"\ninstall:\n  python3 tools/install_models.py --parts-dir {a.out} --engine engine_src")

if __name__ == "__main__":
    main()
