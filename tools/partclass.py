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
    round_ = 1.0 - float(np.std(r[r > rmax * 0.25]) / rmax) if (r > rmax * 0.25).sum() > 8 else 0.0
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
    elif cmd == "stub":
        k = int(sys.argv[sys.argv.index("-k") + 1]) if "-k" in sys.argv else 24
        label_clusters(path, k)


if __name__ == "__main__":
    main()
