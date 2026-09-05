#!/usr/bin/env python3
"""pair_dataset.py — a frame-aligned low/high-bitrate pair of the same movie as a
restoration training set, plus the scorer that judges any output against it.

    pair_dataset.py extract lo.mkv hi.mkv out/ --every 2.0 --count 200 --crop 256
    pair_dataset.py score   lo.rgb hi.rgb [den.rgb ...] --size 1920x1080

`extract` decodes both files at the same timestamps (accurate seek), verifies the
pair is aligned (PSNR to the matching frame must beat PSNR to its neighbours by a
margin), and writes `out/NNNNN_lo.png` / `out/NNNNN_hi.png` random crops. Train
an SRVGG-class net on those (input lo, target hi; x1 for restoration, or target
the x2/x4 upscale of hi for joint restoration+SR), export with tools/pth2st.py
and run it as a cascade tier.

`score` prints per-frame and mean PSNR / SSIM of each rgb24 file against hi.rgb
— the same numbers d2x-core/examples/tdenoise was judged by (numpy only; SSIM is
an 8x8 block approximation, fine for A/B).
"""
import argparse, os, random, subprocess, sys
import numpy as np

def decode(path, t, n, w, h):
    cmd = ["ffmpeg", "-v", "error", "-ss", f"{t:.3f}", "-i", path, "-frames:v", str(n), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    return np.frombuffer(raw, dtype=np.uint8).reshape(-1, h, w, 3)

def probe(path):
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height:format=duration", "-of", "csv=p=0", path], capture_output=True, text=True, check=True).stdout.split()
    w, h = map(int, out[0].split(",")[:2]); dur = float(out[-1].split(",")[-1])
    return w, h, dur

def psnr(a, b):
    m = ((a.astype(np.float64) - b) ** 2).mean()
    return 99.0 if m == 0 else 10 * np.log10(255 ** 2 / m)

def luma(x):
    return 0.299 * x[..., 0] + 0.587 * x[..., 1] + 0.114 * x[..., 2]

def ssim(a, b):
    A, B = luma(a).astype(np.float64), luma(b).astype(np.float64)
    H, W = A.shape; H, W = H // 8 * 8, W // 8 * 8
    A = A[:H, :W].reshape(H // 8, 8, W // 8, 8); B = B[:H, :W].reshape(H // 8, 8, W // 8, 8)
    ma, mb = A.mean((1, 3)), B.mean((1, 3)); va, vb = A.var((1, 3)), B.var((1, 3))
    cov = ((A - ma[:, None, :, None]) * (B - mb[:, None, :, None])).mean((1, 3))
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    return (((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma ** 2 + mb ** 2 + c1) * (va + vb + c2))).mean()

def cmd_extract(a):
    w, h, dur = probe(a.hi)
    os.makedirs(a.out, exist_ok=True)
    rng = random.Random(a.seed); n = 0; t = a.start
    while t + 0.2 < dur and n < a.count:
        lo = decode(a.lo, t, 3, w, h); hi = decode(a.hi, t, 3, w, h)
        if lo.shape[0] < 3 or hi.shape[0] < 3:
            break
        # alignment: lo[1] must match hi[1] clearly better than hi[0]/hi[2]
        p = [psnr(lo[1], hi[k]) for k in range(3)]
        if p[1] - max(p[0], p[2]) < a.margin:
            print(f"t={t:.2f}s skipped: alignment unclear (PSNR to t-1,t,t+1 = {p[0]:.1f} {p[1]:.1f} {p[2]:.1f})", file=sys.stderr)
        else:
            for _ in range(a.crops):
                y = rng.randrange(0, h - a.crop + 1); x = rng.randrange(0, w - a.crop + 1)
                from PIL import Image
                Image.fromarray(lo[1, y:y + a.crop, x:x + a.crop]).save(f"{a.out}/{n:05d}_lo.png")
                Image.fromarray(hi[1, y:y + a.crop, x:x + a.crop]).save(f"{a.out}/{n:05d}_hi.png")
                n += 1
            print(f"t={t:.2f}s ok ({p[1]:.1f} dB), {n} crops", file=sys.stderr)
        t += a.every
    print(f"{n} crop pairs in {a.out}")

def cmd_score(a):
    w, h = map(int, a.size.split("x"))
    hi = np.fromfile(a.hi, dtype=np.uint8).reshape(-1, h, w, 3)
    for path in [a.lo] + a.others:
        x = np.fromfile(path, dtype=np.uint8).reshape(-1, h, w, 3)
        n = min(len(x), len(hi))
        ps = [psnr(x[t], hi[t]) for t in range(n)]; ss = [ssim(x[t], hi[t]) for t in range(n)]
        print(f"{path}: {n} frames  PSNR mean {np.mean(ps):.2f} min {min(ps):.2f} @ {int(np.argmin(ps))}  SSIM mean {np.mean(ss):.4f} min {min(ss):.4f}")
        if a.per_frame:
            print("  " + " ".join(f"{v:.1f}" for v in ps))

if __name__ == "__main__":
    p = argparse.ArgumentParser(); sub = p.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("extract"); e.add_argument("lo"); e.add_argument("hi"); e.add_argument("out")
    e.add_argument("--every", type=float, default=2.0); e.add_argument("--start", type=float, default=1.0)
    e.add_argument("--count", type=int, default=200); e.add_argument("--crop", type=int, default=256)
    e.add_argument("--crops", type=int, default=2); e.add_argument("--margin", type=float, default=6.0); e.add_argument("--seed", type=int, default=1)
    s = sub.add_parser("score"); s.add_argument("lo"); s.add_argument("hi"); s.add_argument("others", nargs="*")
    s.add_argument("--size", default="1920x1080"); s.add_argument("--per-frame", action="store_true")
    a = p.parse_args(); (cmd_extract if a.cmd == "extract" else cmd_score)(a)
