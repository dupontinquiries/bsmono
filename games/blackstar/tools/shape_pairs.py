#!/usr/bin/env python3
"""shape_pairs.py -- build (blob -> detail) training pairs for a 3D upscaler.

The idea is the user's: take the models that already look right (Scorponok,
Soundwave, Megatron, Laserbeak, Brawl), degrade them into blobs, and learn the
mapping back. Degrading is free and exact, so the pairs are self-supervised -- no
labelling, and the detail comes from real Transformers geometry rather than
foreign parts bolted on.

ENCODING. A component is rendered as six orthographic DEPTH views (+X,-X,+Y,-Y,
+Z,-Z) at 64x64, tiled 2 rows x 4 cols = 128x256, single channel, values in
[-1,1]. That is deliberately the same tensor shape and range as the kBM3 mel
cache, so the existing Rust DCGAN reads it with no change to data.rs.

BLOB DEGRADATION mirrors what a low-detail model actually looks like: collapse the
component to its coarse hull and re-voxelise, destroying surface structure while
keeping gross proportions.

Output:
  out/pairs.npz        blob[N,128,256], detail[N,128,256], meta
  out/preview/*.pgm    a few pairs as images, to eyeball the encoding

Usage:
  python3 tools/shape_pairs.py --donors donors/ --out pairs --min-tris 50
"""
import sys, os, json, argparse

VIEWS = [(0, 1, 2, +1), (0, 1, 2, -1),      # look along Z
         (2, 1, 0, +1), (2, 1, 0, -1),      # along X
         (0, 2, 1, +1), (0, 2, 1, -1)]      # along Y

def depth_view(V, F, u, v, w, sign, res):
    """Orthographic depth map: for each pixel, the nearest surface along w.

    Rasterises triangle centroids with a small footprint -- exact scan conversion
    is unnecessary at 64x64 and much slower."""
    import numpy as np
    img = np.full((res, res), -1.0, np.float32)
    if len(F) == 0: return img
    tri = V[F]
    c = tri.mean(1)
    r = np.max(np.abs(tri - c[:, None, :]), axis=(1, 2))
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    pu = (c[:, u] - lo[u]) / span[u]
    pv = (c[:, v] - lo[v]) / span[v]
    pw = (c[:, w] - lo[w]) / span[w]
    if sign < 0: pw = 1.0 - pw
    rad = np.maximum(1, (r / span[[u, v]].mean() * res * 0.6).astype(int))
    order = np.argsort(-pw)                      # far to near, so near overwrites
    for i in order:
        x = int(pu[i] * (res - 1)); y = int(pv[i] * (res - 1)); k = int(rad[i])
        x0, x1 = max(0, x - k), min(res, x + k + 1)
        y0, y1 = max(0, y - k), min(res, y + k + 1)
        d = pw[i] * 2.0 - 1.0
        img[y0:y1, x0:x1] = np.maximum(img[y0:y1, x0:x1], d)
    return img

def encode(V, F, res=64):
    """Six views tiled 2x4 -> (128, 256) in [-1,1]."""
    import numpy as np
    tiles = [depth_view(V, F, u, v, w, s, res) for (u, v, w, s) in VIEWS]
    tiles += [np.full((res, res), -1.0, np.float32)] * (8 - len(tiles))
    rows = [np.concatenate(tiles[0:4], axis=1), np.concatenate(tiles[4:8], axis=1)]
    return np.concatenate(rows, axis=0)

