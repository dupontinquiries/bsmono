#!/usr/bin/env python3
"""render_parity.py -- does a model rebuilt from the web look like the original?

Every fidelity number so far has been geometric: quantisation error, occupancy
IoU, cluster residual. None of them answers the question that actually decides
this, which is whether the reconstruction looks the same. This rasterises both
meshes from several viewpoints and compares what a camera would see.

Software rasteriser on purpose — no GPU here, and a depth buffer is all the
comparison needs:

  silhouette IoU -- do they occupy the same pixels? Catches missing parts,
                    wrong scale, and parts placed somewhere they should not be.
  depth RMSE     -- within the shared silhouette, is the surface at the same
                    distance? Catches a shape that has the right outline and
                    the wrong body.

Reported per viewpoint as well as averaged, because a reconstruction can be
convincing head-on and wrong from above, and an average hides exactly that.
"""
import sys, os, json, math, gzip, struct, argparse


def load_obj(path):
    verts, tris = [], []
    with open(path) as f:
        for line in f:
            if line.startswith("v "):
                verts.append([float(x) for x in line.split()[1:4]])
            elif line.startswith("f "):
                idx = [int(p.split("/")[0]) - 1 for p in line.split()[1:]]
                for i in range(1, len(idx) - 1):
                    tris.append((idx[0], idx[i], idx[i + 1]))
    return verts, tris


def load_web(path):
    with gzip.open(path, "rb") as f:
        assert f.read(6) == b"BSWEB\x01"
        nidx, npay = struct.unpack("<II", f.read(8))
        idx = json.loads(f.read(nidx))
        pay = f.read(npay)
    off, o = [], 0
    for g in idx["geom"]:
        off.append((o, o + g["v"], o + g["v"] + g["i"]))
        o += g["v"] + g["i"]
    return idx, pay, off


def part_mesh(part, pay, off):
    vs, ve, ie = off[part["geom"]]
    vb, ib = pay[vs:ve], pay[ve:ie]
    lo, span = part["lo"], part["span"]
    verts = []
    for i in range(part["nv"]):
        q = struct.unpack_from("<3H", vb, i * 6)
        verts.append([lo[d] + q[d] / 65535.0 * span[d] for d in range(3)])
    fmt, sz = ("<3H", 6) if part["isize"] == 2 else ("<3I", 12)
    tris = [struct.unpack_from(fmt, ib, i * sz) for i in range(part["nt"])]
    return verts, tris


def reconstruct(model, placements, idx, pay, off):
    """Assemble a model from its placement list.

    Applies `rot` when the placement carries one (see solve_placements.py) and
    falls back to unrotated when it does not — so the same harness measures both
    states and the difference between them is the answer to "did the solve
    help".
    """
    by_sig = {}
    for p in idx["parts"]:
        if p.get("sig") and p["sig"] not in by_sig:
            by_sig[p["sig"]] = p
    verts, tris, missing = [], [], 0
    for pl in placements:
        part = by_sig.get(pl["sig"])
        if part is None:
            missing += 1
            continue
        pv, pt = part_mesh(part, pay, off)
        # Scale the exemplar to the placement's recorded size, about its own
        # centre, then move it to the placement centroid.
        c = [sum(v[d] for v in pv) / len(pv) for d in range(3)]
        ext = max(max(v[d] for v in pv) - min(v[d] for v in pv) for d in range(3)) or 1.0
        s = (pl["size"] or ext) / ext
        base = len(verts)
        R = pl.get("rot")
        for v in pv:
            q = [(v[d] - c[d]) * s for d in range(3)]
            if R:
                # Rotate about the part's own centre BEFORE translating, or the
                # rotation would swing the part around the model origin and
                # move it somewhere else entirely.
                q = [sum(R[i][j] * q[j] for j in range(3)) for i in range(3)]
            verts.append([q[d] + pl["centroid"][d] for d in range(3)])
        for a, b, cc in pt:
            tris.append((a + base, b + base, cc + base))
    return verts, tris, missing


