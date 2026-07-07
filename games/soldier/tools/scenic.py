#!/usr/bin/env python3
"""scenic.py — an OpenCV scenic/variety scorer for early-fps map renders.

A geometrically-sound gameplay map can still look *bland*: flat repeated textures, a monotone palette, a
boring silhouette. This tool judges the *look* of a map from the player's vantage and — importantly —
NEGATIVELY scores repetition and blandness, the two failure modes of procedural/box-built levels.

It consumes the multi-angle renders dumped by `cargo run --example scenic_shots --release` into
/tmp/scenic/<map>/angle*.png and reports, per map, a 0..100 scenic score built from five sub-metrics:

  texture   — local detail: mean of the local standard-deviation map (Laplacian energy + local variance).
              Flat painted boxes score low; furrowed bark, mottled plaster, rubble score high.
  colour    — palette richness: entropy of the hue histogram over reasonably-saturated pixels. A monochrome
              sand map scores low; a map mixing stone, foliage, water, rust scores high.
  silhouette— structural interest: edge density + the spread of edge orientations (Canny + gradient angle
              histogram entropy). A flat plane scores low; varied rooflines/rocks/trees score high.
  depth     — tonal range actually used: how much of the value range the frame occupies (contrast), so a
              foggy grey-out scores low and a scene with real lights and shadows scores high.
  variety   — REPETITION PENALTY (this is the negative term): autocorrelation of the luminance image at
              tile-sized offsets. A strongly self-similar (tiled/checkerboard/repeated-prefab) frame has a
              high autocorrelation peak → low variety → the scenic score is pulled DOWN.

Usage:
    python3 tools/scenic.py                 # score every map under /tmp/scenic
    python3 tools/scenic.py flood_zone      # score one map
"""
import os
import sys
import glob
import numpy as np
import cv2

SCENIC_DIR = "/tmp/scenic"


def _local_detail(gray: np.ndarray) -> float:
    """Mean local standard deviation + Laplacian energy — how much fine texture the surfaces carry."""
    g = gray.astype(np.float32)
    # local variance via box filter of x^2 - (box filter of x)^2
    k = 9
    mean = cv2.boxFilter(g, -1, (k, k))
    sq = cv2.boxFilter(g * g, -1, (k, k))
    var = np.clip(sq - mean * mean, 0, None)
    local_std = np.sqrt(var).mean()
    lap = cv2.Laplacian(g, cv2.CV_32F, ksize=3)
    lap_energy = np.sqrt((lap * lap).mean())
    # combine and normalise to ~0..1. Divisors calibrated against the actual render range (local_std ~5–22,
    # lap_energy ~70–200 on these 640x400 frames) so the metric *discriminates* between flat and detailed
    # maps rather than saturating at 1.0 for everything.
    return float(np.clip((local_std / 26.0) * 0.55 + (lap_energy / 230.0) * 0.45, 0, 1))


def _colour_richness(bgr: np.ndarray) -> float:
    """Entropy of the hue histogram over saturated pixels — palette diversity."""
    hsv = cv2.cvtColor(bgr, cv2.COLOR_BGR2HSV)
    h, s, v = hsv[:, :, 0], hsv[:, :, 1], hsv[:, :, 2]
    mask = (s > 40) & (v > 30)  # ignore washed-out sky / near-black shadow
    if mask.sum() < 500:
        return 0.0
    hue = h[mask]
    hist, _ = np.histogram(hue, bins=36, range=(0, 180))
    p = hist / hist.sum()
    p = p[p > 0]
    ent = -(p * np.log2(p)).sum()
    return float(np.clip(ent / np.log2(36), 0, 1))  # normalise by max entropy


def _silhouette(gray: np.ndarray) -> float:
    """Edge density + orientation spread — structural / silhouette interest."""
    edges = cv2.Canny(gray, 60, 160)
    density = edges.mean() / 255.0
    # orientation entropy of gradient angles where edges are strong
    gx = cv2.Sobel(gray, cv2.CV_32F, 1, 0, ksize=3)
    gy = cv2.Sobel(gray, cv2.CV_32F, 0, 1, ksize=3)
    mag = np.sqrt(gx * gx + gy * gy)
    strong = mag > np.percentile(mag, 90)
    if strong.sum() < 200:
        orient_ent = 0.0
    else:
        ang = (np.arctan2(gy[strong], gx[strong]) * 180 / np.pi) % 180
        hist, _ = np.histogram(ang, bins=18, range=(0, 180))
        p = hist / hist.sum()
        p = p[p > 0]
        orient_ent = -(p * np.log2(p)).sum() / np.log2(18)
    # density saturates quickly; blend with orientation variety
    return float(np.clip((density / 0.14) * 0.5 + orient_ent * 0.5, 0, 1))