def blobify(V, F, pm=None, ratio=12, grid=6, mode="decimate"):
    """Degrade a component into the coarse form a LOW-POLY model would have.

    mode="decimate" (default) reduces the triangle count, which is what a
    low-detail model actually is. The first version voxelised into cubes instead,
    and measurement showed that going the WRONG WAY: the voxel blob carried MORE
    depth variation than the original (0.506 vs 0.415) because cube edges are hard
    steps. Training on that would teach the model to remove blockiness, not to add
    detail. mode="voxel" keeps the old behaviour for comparison."""
    import numpy as np
    if mode == "decimate" and pm is not None and len(F) > 24:
        try:
            Vd, Fd = pm._decimate(np.asarray(V, "f4"), np.asarray(F, "u4"),
                                  max(12, len(F) // max(2, ratio)))
            if len(Fd) >= 4:
                return np.asarray(Vd, "f4"), np.asarray(Fd, "u4")
        except Exception:
            pass
    lo, hi = V.min(0), V.max(0)
    span = np.maximum(hi - lo, 1e-6)
    idx = np.clip(((V - lo) / span * grid).astype(int), 0, grid - 1)
    occ = set(map(tuple, idx))
    Vs, Fs, off = [], [], 0
    cell = span / grid
    for (i, j, k) in occ:
        o = lo + np.array([i, j, k], "f4") * cell
        c = np.array([(0,0,0),(1,0,0),(1,1,0),(0,1,0),
                      (0,0,1),(1,0,1),(1,1,1),(0,1,1)], "f4") * cell + o
        f = np.array([(0,2,1),(0,3,2),(4,5,6),(4,6,7),(0,1,5),(0,5,4),
                      (1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,0,4),(3,4,7)], "u4")
        Vs.append(c); Fs.append(f + off); off += 8
    if not Vs: return V, F
    return np.vstack(Vs).astype("f4"), np.vstack(Fs).astype("u4")

def write_pgm(path, img):
    import numpy as np
    a = ((np.clip(img, -1, 1) + 1) * 127.5).astype("u1")
    with open(path, "wb") as fh:
        fh.write(b"P5\n%d %d\n255\n" % (a.shape[1], a.shape[0]))
        fh.write(a.tobytes())

def main():
    import numpy as np
    ap = argparse.ArgumentParser()
    ap.add_argument("--donors", required=True, help="folder of high-detail models")
    ap.add_argument("--out", default="pairs")
    ap.add_argument("--min-tris", type=int, default=50)
    ap.add_argument("--degrade", choices=("decimate", "voxel"), default="decimate",
                    help="how to make the low-detail side. decimate = fewer triangles "
                         "(what low-poly really is); voxel = cubes (measurably wrong)")
    ap.add_argument("--ratio", type=int, default=12,
                    help="decimation factor for the low-detail side")
    ap.add_argument("--grid", type=int, default=6, help="voxel resolution if --degrade voxel")
    ap.add_argument("--preview", type=int, default=8)
    a = ap.parse_args()

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
    pm, ph = imp("process_models"), imp("part_harvest")
    if not (pm and ph): raise SystemExit("need process_models.py and part_harvest.py")

    models = []
    for dp, dirs, files in os.walk(a.donors):
        dirs[:] = [d for d in dirs if d not in {"parts", "iqm", "build"}]
        cand = [f for f in files if f.lower().endswith((".gltf", ".glb", ".obj"))]
        if cand:
            def sz(f):
                try: return os.path.getsize(os.path.join(dp, f))
                except OSError: return 0
            models.append((os.path.basename(dp), os.path.join(dp, max(cand, key=sz))))
    if not models: raise SystemExit("no models under " + a.donors)

    os.makedirs(a.out, exist_ok=True)
    prev = os.path.join(a.out, "preview"); os.makedirs(prev, exist_ok=True)
    blobs, details, meta = [], [], []
    for name, path in models:
        geo = pm.load_geometry(path)
        if not geo: print("  %-34s FAIL" % name[:34]); continue
        V, F = np.asarray(geo[0], "f4"), np.asarray(geo[1], "u4")
        comps = ph.components(V, F.astype(np.int64), min_tris=a.min_tris)
        n = 0
        for (cv, cf) in comps:
            lo, hi = cv.min(0), cv.max(0)
            s = float((hi - lo).max()) or 1.0
            Vd = ((cv - (lo + hi) * 0.5) / s).astype("f4")     # unit-normalised
            Vb, Fb = blobify(Vd, cf, pm, a.ratio, a.grid, a.degrade)
            details.append(encode(Vd, cf))
            blobs.append(encode(Vb, Fb))
            meta.append({"model": name, "tris": int(len(cf))})
            n += 1
        print("  %-34s %5d components" % (name[:34], n))

    if not blobs: raise SystemExit("no components extracted")
    B = np.stack(blobs).astype("f4"); D = np.stack(details).astype("f4")
    np.savez_compressed(os.path.join(a.out, "pairs.npz"), blob=B, detail=D,
                        meta=json.dumps(meta))
    # raw f32 pair stream for the Rust trainer: no numpy dependency on that side.
    # layout: [blob_0 (128*256 f32) | detail_0 | blob_1 | detail_1 | ...]
    with open(os.path.join(a.out, "pairs.f32"), "wb") as fh:
        for i in range(len(B)):
            fh.write(B[i].astype("<f4").tobytes())
            fh.write(D[i].astype("<f4").tobytes())
    json.dump({"count": int(len(B)), "height": int(B.shape[1]),
               "width": int(B.shape[2]), "layout": "blob,detail interleaved f32 LE",
               "range": [-1, 1]},
              open(os.path.join(a.out, "pairs.json"), "w"), indent=1)
    for i in range(min(a.preview, len(B))):
        write_pgm(os.path.join(prev, "%03d_blob.pgm" % i), B[i])
        write_pgm(os.path.join(prev, "%03d_detail.pgm" % i), D[i])

    occupied = float((D > -0.99).mean())
    print("\n%d pairs, tensors %s in [%.2f, %.2f]" % (len(B), B.shape[1:], D.min(), D.max()))
    print("  detail maps %.0f%% occupied (rest is empty background)" % (100 * occupied))
    raw = os.path.getsize(os.path.join(a.out, "pairs.f32"))
    print("  wrote %s/pairs.npz, pairs.f32 (%.0f MB raw) + %d previews"
          % (a.out, raw / 1e6, 2 * min(a.preview, len(B))))
    print("\n  shape (128,256) in [-1,1] matches the kBM3 mel cache, so the existing")
    print("  Rust trainer reads these without touching data.rs.")

if __name__ == "__main__":
    main()
