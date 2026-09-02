#!/usr/bin/env python3
"""depth_probe — how much does RealESRGAN (RRDBNet) lose when the body is truncated?

Runs the x2 model at full depth (23 RRDB blocks) and at shallower depths on
one image, no training, and reports PSNR of each truncated output against the
full-depth output. numpy only. This is a measurement, not a recommendation:
the number is content-dependent and one frame is one frame.

Usage: depth_probe.py model.pth image.png [depths...]
"""
import sys, time
import numpy as np
sys.path.insert(0, __import__("os").path.dirname(__file__))
from pth2st import load_state_dict


def conv3x3(x, w, b, pad=1):
    # x: [C,H,W]; w: [O,C,3,3]; im2col matmul
    C, H, W = x.shape
    xp = np.pad(x, ((0, 0), (pad, pad), (pad, pad))) if pad else x
    oh, ow = H + 2 * pad - 2, W + 2 * pad - 2
    cols = np.empty((C * 9, oh * ow), dtype=np.float32)
    k = 0
    for ky in range(3):
        for kx in range(3):
            cols[k::9] = xp[:, ky:ky + oh, kx:kx + ow].reshape(C, -1)
            k += 1
    # w laid out [O, C, ky, kx] -> [O, C*9] with (c, ky, kx) order matching cols (c*9 + ky*3 + kx)
    return (w.reshape(w.shape[0], -1) @ cols).reshape(-1, oh, ow) + b[:, None, None]


def lrelu(x, s=0.2):
    return np.where(x >= 0, x, x * s)


def rdb(sd, p, x):
    x1 = lrelu(conv3x3(x, sd[p + "conv1.weight"], sd[p + "conv1.bias"]))
    x2 = lrelu(conv3x3(np.concatenate([x, x1]), sd[p + "conv2.weight"], sd[p + "conv2.bias"]))
    x3 = lrelu(conv3x3(np.concatenate([x, x1, x2]), sd[p + "conv3.weight"], sd[p + "conv3.bias"]))
    x4 = lrelu(conv3x3(np.concatenate([x, x1, x2, x3]), sd[p + "conv4.weight"], sd[p + "conv4.bias"]))
    x5 = conv3x3(np.concatenate([x, x1, x2, x3, x4]), sd[p + "conv5.weight"], sd[p + "conv5.bias"])
    return x5 * 0.2 + x


def rrdb(sd, p, x):
    out = rdb(sd, p + "rdb1.", x)
    out = rdb(sd, p + "rdb2.", out)
    out = rdb(sd, p + "rdb3.", out)
    return out * 0.2 + x


def up2(x):
    return x.repeat(2, axis=1).repeat(2, axis=2)


def pixel_unshuffle(x, r):
    C, H, W = x.shape
    x = x.reshape(C, H // r, r, W // r, r)
    return x.transpose(0, 2, 4, 1, 3).reshape(C * r * r, H // r, W // r)


def forward(sd, img, scale, depth, n_blocks):
    x = img
    if scale == 2:
        x = pixel_unshuffle(x, 2)
    elif scale == 1:
        x = pixel_unshuffle(x, 4)
    feat = conv3x3(x, sd["conv_first.weight"], sd["conv_first.bias"])
    body = feat
    for i in range(min(depth, n_blocks)):
        body = rrdb(sd, f"body.{i}.", body)
    feat = feat + conv3x3(body, sd["conv_body.weight"], sd["conv_body.bias"])
    feat = lrelu(conv3x3(up2(feat), sd["conv_up1.weight"], sd["conv_up1.bias"]))
    feat = lrelu(conv3x3(up2(feat), sd["conv_up2.weight"], sd["conv_up2.bias"]))
    if "conv_up3.weight" in sd:
        feat = lrelu(conv3x3(up2(feat), sd["conv_up3.weight"], sd["conv_up3.bias"]))
    out = conv3x3(lrelu(conv3x3(feat, sd["conv_hr.weight"], sd["conv_hr.bias"])), sd["conv_last.weight"], sd["conv_last.bias"])
    return np.clip(out, 0, 1)


def psnr(a, b):
    mse = np.mean((a - b) ** 2)
    return float("inf") if mse == 0 else 20 * np.log10(1.0) - 10 * np.log10(mse)


def main():
    from PIL import Image
    sd = load_state_dict(sys.argv[1])
    img = np.asarray(Image.open(sys.argv[2]).convert("RGB"), dtype=np.float32).transpose(2, 0, 1) / 255.0
    n_blocks = 1 + max(int(k.split(".")[1]) for k in sd if k.startswith("body."))
    cin = sd["conv_first.weight"].shape[1]
    scale = {3: 4, 12: 2, 48: 1}[cin]
    if "conv_up3.weight" in sd:
        scale = 8
    depths = [int(d) for d in sys.argv[3:]] or [n_blocks, 16, 12, 8, 6, 4, 2, 0]
    print(f"model: {n_blocks} RRDB blocks, native scale x{scale}, image {img.shape[2]}x{img.shape[1]}")
    t0 = time.time()
    full = forward(sd, img, scale, n_blocks, n_blocks)
    print(f"full depth: {time.time() - t0:.1f}s")
    # Bicubic baseline for perspective.
    hr = full.shape[1:]
    bic = np.asarray(Image.open(sys.argv[2]).convert("RGB").resize((hr[1], hr[0]), Image.BICUBIC), dtype=np.float32).transpose(2, 0, 1) / 255.0
    print(f"bicubic vs full: {psnr(bic, full):.2f} dB   (how far a non-CNN is from the model)")
    for d in depths:
        if d >= n_blocks:
            continue
        t0 = time.time()
        y = forward(sd, img, scale, d, n_blocks)
        print(f"depth {d:2d}/{n_blocks}: {psnr(y, full):6.2f} dB vs full   ({time.time() - t0:.1f}s, cost ≈ {(d * 3 * 0.2396 * 1e6 + 0.35e6) / (n_blocks * 3 * 0.2396 * 1e6 + 0.35e6):.0%})")
    Image.fromarray((full.transpose(1, 2, 0) * 255).round().astype(np.uint8)).save("/tmp/depth_full.png")


if __name__ == "__main__":
    main()
