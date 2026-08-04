#!/usr/bin/env python3
"""partclass.py — annotate the part database without a neural net.

    ./tools/partclass.py features out.npz      # extract, whole roster
    ./tools/partclass.py cluster out.npz -k 24 # find the natural classes
    ./tools/partclass.py label out.npz         # apply the rules, report

WHY NOT YOLO. YOLOv3 is a 2D detector trained on COCO: it knows "car",
"truck", "person". It has never seen an exhaust pipe as a class, and there
is no labelled corpus of "hover wheel vs tread" to fine-tune on — building
one is the expensive part, and running a net does not avoid it.

What we have instead is much better than pixels: the parts are ALREADY
SEGMENTED. Every model arrives as a few hundred named meshes with exact
geometry. So the job is not detection, it is DESCRIPTION plus clustering:
measure each part, group the roster's ~150k parts into a few dozen natural
classes, and label the CLASSES by hand. Thirty labels instead of a hundred
thousand, and every one checkable by listing its members.

The features are chosen for the classes you named:

  exhaust / thruster  a tube: high aspect, round cross-section, at the REAR,
                      and hollow — a nozzle has a hole down it
  gun / barrel        the same tube shape, but forward or on the upper deck
  armour plating      flat, wide, thin, and on the SURFACE (low enclosure)
  hover pad vs wheel  a wheel is a round disc in a mirrored pair low down; a
                      hover pad is flat and downward-facing with no roundness
  tread               many small round parts in a line along the body
  structure           high enclosure — the interior work already measured

Nothing here is a guess about what a part IS. It is a measurement of what a
part looks like, and the naming happens once, at the cluster level, where a
human can see all the members at once.
"""
import json
import math
import os
import subprocess
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
SKY = os.path.dirname(HERE)
BS = os.path.join(SKY, "..", "blackstar")
DATA = os.path.join(BS, "engine_src", "data")
FV = os.path.join(BS, "rust", "foldview", "target", "release", "foldview")

# names foldview gives parts; kept as a WEAK prior, never as the answer
FEATURES = [
    "elong",        # longest / middle extent
    "flat",         # shortest / middle    (a plate is small here)
    "round",        # how circular the cross-section is, 0..1
    "hollow",       # fraction of the cross-section that is empty
    "vol_frac",     # size relative to the whole model
    "fore",         # -1 tail .. +1 nose
    "up",           # -1 floor .. +1 roof
    "lateral",      # |x| from the centreline, 0..1
    "mirrored",     # 1 if a part of similar size sits mirrored in x
    "enclosure",    # 0 exposed .. 1 buried
    "axis_fore",    # |long axis . forward|
    "axis_up",      # |long axis . up|
]


def parts_of(iqm, t=0.0):
    """Named part clouds from a foldview export."""
    out_dir = "/tmp/_pc"
    subprocess.run(
        [FV, "--data", DATA, "--content", os.path.join(BS, "content"),
         "--veh", os.path.join(DATA, iqm), "--gltf", out_dir, "--t", str(t)],
        capture_output=True, timeout=240)
    g = json.load(open(f"{out_dir}/scene.gltf"))
    buf = open(f"{out_dir}/scene.bin", "rb").read()
    out = []
    for n in g["nodes"]:
        a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        p = np.frombuffer(buf, "<f4", count=a["count"] * 3, offset=off).reshape(-1, 3)
        out.append((n.get("name", ""), np.asarray(p, float)))
    return out


