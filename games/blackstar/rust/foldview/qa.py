#!/usr/bin/env python3
"""foldview QA: eyes made of numpy.

Renders every category model at t=0, 0.5, 1 and measures, from the actual
pixels: silhouette blob count (cohesion — multiple blobs means something
visually flew off the body), and left/right symmetry IoU at t=1 (humanoid
read). Ranks worst-first so hand-tuning chases evidence.
"""
import subprocess, sys, os, glob
import numpy as np
from PIL import Image

BIN = "./target/release/foldview"
DATA = "../../engine_src/data"
CATS = ["flyer", "tank", "hoverer", "hovertank", "racer", "flyerf"]
W, H = 320, 180
TMP = "/tmp/fvqa"

def mask_of(path):
    im = np.array(Image.open(path).convert("RGB")).astype(int)
    h, w, _ = im.shape
    ys = np.arange(h)
    k = 14 + 26 * ys / h
    bg = np.stack([k, k + 2, k + 6], axis=1)[:, None, :]
    return np.abs(im - bg).sum(axis=2) > 18

def blobs(mask, min_frac=0.01):
    lab = np.zeros(mask.shape, dtype=int)
    cur = 0
    total = mask.sum()
    if total == 0:
        return 0
    sizes = []
    for sy in range(mask.shape[0]):
        for sx in range(mask.shape[1]):
            if mask[sy, sx] and lab[sy, sx] == 0:
                cur += 1
                stack = [(sy, sx)]
                lab[sy, sx] = cur
                size = 0
                while stack:
                    y, x = stack.pop()
                    size += 1
                    for dy, dx in ((1,0),(-1,0),(0,1),(0,-1),(1,1),(1,-1),(-1,1),(-1,-1)):
                        ny, nx = y + dy, x + dx
                        if 0 <= ny < mask.shape[0] and 0 <= nx < mask.shape[1] \
                           and mask[ny, nx] and lab[ny, nx] == 0:
                            lab[ny, nx] = cur
                            stack.append((ny, nx))
                sizes.append(size)
    return sum(1 for s in sizes if s >= total * min_frac)

def symmetry(mask):
    if mask.sum() == 0:
        return 0.0
    xs = np.where(mask.any(axis=0))[0]
    cx = (xs.min() + xs.max()) // 2
    w = mask.shape[1]
    shift = int(2 * cx - (w - 1))
    mirrored = np.fliplr(mask)
    mirrored = np.roll(mirrored, shift, axis=1)
    inter = (mask & mirrored).sum()
    union = (mask | mirrored).sum()
    return inter / max(1, union)

def main():
    os.makedirs(TMP, exist_ok=True)
    rows = []
    files = []
    for c in CATS:
        files += sorted(glob.glob(os.path.join(DATA, f"imported_{c}_*.iqm")))
    only = sys.argv[1:] if len(sys.argv) > 1 else None
    for f in files:
        base = os.path.basename(f)
        if only and base not in only:
            continue
        ms = {}
        ok = True
        for t in (0.0, 0.5, 1.0):
            out = os.path.join(TMP, f"{base}.{t}.png")
            r = subprocess.run([BIN, "--data", DATA, "--veh", f, "--frames", "0",
                                "--t", str(t), "--size", f"{W}x{H}", "--out", out],
                               capture_output=True)
            if r.returncode != 0 or not os.path.exists(out):
                ok = False
                break
            ms[t] = mask_of(out)
        if ok:
            # symmetry judged from a true FRONT view of the finished robot
            outf = os.path.join(TMP, f"{base}.front.png")
            r = subprocess.run([BIN, "--data", DATA, "--veh", f, "--frames", "0",
                                "--t", "1.0", "--front", "--size", f"{W}x{H}", "--out", outf],
                               capture_output=True)
            if r.returncode == 0 and os.path.exists(outf):
                ms["front"] = mask_of(outf)
        if not ok:
            rows.append((base, -1, -1, -1, 0.0, 0.0))
            continue
        fm = ms.get("front", ms[1.0])
        b1w = max(blobs(ms[1.0]), blobs(fm))
        # grounded: fraction of silhouette columns whose lowest pixel sits in
        # the bottom band — does the robot stand ON something
        g = 0.0
        if fm.sum() > 0:
            cols = [fm[:, x].nonzero()[0] for x in range(fm.shape[1])]
            bots = [c.max() for c in cols if len(c) > 0]
            floor = max(bots)
            near = sum(1 for b in bots if b >= floor - fm.shape[0] * 0.06)
            g = near / max(1, len(bots))
        rows.append((base, blobs(ms[0.0]), blobs(ms[0.5]), b1w, symmetry(fm), g))
    # worst first: end-pose blobs, then low symmetry (mid-fold blobs are
    # the burst — intentional now, reported as spread not penalized)
    rows.sort(key=lambda r: (-r[3], r[4]))
    print(f"{'model':<52} b@0 burst b@1w sym  gnd")
    for name, b0, bm, b1, sy, g in rows:
        print(f"{name:<52} {b0:>3} {bm:>4} {b1:>3}  {sy:.2f} {g:.2f}")
    bad = [r for r in rows if r[3] > 1]
    good = [r for r in rows if r[3] == 1]
    print(f"\ncohesive at t=1 (both views): {len(good)}/{len(rows)}   multi-blob: {len(bad)}")
    if rows:
        ok = [r for r in rows if r[3] >= 0]
        print(f"mean symmetry: {np.mean([r[4] for r in ok]):.2f}   mean grounded: {np.mean([r[5] for r in ok]):.2f}")

if __name__ == "__main__":
    main()