def render_depth(verts, tris, res, view, bounds):
    """Orthographic depth buffer. `bounds` is shared between both meshes so the
    two renders are directly comparable — normalising each to its own bbox
    would hide a scale error, which is one of the things worth catching."""
    lo, hi = bounds
    span = max(hi[d] - lo[d] for d in range(3)) or 1.0
    ax = {"front": (0, 1, 2), "side": (2, 1, 0), "top": (0, 2, 1)}[view]
    buf = [None] * (res * res)
    for a, b, c in tris:
        pts = []
        for i in (a, b, c):
            if i >= len(verts):
                break
            v = verts[i]
            x = (v[ax[0]] - lo[ax[0]]) / span
            y = (v[ax[1]] - lo[ax[1]]) / span
            z = (v[ax[2]] - lo[ax[2]]) / span
            pts.append((x * (res - 1), (1 - y) * (res - 1), z))
        if len(pts) != 3:
            continue
        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]
        x0, x1 = max(0, int(min(xs))), min(res - 1, int(max(xs)) + 1)
        y0, y1 = max(0, int(min(ys))), min(res - 1, int(max(ys)) + 1)
        d = ((pts[1][0] - pts[0][0]) * (pts[2][1] - pts[0][1])
             - (pts[2][0] - pts[0][0]) * (pts[1][1] - pts[0][1]))
        if abs(d) < 1e-12:
            continue
        for py in range(y0, y1 + 1):
            for px in range(x0, x1 + 1):
                w0 = ((pts[1][0] - px) * (pts[2][1] - py)
                      - (pts[2][0] - px) * (pts[1][1] - py)) / d
                w1 = ((pts[2][0] - px) * (pts[0][1] - py)
                      - (pts[0][0] - px) * (pts[2][1] - py)) / d
                w2 = 1.0 - w0 - w1
                if w0 < 0 or w1 < 0 or w2 < 0:
                    continue
                z = w0 * pts[0][2] + w1 * pts[1][2] + w2 * pts[2][2]
                k = py * res + px
                if buf[k] is None or z < buf[k]:
                    buf[k] = z
    return buf


def compare(a, b):
    inter = sum(1 for x, y in zip(a, b) if x is not None and y is not None)
    union = sum(1 for x, y in zip(a, b) if x is not None or y is not None)
    if not union:
        return 0.0, float("nan")
    if not inter:
        return 0.0, float("nan")
    se = sum((x - y) ** 2 for x, y in zip(a, b) if x is not None and y is not None)
    return inter / union, math.sqrt(se / inter)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--web", required=True)
    ap.add_argument("--lib", required=True, help="partlib.json with placements")
    ap.add_argument("--originals", required=True, help="dir of <model>/<model>.obj")
    ap.add_argument("--res", type=int, default=96)
    ap.add_argument("--limit", type=int, default=5)
    a = ap.parse_args()

    idx, pay, off = load_web(a.web)
    lib = json.load(open(a.lib))
    views = ["front", "side", "top"]
    rows = []
    for name, pls in list(lib.get("placements", {}).items())[:a.limit]:
        src = os.path.join(a.originals, name, name + ".obj")
        if not os.path.exists(src):
            continue
        ov, ot = load_obj(src)
        rv, rt, missing = reconstruct(name, pls, idx, pay, off)
        if not ov or not rv:
            continue
        allv = ov + rv
        bounds = ([min(v[d] for v in allv) for d in range(3)],
                  [max(v[d] for v in allv) for d in range(3)])
        per = []
        for view in views:
            da = render_depth(ov, ot, a.res, view, bounds)
            db = render_depth(rv, rt, a.res, view, bounds)
            per.append(compare(da, db))
        rows.append((name, len(pls), missing, per))

    print("%-42s %5s %5s %s" % ("model", "place", "miss",
                                "  ".join("%s IoU/RMSE" % v for v in views)))
    for name, npl, miss, per in rows:
        cells = "  ".join("%.3f/%.3f" % (i, r) for i, r in per)
        print("%-42s %5d %5d  %s" % (name[:42], npl, miss, cells))
    if rows:
        m = sum(p[0] for r in rows for p in r[3]) / (len(rows) * len(views))
        print("\nmean silhouette IoU across %d models x %d views: %.3f"
              % (len(rows), len(views), m))
    return 0


if __name__ == "__main__":
    sys.exit(main())