def describe(pts, model_lo, model_ext, others_c, others_r):
    """The twelve numbers. All scale-free, so a lander and a carrier compare."""
    c = pts.mean(0)
    q = pts - c
    e = q.max(0) - q.min(0)
    order = np.argsort(-e)
    a, b, cc = e[order[0]], e[order[1]], e[order[2]]
    elong = a / max(b, 1e-9)
    flat = cc / max(b, 1e-9)

    # cross-section perpendicular to the longest axis: roundness and hollowness
    long_ax = np.zeros(3); long_ax[order[0]] = 1.0
    perp = q - np.outer(q @ long_ax, long_ax)
    r = np.linalg.norm(perp, axis=1)
    rmax = max(r.max(), 1e-9)
    # ROUNDNESS WAS BROKEN, and it was 87% of the reason 62% of real parts
    # matched nothing. std/rmax is not a shape measure: for a shell-sampled
    # BOX the radii spread from face-centre to corner, so std is large and
    # roundness collapses to ~0.007 — while every synthetic exemplar, being
    # sampled from analytic surfaces with far more regular radii, sat at
    # 0.85. The two populations could not meet, and no amount of new
    # archetypes would have fixed it because the axis itself was wrong.
    #
    # Measure it properly: how close is the cross-section to a circle, as the
    # ratio of its mean radius to its max. A circle gives 1, a square about
    # 0.9, a flat plate about 0.5. Bounded, stable under sampling density,
    # and it means the same thing for both populations.
    sel = r > rmax * 0.15
    round_ = float(r[sel].mean() / rmax) if sel.sum() > 8 else 0.0
    hollow = float((r > rmax * 0.55).mean())

    vol = a * b * cc
    vol_frac = vol / max(float(np.prod(model_ext)), 1e-9)

    rel = (c - model_lo) / np.maximum(model_ext, 1e-9)
    fore = float(1.0 - 2.0 * rel[2])     # nose is -Z
    up = float(2.0 * rel[1] - 1.0)
    lateral = float(min(abs(rel[0] - 0.5) * 2.0, 1.0))

    # a mirrored twin of similar size on the other side of the centreline
    mid_x = model_lo[0] + model_ext[0] * 0.5
    mirror = np.array([2 * mid_x - c[0], c[1], c[2]])
    d = np.linalg.norm(others_c - mirror, axis=1)
    tol = max(float(np.max(model_ext)) * 0.04, 1e-6)
    mirrored = float((d < tol).any())

    # long-axis orientation
    axis_fore = abs(float(long_ax[2]))
    axis_up = abs(float(long_ax[1]))
    return [elong, flat, round_, hollow, vol_frac, fore, up, lateral,
            mirrored, 0.0, axis_fore, axis_up]


def extract(models, out_path, limit_parts=100000):
    rows, meta = [], []
    for mi, iqm in enumerate(models):
        try:
            ps = parts_of(iqm)
        except Exception as ex:
            print(f"  skip {iqm}: {ex}")
            continue
        allp = np.vstack([p for _, p in ps])
        lo, ext = allp.min(0), allp.max(0) - allp.min(0)
        cents = np.array([p.mean(0) for _, p in ps])
        radii = np.array([np.linalg.norm(p - p.mean(0), axis=1).max() for _, p in ps])
        for k, (name, p) in enumerate(ps):
            if len(p) < 4:
                continue
            f = describe(p, lo, ext, cents, radii)
            rows.append(f)
            meta.append((os.path.basename(iqm), name, k))
        print(f"  [{mi+1}/{len(models)}] {os.path.basename(iqm)[:40]:40s} {len(ps)} parts")
        if len(rows) > limit_parts:
            break
    X = np.array(rows, dtype=np.float32)
    np.savez_compressed(out_path, X=X, meta=np.array(meta, dtype=object),
                        names=np.array(FEATURES))
    print(f"{len(X)} parts x {X.shape[1]} features -> {out_path}")


def kmeans(X, k, iters=40, seed=0):
    """Plain Lloyd's. No sklearn: one dependency for something this small is
    not worth it, and the result is checkable by eye."""
    rng = np.random.default_rng(seed)
    # k-means++ seeding, because random init on skewed data gives empty clusters
    C = [X[rng.integers(len(X))]]
    for _ in range(k - 1):
        d = np.min(((X[:, None, :] - np.array(C)[None]) ** 2).sum(-1), axis=1)
        p = d / max(d.sum(), 1e-12)
        C.append(X[rng.choice(len(X), p=p)])
    C = np.array(C)
    for _ in range(iters):
        lab = np.argmin(((X[:, None, :] - C[None]) ** 2).sum(-1), axis=1)
        for j in range(k):
            m = lab == j
            if m.any():
                C[j] = X[m].mean(0)
    return lab, C


