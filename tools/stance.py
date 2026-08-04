#!/usr/bin/env python3
"""stance.py -- which way up is a legged bot, from its own support geometry.

Your rule, made mechanical: a body that stands has SEVERAL CONTACT POINTS AT
THE BOTTOM, and they are symmetric about the centreline. Put it on its head
and there is one point of contact. Put it on its side and the symmetric pairs
sit horizontally instead of underneath.

So the test is not "which way looks right" — it is: over the six axis-aligned
orientations, which one puts the most symmetric support underneath?

  contacts    distinct clusters of geometry in the bottom band
  symmetry    do those clusters pair across the centreline
  spread      how far apart they are, as a fraction of the body — a stance
              with all its feet in one place is not a stance
  headroom    mass should sit ABOVE the contacts, not below them

Scored together, because each alone is fooled: a car has wide flat contact
and no legs; an upside-down spider still has symmetric clusters, just at the
top; a body on its side has two contacts and no headroom.
"""
import json, subprocess, sys, os
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
BS = os.path.join(HERE, "..", "..", "blackstar")
FV = os.path.join(BS, "rust", "foldview", "target", "release", "foldview")
DATA = os.path.join(BS, "engine_src", "data")

# the six axis-aligned ways a body can be set down
ORIENTS = {
    "as exported": np.eye(3),
    "roll 180":    np.diag([1.0, -1.0, -1.0]),
    "roll +90":    np.array([[1, 0, 0], [0, 0, -1], [0, 1, 0]], float),
    "roll -90":    np.array([[1, 0, 0], [0, 0, 1], [0, -1, 0]], float),
    "pitch +90":   np.array([[0, -1, 0], [1, 0, 0], [0, 0, 1]], float),
    "pitch -90":   np.array([[0, 1, 0], [-1, 0, 0], [0, 0, 1]], float),
}


def points(iqm, t=1.0):
    out = "/tmp/_stance"
    import shutil
    shutil.rmtree(out, ignore_errors=True)
    r = subprocess.run([FV, "--data", DATA, "--content", os.path.join(BS, "content"),
                        "--veh", os.path.join(DATA, iqm), "--repair", "--reattach",
                        "--gltf", out, "--t", str(t)], capture_output=True, timeout=240)
    if r.returncode != 0 or not os.path.exists(f"{out}/scene.gltf"):
        raise RuntimeError(f"export failed: {iqm}")
    g = json.load(open(f"{out}/scene.gltf")); buf = open(f"{out}/scene.bin", "rb").read()
    ps = []
    for n in g["nodes"]:
        a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        ps.append(np.frombuffer(buf, "<f4", count=a["count"] * 3, offset=off).reshape(-1, 3))
    return np.vstack(ps).astype(float)


def clusters(xz, cell):
    """8-connected blobs on a grid — the contact points."""
    keys = {(int(p[0] // cell), int(p[1] // cell)) for p in xz}
    seen, blobs = set(), []
    for k in keys:
        if k in seen:
            continue
        stack, cells = [k], []
        seen.add(k)
        while stack:
            c = stack.pop(); cells.append(c)
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    q = (c[0] + dx, c[1] + dy)
                    if q in keys and q not in seen:
                        seen.add(q); stack.append(q)
        if len(cells) >= 2:
            blobs.append(np.mean(cells, axis=0) * cell)
    return blobs


def score(P):
    ext = P.max(0) - P.min(0)
    lo = P.min(0)
    band = P[P[:, 1] < lo[1] + ext[1] * 0.14]
    if len(band) < 20:
        return 0.0, 0, 0.0
    cell = max(ext[0], ext[2]) * 0.06
    if cell <= 0:
        return 0.0, 0, 0.0
    bl = clusters(band[:, [0, 2]], cell)
    n = len(bl)
    if n == 0:
        return 0.0, 0, 0.0
    mid_x = (P[:, 0].max() + P[:, 0].min()) * 0.5
    # SYMMETRY: does each contact have a partner mirrored in x
    tol = max(ext[0], ext[2]) * 0.16
    paired = 0
    for b in bl:
        m = np.array([2 * mid_x - b[0], b[1]])
        if any(np.linalg.norm(np.array(o) - m) < tol for o in bl):
            paired += 1
    sym = paired / n
    # SPREAD: feet in one place is not a stance
    if n > 1:
        arr = np.array(bl)
        spread = float(np.ptp(arr, axis=0).max()) / max(ext[0], ext[2], 1e-9)
    else:
        spread = 0.0
    # HEADROOM: mass above the contacts, not below
    head = float((P[:, 1] > lo[1] + ext[1] * 0.5).mean())
    # SLENDERNESS: legs are thin things reaching DOWN. A body lying on its
    # side has contacts too — its flank — and they are wide and continuous
    # rather than a few narrow points. So measure how much of the bottom band
    # is actually occupied: real feet cover a little of a wide footprint,
    # a flank covers most of it.
    fp_area = max(np.ptp(band[:, 0]), 1e-9) * max(np.ptp(band[:, 2]), 1e-9)
    covered = len(clusters(band[:, [0, 2]], cell)) * (cell * cell)
    slender = 1.0 - min(covered / max(fp_area, 1e-9), 1.0)
    s = (min(n, 6) / 6.0) * 0.28 + sym * 0.24 + min(spread, 1.0) * 0.14 \
        + head * 0.24 + slender * 0.10
    return s, n, sym


def verdict(rows, margin=0.12):
    """ONLY CALL IT WHEN IT IS CLEAR. The rule is about legged bodies and a
    hoverer has no legs — rolling a flat hull on its side also produces
    contacts, just meaningless ones. So a correction is reported only when
    the winner beats "as exported" by a real margin AND has more support
    under it, and otherwise the answer is "leave it alone"."""
    top = rows[0]
    cur = next(r for r in rows if r[1] == "as exported")
    if top[1] == "as exported":
        return None, top[0] - rows[1][0]
    better_support = top[2] > cur[2] and top[3] >= cur[3]
    if top[0] - cur[0] >= margin and better_support:
        return top[1], top[0] - cur[0]
    return None, top[0] - cur[0]


def best(iqm):
    P = points(iqm)
    P = P - P.mean(0)
    rows = []
    for name, R in ORIENTS.items():
        Q = P @ R.T
        s, n, sym = score(Q)
        rows.append((s, name, n, sym))
    rows.sort(reverse=True)
    return rows


if __name__ == "__main__":
    for iqm in sys.argv[1:]:
        if not iqm.endswith(".iqm"):
            iqm = f"imported_{iqm}.iqm"
        try:
            rows = best(iqm)
        except Exception as e:
            print(f"{iqm[:44]:44s} ERR {e}"); continue
        print(f"\n{iqm.replace('imported_','')[:52]}")
        for s, name, n, sym in rows:
            print(f"   {s:.3f}  {name:12s} contacts {n:2d} symmetry {sym:.2f}")
        call, marg = verdict(rows)
        print(f"   -> {('APPLY ' + call) if call else 'leave as exported'}  (margin {marg:+.3f})")