def _depth_contrast(gray: np.ndarray) -> float:
    """How much of the tonal range the frame uses — proxy for real lighting/shadow depth."""
    lo, hi = np.percentile(gray, 3), np.percentile(gray, 97)
    return float(np.clip((hi - lo) / 200.0, 0, 1))


def _repetition_penalty(gray: np.ndarray) -> float:
    """Autocorrelation at tile-sized offsets. Returns a VARIETY score in 0..1 where 1 = not repetitive.

    We normalise the image, correlate it against copies shifted by a range of offsets, and take the strongest
    off-zero correlation peak. A tiled / checkerboard / repeated-prefab frame self-aligns strongly at some
    offset (high peak → low variety). An organic, non-repeating frame does not.
    """
    g = gray.astype(np.float32)
    g = g - g.mean()
    denom = (g * g).sum()
    if denom < 1e-3:
        return 0.0
    peak = 0.0
    H, W = g.shape
    # sample a set of offsets spanning plausible tile/prefab periods
    for dy in range(0, 48, 6):
        for dx in range(0, 48, 6):
            if dx == 0 and dy == 0:
                continue
            a = g[dy:H, dx:W]
            b = g[0:H - dy, 0:W - dx]
            c = float((a * b).sum() / denom)
            peak = max(peak, c)
    # peak near 1.0 = strongly self-similar (repetitive) → variety low. Map peak∈[0.3,0.9]→variety[1,0].
    variety = np.clip((0.9 - peak) / 0.6, 0, 1)
    return float(variety)


def score_image(path: str) -> dict:
    bgr = cv2.imread(path)
    if bgr is None:
        return {}
    gray = cv2.cvtColor(bgr, cv2.COLOR_BGR2GRAY)
    return {
        "texture": _local_detail(gray),
        "colour": _colour_richness(bgr),
        "silhouette": _silhouette(gray),
        "depth": _depth_contrast(gray),
        "variety": _repetition_penalty(gray),
    }


def score_map(map_dir: str) -> dict:
    shots = sorted(glob.glob(os.path.join(map_dir, "*.png")))
    if not shots:
        return {}
    subs = [score_image(p) for p in shots]
    subs = [s for s in subs if s]
    if not subs:
        return {}
    keys = subs[0].keys()
    avg = {k: float(np.mean([s[k] for s in subs])) for k in keys}
    # Scenic score: texture + colour + silhouette + depth are positive; variety gates the whole thing (a
    # gorgeous-but-tiled map should still be marked down). Weights sum to 1 before the variety gate.
    base = (
        avg["texture"] * 0.30
        + avg["colour"] * 0.24
        + avg["silhouette"] * 0.26
        + avg["depth"] * 0.20
    )
    # variety acts as a multiplicative penalty in [0.55, 1.0] — repetition can cost up to 45%.
    gate = 0.55 + 0.45 * avg["variety"]
    avg["scenic"] = float(np.clip(base * gate, 0, 1) * 100)
    return avg


def main():
    only = sys.argv[1] if len(sys.argv) > 1 else None
    if not os.path.isdir(SCENIC_DIR):
        print(f"no renders found at {SCENIC_DIR}; run: cargo run --example scenic_shots --release")
        return
    maps = sorted(d for d in os.listdir(SCENIC_DIR) if os.path.isdir(os.path.join(SCENIC_DIR, d)))
    if only:
        maps = [m for m in maps if m == only]
    print(f"{'MAP':<20} {'tex':>5} {'col':>5} {'silh':>5} {'depth':>5} {'vary':>5} {'SCENIC':>7}")
    print("-" * 60)
    rows = []
    for m in maps:
        r = score_map(os.path.join(SCENIC_DIR, m))
        if not r:
            continue
        rows.append((m, r))
        print(f"{m:<20} {r['texture']:>5.2f} {r['colour']:>5.2f} {r['silhouette']:>5.2f} "
              f"{r['depth']:>5.2f} {r['variety']:>5.2f} {r['scenic']:>6.1f}")
    if rows:
        print("-" * 60)
        best = max(rows, key=lambda x: x[1]["scenic"])
        worst = min(rows, key=lambda x: x[1]["scenic"])
        print(f"most scenic:  {best[0]} ({best[1]['scenic']:.1f})")
        print(f"blandest:     {worst[0]} ({worst[1]['scenic']:.1f})  ← candidate for more texture/variety work")


if __name__ == "__main__":
    main()