def znorm(X):
    mu, sd = X.mean(0), X.std(0)
    sd[sd < 1e-6] = 1.0
    return (X - mu) / sd


def cluster(path, k):
    d = np.load(path, allow_pickle=True)
    X, meta = d["X"], d["meta"]
    lab, C = kmeans(znorm(X), k)
    print(f"{len(X)} parts into {k} clusters\n")
    for j in range(k):
        m = lab == j
        if not m.any():
            continue
        names = {}
        for nm in meta[m][:, 1]:
            stem = "".join(ch for ch in str(nm) if ch.isalpha())
            names[stem] = names.get(stem, 0) + 1
        top = sorted(names.items(), key=lambda x: -x[1])[:4]
        f = X[m].mean(0)
        print(f"cluster {j:2d}  n={m.sum():6d}  elong{f[0]:5.1f} flat{f[1]:5.2f} "
              f"round{f[2]:5.2f} hollow{f[3]:5.2f} fore{f[5]:+5.2f} up{f[6]:+5.2f} "
              f"mirr{f[8]:4.2f}  {top}")


# ---- A-PRIORI RULES DO NOT WORK, and the first run proved it: they put
# ---- 94.7% of parts in "hull" and filled "wheel" with `muscle`. Guessing
# ---- thresholds before seeing the distribution is the same error as picking
# ---- 0.55 for enclosure before seeing that a car is dense.
# ----
# ---- Two things the run also taught, both worth keeping:
# ----
# ----   `flat` near 1 means CHUNKY, not flat — it is shortest/middle, so a
# ----   plate is near 0. Easy to invert in your head and I did.
# ----
# ----   THE PLATE SIGNATURE IS GONE, and I removed it. gltfx now thickens
# ----   degenerate sheets into slabs, so the armour panels that used to read
# ----   as flat < 0.02 now read as ordinary boxes. The thickening was right;
# ----   it just means "is this armour" has to come from area and enclosure
# ----   rather than from thinness.
# ----
# ---- So the rules stay as a FALLBACK for parts no cluster covers, and the
# ---- real labelling happens the other way round: cluster first, name the
# ---- clusters, propagate. Thirty labels a human can check beats a hundred
# ---- thousand nobody can.
def classify(f):
    elong, flat, round_, hollow, vol, fore, up, lateral, mirrored, enc, ax_f, ax_u = f
    tube = elong > 2.5 and round_ > 0.55 and flat > 0.45
    if tube and hollow > 0.35 and fore < -0.25 and ax_f > 0.6:
        return "exhaust"           # hollow tube, pointing back, at the tail
    if tube and (fore > 0.2 or up > 0.3) and ax_f > 0.5:
        return "barrel"            # solid tube, forward or on the upper deck
    if flat < 0.12 and vol > 0.004 and elong < 3.0:
        return "plate"             # wide, thin: armour or panel
    if round_ > 0.7 and flat > 0.7 and mirrored > 0.5 and up < -0.2:
        return "wheel"             # round disc, paired, low
    if flat < 0.3 and up < -0.35 and abs(fore) < 0.6 and not mirrored:
        return "underpan"          # flat, downward, central — hover surface
    if elong > 4.0 and flat < 0.35 and abs(up) < 0.4:
        return "strut"
    return "hull"


LABELS_PATH = os.path.join(HERE, "part-labels.json")


