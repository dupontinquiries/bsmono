#!/usr/bin/env python3
"""Train and VERIFY the "AI half-rate" frame synthesiser.

The lever it completes: with non-reference frames skipped (vidkit's fps-cap
path, -41..46% of AV1 decode CPU), every other frame is never decoded. This
predicts each skipped frame from the two decoded frames around it -- the
reference graph's own shape: a non-reference frame sits between the frames
it would have been predicted from -- so the display still gets every frame.

Design, and why (each step was forced by a measurement, see NOTES.md):

1. Motion: a symmetric Lucas-Kanade step at 1/16, 1/8 and 1/4 of the frame
   (warp both neighbours to the middle, solve a 2x2 system per pixel over a
   5x5 box, refine). Closed form, nothing learned. A pure CNN this small
   could NOT learn displacement -- it is a division (difference over
   gradient) -- and collapsed to zero motion, the blend, in every
   configuration tried.
2. A small dilated CNN (dilations 1,2,4,8; 8 channels) at 1/4 scale refines
   that flow and learns the blend mask (occlusions, where one side is
   right). Its output layer starts at zero: it begins exactly at the
   Lucas-Kanade result and training only moves it where that helps.
3. Full resolution: one bilinear warp of each neighbour and a blend -- four
   texture fetches a pixel in the player's shader.

Verification: PSNR of the synthesised frames against the REAL frames, on a
held-out stretch never trained on, next to what the player could do instead
-- repeat the previous frame (what a 30 fps cap shows), a 50/50 blend -- and
the analytic flow alone (did the CNN earn its cost?). Scene cuts are
reported apart: no method interpolates across a cut from its neighbours,
and encoders put a key frame at a cut anyway.

Usage: train.py FRAMES.yuv W H [out_dir]   (yuv420p, 60 fps content)
"""
import json
import math
import os
import sys
import time

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

torch.manual_seed(1)
np.random.seed(1)
torch.set_num_threads(os.cpu_count() or 1)

LEVELS = [16, 8, 4]  # coarse to fine, divisors of the frame size
LK_ITERS = 2
LK_WIN = 5
CH = int(os.environ.get("CH", "8"))  # the "ultra low power" knob
DIL = [1, 2, 4, 8]


