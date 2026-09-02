#!/usr/bin/env python3
"""flow_oracle — bound what a dense-flow / mesh motion model can buy.

    tools/flow_oracle.py <src.y4m> <recon.y4m> <out_prefix> [grid ...]

Builds prediction y4ms for `bench` (FAO_PRED_Y4M):
  <prefix>_oracle.y4m   previous RECON warped by DIS dense flow
                        (source->source), flow uncharged: the ceiling
                        for any motion representation
  <prefix>_mesh<G>.y4m  the same flow sampled on a G px grid,
                        quarter-pel, bilinear back to dense; its wire
                        cost printed as delta-coded zstd (a proxy for
                        the context coder)
Also prints the non-rigid mask statistics: pixels deviating >1.5 px
from a RANSAC global affine of the flow, opened, blobs >2000 px.

Requires opencv-python-headless and zstandard. Recon comes from
plate_lab <src> <recon.y4m>. Measured 2026-09-02 on w1: oracle -40%
bytes at +0.0022 MS-SSIM; mesh at 32 px -5.4% / +0.0016 after paying
1.76 KB/frame for the flow; non-rigid region 42% of the frame in ~13
blobs — not a box.
"""
import sys, numpy as np, cv2
try:
    import zstandard
except ImportError:
    zstandard = None

src_p, rec_p, prefix = sys.argv[1:4]
grids = [int(g) for g in sys.argv[4:]] or [32]

def read_y4m(p):
    f = open(p, 'rb'); hdr = f.readline().decode()
    W = int([t for t in hdr.split() if t[0] == 'W'][0][1:]); H = int([t for t in hdr.split() if t[0] == 'H'][0][1:])
    ysz, csz = W * H, (W // 2) * (H // 2); out = []
    while True:
        l = f.readline()
        if not l: break
        y = np.frombuffer(f.read(ysz), np.uint8).reshape(H, W)
        u = np.frombuffer(f.read(csz), np.uint8).reshape(H // 2, W // 2)
        v = np.frombuffer(f.read(csz), np.uint8).reshape(H // 2, W // 2)
        out.append((y, u, v))
    return W, H, out

W, H, src = read_y4m(src_p); _, _, rec = read_y4m(rec_p)
dis = cv2.DISOpticalFlow_create(cv2.DISOPTICAL_FLOW_PRESET_MEDIUM)
gx, gy = np.meshgrid(np.arange(W, dtype=np.float32), np.arange(H, dtype=np.float32))
hdr = f"YUV4MPEG2 W{W} H{H} F24:1 Ip A1:1 C420\n".encode()

def warp(prev, mx, my):
    y = cv2.remap(prev[0], mx, my, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    mxc = cv2.resize(mx, (W // 2, H // 2)) / 2; myc = cv2.resize(my, (W // 2, H // 2)) / 2
    u = cv2.remap(prev[1], mxc, myc, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    v = cv2.remap(prev[2], mxc, myc, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    return y, u, v

def write(out, yuv):
    out.write(b"FRAME\n")
    for p in yuv: out.write(np.ascontiguousarray(p).tobytes())

flows = [None] + [dis.calc(src[i][0], src[i - 1][0], None) for i in range(1, len(src))]

# oracle + mask stats
out = open(f"{prefix}_oracle.y4m", 'wb'); out.write(hdr); stats = []
for i, fl in enumerate(flows):
    if fl is None: write(out, rec[0]); continue
    mx, my = gx + fl[..., 0], gy + fl[..., 1]
    write(out, warp(rec[i - 1], mx, my))
    pts = np.ascontiguousarray(np.stack([gx.ravel(), gy.ravel()], 1)[::97]).astype(np.float32)
    dst = np.ascontiguousarray(np.stack([mx.ravel(), my.ravel()], 1)[::97]).astype(np.float32)
    A, inl = cv2.estimateAffine2D(pts, dst, ransacReprojThreshold=1.0)
    dev = np.hypot(mx - (A[0, 0] * gx + A[0, 1] * gy + A[0, 2]), my - (A[1, 0] * gx + A[1, 1] * gy + A[1, 2]))
    mask = cv2.morphologyEx((dev > 1.5).astype(np.uint8), cv2.MORPH_OPEN, np.ones((9, 9), np.uint8))
    n, lab, st, _ = cv2.connectedComponentsWithStats(mask)
    big = [s for s in st[1:] if s[4] > 2000]
    stats.append((sum(s[4] for s in big) / (W * H), len(big), inl.mean()))
out.close()
a = np.array(stats)
print(f"non-rigid mask: {a[:,0].mean()*100:.1f}% of frame in {a[:,1].mean():.1f} blobs; global-affine inliers {a[:,2].mean()*100:.0f}%")

for G in grids:
    if W % G or H % G:
        print(f"grid {G}: frame not divisible, skipped"); continue
    out = open(f"{prefix}_mesh{G}.y4m", 'wb'); out.write(hdr); tot = 0
    for i, fl in enumerate(flows):
        if fl is None: write(out, rec[0]); continue
        q = np.round(fl.reshape(H // G, G, W // G, G, 2).mean(axis=(1, 3)) * 4).astype(np.int16)
        d = q.copy(); d[:, 1:] -= q[:, :-1]
        raw = d.astype(np.int8).tobytes()
        tot += len(zstandard.ZstdCompressor(level=19).compress(raw)) if zstandard else len(raw)
        up = cv2.resize(q.astype(np.float32) / 4, (W, H), interpolation=cv2.INTER_LINEAR)
        write(out, warp(rec[i - 1], gx + up[..., 0], gy + up[..., 1]))
    out.close(); print(f"grid {G}: flow bytes {tot} ({tot // max(1, len(src) - 1)} B/frame)")