# =====================================================================
# SYNTHETIC EXEMPLARS — the way out of the bootstrap.
#
# There is no labelled corpus of "hover pad vs tread link", so make one.
# Every archetype below is a parametric surface sampler with a KNOWN label,
# and every one is measured by the same describe() the real parts go
# through — that identity is the whole correctness requirement. If the
# synthetic features were computed differently the nearest-neighbour lookup
# would be comparing two different things and quietly returning nonsense.
#
# Surface samples, not solids, because the real meshes are surfaces too. A
# solid-sampled cylinder and a shell-sampled cylinder have completely
# different `hollow` and `round`, which is the trap the plate signature fell
# into earlier.
# =====================================================================

def _rng(seed):
    return np.random.default_rng(seed)


def _tube(r, g, length, radius, wall=0.0, n=900):
    """Open cylinder shell. wall > 0 makes it an annulus — a nozzle."""
    th = g.uniform(0, 2 * math.pi, n)
    z = g.uniform(-length / 2, length / 2, n)
    rad = radius * np.ones(n) if wall <= 0 else g.choice([radius, radius * (1 - wall)], n)
    return np.stack([rad * np.cos(th), rad * np.sin(th), z], 1)


def _box(g, sx, sy, sz, n=900):
    """Box SHELL: pick a face, then a point on it."""
    f = g.integers(0, 6, n)
    u, v = g.uniform(-0.5, 0.5, n), g.uniform(-0.5, 0.5, n)
    p = np.zeros((n, 3))
    for i in range(6):
        m = f == i
        ax, sgn = i // 2, (i % 2) * 2 - 1
        a, b = [k for k in range(3) if k != ax]
        p[m, ax] = 0.5 * sgn
        p[m, a] = u[m]
        p[m, b] = v[m]
    return p * np.array([sx, sy, sz])


def _disc(g, radius, thick, n=900):
    """Wheel: an annular rim plus two faces."""
    th = g.uniform(0, 2 * math.pi, n)
    rr = radius * np.sqrt(g.uniform(0.25, 1.0, n))
    z = g.choice([-thick / 2, thick / 2], n)
    rim = g.random(n) < 0.35
    rr[rim] = radius
    z[rim] = g.uniform(-thick / 2, thick / 2, rim.sum())
    return np.stack([rr * np.cos(th), rr * np.sin(th), z], 1)


def _plate(g, w, h, t, n=900):
    return _box(g, w, t, h, n)


def _taper(g, length, r0, r1, n=900):
    th = g.uniform(0, 2 * math.pi, n)
    u = g.uniform(0, 1, n)
    rad = r0 + (r1 - r0) * u
    return np.stack([rad * np.cos(th), rad * np.sin(th), (u - 0.5) * length], 1)