def load(path, w, h):
    fs = w * h * 3 // 2
    raw = np.fromfile(path, dtype=np.uint8)
    n = raw.size // fs
    raw = raw[: n * fs].reshape(n, fs)
    y = raw[:, : w * h].reshape(n, h, w)
    u = raw[:, w * h : w * h + (w // 2) * (h // 2)].reshape(n, h // 2, w // 2)
    v = raw[:, w * h + (w // 2) * (h // 2) :].reshape(n, h // 2, w // 2)
    return y, u, v


def warp(img, flow):
    """Backward warp: out(x) = img(x + flow(x)); flow in pixels (N,2,H,W).
    Border clamp, texel-centre convention: what texture() with
    CLAMP_TO_EDGE and LINEAR filtering does in the shader."""
    n, _, h, w = img.shape
    ys, xs = torch.meshgrid(torch.arange(h, dtype=torch.float32), torch.arange(w, dtype=torch.float32), indexing="ij")
    gx = (xs[None] + flow[:, 0] + 0.5) * (2.0 / w) - 1.0
    gy = (ys[None] + flow[:, 1] + 0.5) * (2.0 / h) - 1.0
    return F.grid_sample(img, torch.stack([gx, gy], -1), mode="bilinear", padding_mode="border", align_corners=False)


def lk_step(wa, wc):
    """One symmetric Lucas-Kanade step between two warped planes: solves
    sum(g g^T) d = sum(g It) over a box, g = grad(wa + wc), It = wc - wa.
    d is added to the flow to prev and subtracted from the flow to next."""
    kx = torch.tensor([[[[-0.5, 0.0, 0.5]]]])
    s = wa + wc
    gx = F.conv2d(F.pad(s, (1, 1, 0, 0), mode="replicate"), kx)
    gy = F.conv2d(F.pad(s, (0, 0, 1, 1), mode="replicate"), kx.transpose(2, 3))
    it = wc - wa
    r = LK_WIN // 2
    box = lambda t: F.avg_pool2d(F.pad(t, (r, r, r, r), mode="replicate"), LK_WIN, 1)
    a11, a12, a22 = box(gx * gx), box(gx * gy), box(gy * gy)
    b1, b2 = box(gx * it), box(gy * it)
    # Regularised RELATIVE to the local gradient energy: an absolute epsilon
    # swamped the determinant at coarse levels (it recovered 5% of a known pan).
    tr = a11 + a22
    det = a11 * a22 - a12 * a12 + 1e-3 * tr * tr + 1e-12
    dx = (a22 * b1 - a12 * b2) / det
    dy = (a11 * b2 - a12 * b1) / det
    return torch.cat([dx, dy], 1).clamp(-2.0, 2.0)


def lk_flow(y0, y2):
    """Flow to prev and to next (N,4,H/4,W/4), pixels at 1/4 scale."""
    flow = None
    for lv in LEVELS:
        a, c = F.avg_pool2d(y0, lv), F.avg_pool2d(y2, lv)
        if flow is None:
            flow = torch.zeros(a.shape[0], 4, a.shape[2], a.shape[3])
        else:
            flow = F.interpolate(flow, size=a.shape[-2:], mode="bilinear", align_corners=False) * 2.0
        for _ in range(LK_ITERS):
            d = lk_step(warp(a, flow[:, 0:2]), warp(c, flow[:, 2:4]))
            flow = flow + torch.cat([d, -d], 1)
    return flow


class Refine(nn.Module):
    """The dilated CNN: (warped prev, warped next, difference, flow) at 1/4
    -> (flow correction to prev, to next, blend-mask logit)."""

    def __init__(self):
        super().__init__()
        chans = [7] + [CH] * (len(DIL) - 1)
        self.body = nn.ModuleList(nn.Conv2d(chans[i], CH, 3, padding=d, dilation=d) for i, d in enumerate(DIL))
        self.head = nn.Conv2d(CH, 5, 3, padding=1)
        nn.init.zeros_(self.head.weight)
        nn.init.zeros_(self.head.bias)

    def forward(self, y0, y2, flow):
        a, c = F.avg_pool2d(y0, 4), F.avg_pool2d(y2, 4)
        wa, wc = warp(a, flow[:, 0:2]), warp(c, flow[:, 2:4])
        x = torch.cat([wa, wc, wa - wc, flow * 0.25], 1)
        for conv in self.body:
            x = F.leaky_relu(conv(x), 0.1)
        return self.head(x)


def model_flow(net, y0, y2):
    """(flow to prev, flow to next, mask logit) at 1/4; net=None: analytic only."""
    fl = lk_flow(y0, y2)
    if net is None:
        return torch.cat([fl, torch.zeros_like(fl[:, :1])], 1)
    r = net(y0, y2, fl)
    return torch.cat([fl + r[:, 0:4], r[:, 4:5]], 1)


def synth(out, planes0, planes2):
    res = []
    for p0, p2 in zip(planes0, planes2):
        h, w = p0.shape[-2:]
        scale = w / out.shape[-1]
        up = F.interpolate(out, size=(h, w), mode="bilinear", align_corners=False)
        m = torch.sigmoid(up[:, 4:5])
        res.append(m * warp(p0, up[:, 0:2] * scale) + (1 - m) * warp(p2, up[:, 2:4] * scale))
    return res


def to_t(a):
    return torch.from_numpy(a.astype(np.float32) / 255.0)[:, None]


def psnr(a, b):
    mse = float(((a - b) ** 2).mean())
    return 10 * math.log10(1.0 / max(mse, 1e-12))


def change(y, a, b):
    return float(np.abs(y[b, ::4, ::4].astype(np.int16) - y[a, ::4, ::4]).mean())


def is_cut(y, i):
    d1, d2 = change(y, i, i + 1), change(y, i + 1, i + 2)
    return max(d1, d2) > 5 and min(d1, d2) < 0.4 * max(d1, d2)


def cost():
    """Multiply-adds a pixel of the OUTPUT frame, roughly: (CNN, LK, warp+blend)."""
    chans = [7] + [CH] * (len(DIL) - 1)
    cnn = (sum(ci * CH * 9 for ci in chans) + CH * 5 * 9) / 16  # runs at 1/4 x 1/4
    lk_px = 2 * 9 + 5 * 2 + 5 * 2 + 12  # warps, gradients, box sums, solve
    lk = LK_ITERS * lk_px * sum(1 / (l * l) for l in LEVELS)
    return cnn, lk, 2 * 4 + 3


def main():
    path, w, h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    out_dir = sys.argv[4] if len(sys.argv) > 4 else os.path.dirname(os.path.abspath(__file__))
    y, u, v = load(path, w, h)
    n = y.shape[0]
    split = (n * 3 // 4) & ~1
    train_i = [i for i in range(0, split - 2, 2) if not is_cut(y, i)]
    val_all = list(range(split, n - 2, 2))
    cnn, lk, full = cost()
    print(f"{n} frames {w}x{h}; train on {len(train_i)} non-cut triplets from 0..{split}; verify on {len(val_all)} held-out from {split}..{n}")
    print(f"cost per output pixel: CNN {cnn:.0f} + Lucas-Kanade {lk:.0f} + warp/blend {full} multiply-adds")

    net = Refine()
    print(f"CNN: {sum(p.numel() for p in net.parameters())} parameters")
    opt = torch.optim.Adam(net.parameters(), lr=float(os.environ.get("LR", "5e-4")))
    steps = int(os.environ.get("STEPS", "1500"))
    ch_, cw_ = 256, 384
    t0 = time.time()
    moving_val = [i for i in val_all if not is_cut(y, i) and change(y, i, i + 2) > 2][:8]
    for step in range(steps):
        # Half real triplets (motion-weighted crops), half synthetic camera
        # pans with exact motion: the clip is mostly a still camera, and a
        # model that never sees motion learns "no motion".
        crops = []
        for i in np.random.choice(train_i, 4):
            best = None
            for _ in range(6):
                oy, ox = np.random.randint(0, (h - ch_) // 16) * 16, np.random.randint(0, (w - cw_) // 16) * 16
                d = np.abs(y[i + 2, oy:oy + ch_:4, ox:ox + cw_:4].astype(np.int16) - y[i, oy:oy + ch_:4, ox:ox + cw_:4]).mean()
                if best is None or d > best[0]:
                    best = (d, i, oy, ox)
            crops.append(best[1:])
        a = to_t(np.stack([y[i, oy:oy + ch_, ox:ox + cw_] for i, oy, ox in crops]))
        b = to_t(np.stack([y[i + 1, oy:oy + ch_, ox:ox + cw_] for i, oy, ox in crops]))
        c = to_t(np.stack([y[i + 2, oy:oy + ch_, ox:ox + cw_] for i, oy, ox in crops]))
        src, vs = [], []
        for _ in range(4):
            i = np.random.randint(0, split)
            oy, ox = np.random.randint(32, h - ch_ - 32), np.random.randint(32, w - cw_ - 32)
            src.append(y[i, oy - 32:oy + ch_ + 32, ox - 32:ox + cw_ + 32])
            vs.append(np.random.uniform(-24, 24, 2))
        S = to_t(np.stack(src))
        V = torch.tensor(np.array(vs), dtype=torch.float32)[:, :, None, None].expand(4, 2, ch_ + 64, cw_ + 64)
        a = torch.cat([a, warp(S, V)[..., 32:-32, 32:-32]])
        b = torch.cat([b, S[..., 32:-32, 32:-32]])
        c = torch.cat([c, warp(S, -V)[..., 32:-32, 32:-32]])
        out = model_flow(net, a, c)
        loss = 0.0
        for s_ in (1, 4):
            aa, bb, cc = (F.avg_pool2d(t, s_) if s_ > 1 else t for t in (a, b, c))
            (pred,) = synth(out, [aa], [cc])
            loss = loss + (pred - bb).abs().mean()
        opt.zero_grad()
        loss.backward()
        torch.nn.utils.clip_grad_norm_(net.parameters(), 1.0)
        opt.step()
        if step == steps * 2 // 3:
            for g in opt.param_groups:
                g["lr"] *= 0.3
        if step % 250 == 0 or step == steps - 1:
            with torch.no_grad():
                gain = []
                for i in moving_val:
                    y0, y1, y2 = to_t(y[i:i + 1]), to_t(y[i + 1:i + 2]), to_t(y[i + 2:i + 3])
                    (pm,) = synth(model_flow(net, y0, y2), [y0], [y2])
                    (pl,) = synth(model_flow(None, y0, y2), [y0], [y2])
                    gain.append(psnr(pm, y1) - psnr(pl, y1))
            print(f"  step {step:5d} L1 {loss.item():.4f}  held-out moving: CNN {np.mean(gain):+.2f} dB over Lucas-Kanade alone ({time.time()-t0:.0f} s)", flush=True)

    net.eval()
    cats = {"all": [], "non-cut": [], "moving": [], "cuts": []}
    with torch.no_grad():
        for i in val_all:
            y0, y1, y2 = to_t(y[i:i + 1]), to_t(y[i + 1:i + 2]), to_t(y[i + 2:i + 3])
            (pm,) = synth(model_flow(net, y0, y2), [y0], [y2])
            (pl,) = synth(model_flow(None, y0, y2), [y0], [y2])
            r = (psnr(y0, y1), psnr((y0 + y2) / 2, y1), psnr(pl, y1), psnr(pm, y1))
            cut = is_cut(y, i)
            cats["all"].append(r)
            cats["cuts" if cut else "non-cut"].append(r)
            if not cut and change(y, i, i + 2) > 2:
                cats["moving"].append(r)
    summary = {"params": sum(p.numel() for p in net.parameters()), "cost_per_px": {"cnn": cnn, "lk": lk, "warp_blend": full}}
    print("held-out verification, PSNR dB against the real frame (higher is better):")
    print(f"  {'':8s} {'n':>4s}  {'repeat':>7s} {'blend':>7s} {'LK only':>8s} {'LK+CNN':>7s}")
    for k, rows in cats.items():
        if not rows:
            continue
        m = np.array(rows).mean(0)
        summary[k] = {"n": len(rows), "repeat": m[0], "blend": m[1], "lk": m[2], "model": m[3]}
        print(f"  {k:8s} {len(rows):4d}  {m[0]:7.2f} {m[1]:7.2f} {m[2]:8.2f} {m[3]:7.2f}")
    nc = np.array(cats["non-cut"])
    summary["model_beats_repeat_and_blend"] = float(np.mean(nc[:, 3] > np.maximum(nc[:, 0], nc[:, 1])))
    summary["cnn_beats_lk"] = float(np.mean(nc[:, 3] > nc[:, 2]))
    print(f"  non-cut frames where LK+CNN beats both repeat and blend: {summary['model_beats_repeat_and_blend']*100:.0f}%; beats LK alone: {summary['cnn_beats_lk']*100:.0f}%")

    layers = [(cv.weight.detach().numpy(), cv.bias.detach().numpy(), cv.dilation[0]) for cv in list(net.body) + [net.head]]
    blob = {"levels": LEVELS, "lk_iters": LK_ITERS, "lk_win": LK_WIN, "layers": [{"w": w_.tolist(), "b": b_.tolist(), "dilation": d} for w_, b_, d in layers]}
    with open(os.path.join(out_dir, "weights.json"), "w") as f:
        json.dump(blob, f)
    with open(os.path.join(out_dir, "verify.json"), "w") as f:
        json.dump(summary, f, indent=1)
    # Parity fixture for the shader: one held-out moving triplet, inputs and
    # the model's synthesised luma and flow.
    i = moving_val[0] if moving_val else val_all[0]
    with torch.no_grad():
        y0, y2 = to_t(y[i:i + 1]), to_t(y[i + 2:i + 3])
        out = model_flow(net, y0, y2)
        (py,) = synth(out, [y0], [y2])
    np.savez_compressed(os.path.join(out_dir, "parity.npz"), y0=y[i], y1=y[i + 1], y2=y[i + 2], synth=(py[0, 0].numpy() * 255.0), flow=out[0].numpy())
    print(f"wrote weights.json, verify.json, parity.npz to {out_dir}")


if __name__ == "__main__":
    main()