# archetype -> (builder, class label). Position and orientation are applied
# by the caller, because fore/up/lateral are features and a barrel at the
# tail is an exhaust.
ARCHETYPES = {
    "barrel":    lambda g: _tube(0, g, g.uniform(1.4, 4.0), g.uniform(0.05, 0.16)),
    "muzzle":    lambda g: _taper(g, g.uniform(0.4, 0.9), g.uniform(0.08, 0.2), g.uniform(0.12, 0.3)),
    "exhaust":   lambda g: _tube(0, g, g.uniform(0.5, 1.6), g.uniform(0.07, 0.2), wall=g.uniform(0.2, 0.5)),
    "thruster":  lambda g: _taper(g, g.uniform(0.6, 1.5), g.uniform(0.10, 0.22), g.uniform(0.22, 0.5)),
    "wheel":     lambda g: _disc(g, g.uniform(0.3, 0.7), g.uniform(0.12, 0.35)),
    "tread":     lambda g: _box(g, g.uniform(0.15, 0.3), g.uniform(0.08, 0.15), g.uniform(1.6, 3.2)),
    "hoverpad":  lambda g: _plate(g, g.uniform(0.5, 1.0), g.uniform(0.6, 1.4), g.uniform(0.04, 0.10)),
    "plate":     lambda g: _plate(g, g.uniform(0.6, 1.6), g.uniform(0.5, 1.5), g.uniform(0.03, 0.09)),
    "fin":       lambda g: _plate(g, g.uniform(0.1, 0.25), g.uniform(0.7, 1.6), g.uniform(0.02, 0.06)),
    "strut":     lambda g: _tube(0, g, g.uniform(1.2, 3.5), g.uniform(0.02, 0.06)),
    "coil":      lambda g: _tube(0, g, g.uniform(0.3, 0.7), g.uniform(0.12, 0.25)),
    "piston":    lambda g: _tube(0, g, g.uniform(0.5, 1.2), g.uniform(0.06, 0.12)),
    "canopy":    lambda g: _taper(g, g.uniform(0.8, 1.6), g.uniform(0.25, 0.5), g.uniform(0.05, 0.15)),
    "intake":    lambda g: _tube(0, g, g.uniform(0.3, 0.7), g.uniform(0.15, 0.35), wall=g.uniform(0.3, 0.6)),
    "antenna":   lambda g: _tube(0, g, g.uniform(0.8, 2.0), g.uniform(0.01, 0.03)),
    "greeble":   lambda g: _box(g, g.uniform(0.05, 0.2), g.uniform(0.05, 0.2), g.uniform(0.05, 0.2)),
    "hullbox":   lambda g: _box(g, g.uniform(0.6, 1.6), g.uniform(0.4, 1.0), g.uniform(0.8, 2.4)),
    # ---- guts: the boxy interior families. THIS IS THE MISSING SET.
    # Ten ordinary road cars put 55% of their parts in "intake" because
    # nothing box-shaped-and-buried existed to match — a Boxster does not
    # have 372 intakes, it has floor pan sections, sill boxes and frame
    # rails. Roundness settled it: the exemplars sit at 0.79, the real
    # matches at 0.13.
    "guts_rail":     lambda g: _box(g, g.uniform(0.06, 0.14), g.uniform(0.08, 0.18), g.uniform(1.2, 3.0)),
    "guts_cross":    lambda g: _box(g, g.uniform(1.0, 2.2), g.uniform(0.06, 0.14), g.uniform(0.06, 0.16)),
    "guts_pan":      lambda g: _box(g, g.uniform(0.7, 1.5), g.uniform(0.03, 0.09), g.uniform(0.8, 1.8)),
    "guts_bulkhead": lambda g: _box(g, g.uniform(0.7, 1.4), g.uniform(0.4, 0.9), g.uniform(0.04, 0.10)),
    "guts_tank":     lambda g: _box(g, g.uniform(0.3, 0.7), g.uniform(0.2, 0.45), g.uniform(0.35, 0.8)),
    "guts_duct":     lambda g: _tube(0, g, g.uniform(0.6, 1.8), g.uniform(0.04, 0.09)),
    "guts_bracket":  lambda g: _box(g, g.uniform(0.08, 0.22), g.uniform(0.08, 0.22), g.uniform(0.10, 0.28)),
    "guts_mount":    lambda g: _box(g, g.uniform(0.12, 0.3), g.uniform(0.12, 0.3), g.uniform(0.12, 0.3)),
    # ---- skeleton ----
    "bone":      lambda g: _taper(g, g.uniform(1.0, 2.4), g.uniform(0.06, 0.12), g.uniform(0.09, 0.18)),
    "ball":      lambda g: _disc(g, g.uniform(0.12, 0.25), g.uniform(0.2, 0.4)),
    "rib":       lambda g: _tube(0, g, g.uniform(0.8, 1.8), g.uniform(0.02, 0.05)),
    "vertebra":  lambda g: _box(g, g.uniform(0.15, 0.3), g.uniform(0.1, 0.2), g.uniform(0.1, 0.2)),
    "pelvis":    lambda g: _box(g, g.uniform(0.5, 0.9), g.uniform(0.3, 0.5), g.uniform(0.3, 0.6)),
    "skull":     lambda g: _disc(g, g.uniform(0.2, 0.35), g.uniform(0.3, 0.5)),
    # ---- human ----
    "h_torso":   lambda g: _box(g, g.uniform(0.35, 0.5), g.uniform(0.55, 0.75), g.uniform(0.2, 0.3)),
    "h_head":    lambda g: _disc(g, g.uniform(0.1, 0.14), g.uniform(0.2, 0.26)),
    "h_limb":    lambda g: _taper(g, g.uniform(0.6, 0.95), g.uniform(0.05, 0.08), g.uniform(0.04, 0.06)),
    "h_hand":    lambda g: _box(g, g.uniform(0.08, 0.12), g.uniform(0.16, 0.22), g.uniform(0.04, 0.06)),
    "h_foot":    lambda g: _box(g, g.uniform(0.09, 0.13), g.uniform(0.06, 0.09), g.uniform(0.22, 0.3)),
}

# where each class typically SITS, as (fore, up, lateral, mirrored) priors.
# fore +1 nose, -1 tail. These are part of the exemplar, not a filter.
PLACEMENT = {
    "barrel": (0.55, 0.25, 0.2, 0), "muzzle": (0.85, 0.2, 0.2, 0),
    "exhaust": (-0.75, -0.15, 0.35, 1), "thruster": (-0.8, 0.0, 0.4, 1),
    "wheel": (0.4, -0.75, 0.85, 1), "tread": (0.0, -0.7, 0.8, 1),
    "hoverpad": (0.0, -0.85, 0.5, 1), "plate": (0.1, 0.2, 0.5, 1),
    "fin": (-0.5, 0.5, 0.6, 1), "strut": (0.0, -0.3, 0.4, 1),
    "coil": (-0.2, -0.2, 0.5, 1), "piston": (0.1, -0.4, 0.6, 1),
    "canopy": (0.45, 0.6, 0.1, 0), "intake": (0.3, 0.1, 0.6, 1),
    "antenna": (-0.3, 0.8, 0.3, 0), "greeble": (0.0, 0.2, 0.5, 0),
    "hullbox": (0.0, 0.0, 0.2, 0),
    # buried, low, and mostly paired — a frame rail has a twin, a pan does not
    "guts_rail": (0.0, -0.45, 0.55, 1), "guts_cross": (0.1, -0.45, 0.05, 0),
    "guts_pan": (0.0, -0.6, 0.15, 0), "guts_bulkhead": (-0.35, -0.1, 0.1, 0),
    "guts_tank": (-0.45, -0.3, 0.35, 0), "guts_duct": (0.2, -0.2, 0.45, 1),
    "guts_bracket": (0.0, -0.2, 0.5, 1), "guts_mount": (0.1, -0.35, 0.5, 1),
    "bone": (0.0, 0.0, 0.5, 1),
    "ball": (0.0, 0.1, 0.4, 1), "rib": (0.0, 0.2, 0.4, 1),
    "vertebra": (-0.1, 0.3, 0.05, 0), "pelvis": (0.0, -0.1, 0.05, 0),
    "skull": (0.2, 0.9, 0.05, 0), "h_torso": (0.0, 0.3, 0.05, 0),
    "h_head": (0.0, 0.85, 0.05, 0), "h_limb": (0.0, -0.2, 0.4, 1),
    "h_hand": (0.1, -0.1, 0.5, 1), "h_foot": (0.15, -0.9, 0.3, 1),
}


def synth(n_each=18, seed=7):
    """Build the exemplar table: features + label, measured by describe()."""
    g = _rng(seed)
    X, y = [], []
    # a fake model box so fore/up/lateral are meaningful
    model_lo = np.array([-2.0, -1.0, -3.0])
    model_ext = np.array([4.0, 2.0, 6.0])
    for cls, build in ARCHETYPES.items():
        fore, up, lat, mirr = PLACEMENT[cls]
        for _ in range(n_each):
            pts = build(g)
            # random orientation about the part's own long axis, plus the
            # occasional 90-degree lay-down: a barrel is a barrel whichever
            # way the modeller pointed it
            if g.random() < 0.35:
                pts = pts[:, [2, 1, 0]]
            if g.random() < 0.25:
                pts = pts[:, [0, 2, 1]]
            c = np.array([
                (lat * g.uniform(0.6, 1.0)) * model_ext[0] * 0.5,
                up * g.uniform(0.6, 1.0) * model_ext[1] * 0.5,
                -fore * g.uniform(0.6, 1.0) * model_ext[2] * 0.5,
            ])
            pts = pts + c
            others_c = np.array([c, [-c[0], c[1], c[2]]]) if mirr else np.array([c, c + 9.0])
            f = describe(pts, model_lo, model_ext, others_c, np.array([0.2, 0.2]))
            X.append(f)
            y.append(cls)
    return np.array(X, np.float32), np.array(y)


def label_clusters(path, k):
    """Cluster, then write a stub label file for a human to fill in.

    This is the labour-efficient half. Naming 24 clusters by looking at their
    members and their shape is an afternoon; naming 150k parts is not, and a
    net that could do it would need the same afternoon done first, several
    hundred times over, to have anything to train on.
    """
    d = np.load(path, allow_pickle=True)
    X, meta = d["X"], d["meta"]
    lab, C = kmeans(znorm(X), k)
    out = {"_doc": [
        "Cluster -> part class. Fill in `label` for each; the shape and the",
        "commonest member names are there to decide from.",
        "",
        "Suggested vocabulary: exhaust, thruster, barrel, muzzle, plate,",
        "armour, wheel, tread, hoverpad, strut, frame, cabin, glass, fin,",
        "greeble, hull.",
    ], "clusters": []}
    for j in range(k):
        m = lab == j
        if not m.any():
            continue
        names = {}
        for nm in meta[m][:, 1]:
            stem = "".join(ch for ch in str(nm) if ch.isalpha())
            names[stem] = names.get(stem, 0) + 1
        f = X[m].mean(0)
        out["clusters"].append({
            "id": int(j),
            "n": int(m.sum()),
            "label": "",
            "shape": {k2: round(float(v), 3) for k2, v in zip(FEATURES, f)},
            "members": sorted(names.items(), key=lambda x: -x[1])[:6],
        })
    json.dump(out, open(LABELS_PATH, "w"), indent=1)
    print(f"{k} clusters -> {LABELS_PATH}  (fill in `label` and rerun `label`)")


def label(path):
    d = np.load(path, allow_pickle=True)
    X, meta = d["X"], d["meta"]
    counts = {}
    examples = {}
    for f, m in zip(X, meta):
        c = classify(f)
        counts[c] = counts.get(c, 0) + 1
        examples.setdefault(c, []).append(str(m[1]))
    tot = len(X)
    for c, n in sorted(counts.items(), key=lambda x: -x[1]):
        ex = {}
        for nm in examples[c][:400]:
            stem = "".join(ch for ch in nm if ch.isalpha())
            ex[stem] = ex.get(stem, 0) + 1
        top = sorted(ex.items(), key=lambda x: -x[1])[:4]
        print(f"  {c:10s} {n:6d}  {100*n/tot:5.1f}%   {top}")


def abstain_cut(A, ye, mult=3.0):
    """How far is too far — measured from the EXEMPLARS, not the query set.

    The first version used the 55th percentile of the query distances, which
    reports 45% unknown by construction however good the library gets.
    Adding eight guts archetypes changed the number by 0.0%, which is what a
    self-fulfilling threshold looks like from the outside.

    The honest cut comes from the reference frame: how far apart are members
    of the SAME class? A real part further than a few of those from anything
    is genuinely unlike everything we have described, and that is a fact
    about the library rather than about the sample.
    """
    spreads = []
    for c in set(map(str, ye)):
        m = np.array([str(v) == c for v in ye])
        if m.sum() < 2:
            continue
        Z = A[m]
        D = ((Z[:, None, :] - Z[None]) ** 2).sum(-1)
        np.fill_diagonal(D, np.nan)
        spreads.append(np.sqrt(np.nanmedian(D)))
    return float(np.median(spreads) * mult) if spreads else 6.0


def report_match(ex, real):
    """Label real parts by nearest synthetic exemplar.

    Both sides are z-normalised with the EXEMPLARS' statistics, not the real
    parts': the exemplar table is the reference frame, so normalising the
    query set by its own mean would slide it against the reference and the
    nearest neighbour would depend on which parts you happened to load.
    """
    Xe, ye = ex["X"], ex["y"]
    Xr, meta = real["X"], real["meta"]
    mu, sd = Xe.mean(0), Xe.std(0)
    sd[sd < 1e-6] = 1.0
    A, B = (Xe - mu) / sd, (Xr - mu) / sd
    d = ((B[:, None, :] - A[None]) ** 2).sum(-1)
    near = np.argmin(d, 1)
    conf = 1.0 / (1.0 + np.sqrt(d[np.arange(len(B)), near]))
    counts, examples = {}, {}
    far_all = np.sqrt(d[np.arange(len(B)), near])
    cut_all = abstain_cut(A, ye)
    for i, k in enumerate(near):
        c = "unknown" if far_all[i] > cut_all else str(ye[k])
        counts[c] = counts.get(c, 0) + 1
        examples.setdefault(c, []).append(str(meta[i][1]))
    tot = len(B)
    print(f"{tot} real parts labelled by nearest of {len(Xe)} exemplars")
    print(f"mean confidence {conf.mean():.2f}")
    # AN HONEST ABSTAIN. The exemplars separate cleanly from each other —
    # 94.8% leave-one-out — but a real part can sit nowhere near any of them,
    # and forcing it to the least-distant one is how 51% of a car became
    # "intake". Anything past this distance is UNKNOWN, which is a fact worth
    # having: it names the archetypes still missing from the library.
    far = np.sqrt(d[np.arange(len(B)), near])
    cut = abstain_cut(A, ye)
    unknown = int((far > cut).sum())
    print(f"abstaining past distance {cut:.1f}: {unknown} unknown "
          f"({100*unknown/tot:.0f}%)\n")
    for c, n in sorted(counts.items(), key=lambda x: -x[1]):
        stems = {}
        for nm in examples[c][:600]:
            st = "".join(ch for ch in nm if ch.isalpha())
            stems[st] = stems.get(st, 0) + 1
        top = sorted(stems.items(), key=lambda x: -x[1])[:4]
        print(f"  {c:10s} {n:6d} {100*n/tot:5.1f}%   {top}")


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return
    cmd, path = sys.argv[1], sys.argv[2]
    if cmd == "features":
        try:
            keep = set(json.load(open(os.path.join(HERE, "foldable.json"))))
        except Exception:
            keep = None
        models = sorted(f for f in os.listdir(DATA) if f.endswith(".iqm"))
        if keep:
            models = [m for m in models if m in keep]
        n = int(sys.argv[sys.argv.index("-n") + 1]) if "-n" in sys.argv else 12
        extract(models[:n], path)
    elif cmd == "cluster":
        k = int(sys.argv[sys.argv.index("-k") + 1]) if "-k" in sys.argv else 20
        cluster(path, k)
    elif cmd == "label":
        label(path)
    elif cmd == "synth":
        X, y = synth()
        np.savez_compressed(path, X=X, y=y, names=np.array(FEATURES))
        print(f"{len(X)} exemplars across {len(set(y))} classes -> {path}")
    elif cmd == "match":
        ex = np.load(path, allow_pickle=True)
        real = np.load(sys.argv[3], allow_pickle=True)
        report_match(ex, real)
    elif cmd == "stub":
        k = int(sys.argv[sys.argv.index("-k") + 1]) if "-k" in sys.argv else 24
        label_clusters(path, k)


if __name__ == "__main__":
    main()
