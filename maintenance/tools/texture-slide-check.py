#!/usr/bin/env python3
"""Does the surface texture stick to the geometry, or slide under the camera?

Take two scene-view screenshots a small camera nudge apart:

    LOGSCOPE_CAM="0.60,0.18,26" scene-view   -> frameA.png
    LOGSCOPE_CAM="0.63,0.18,26" scene-view   -> frameB.png

then run this. It splits each frame into a low-frequency band (geometry and
shading) and a high-frequency band (fine texture detail), and finds the
horizontal shift that best aligns each band between the two frames.

If the texture is locked to the WORLD, both bands shift together. If it is
locked to the SCREEN, the geometry shifts and the detail does not — and the
gap between the two numbers is the size of the problem.

A pure rotation is not a pure translation, so the residuals never reach
zero; what matters is which shift wins, and whether the minimum is a real
peak rather than a flat curve (the script prints the neighbourhood so you
can see).
"""
import sys
import numpy as np
from PIL import Image, ImageFilter

def bands(path, blur=6.0):
    im = Image.open(path).convert("L")
    lo = np.asarray(im.filter(ImageFilter.GaussianBlur(blur)), dtype=np.float32)
    return lo, np.asarray(im, dtype=np.float32) - lo

def best_shift(a, b, span=45, margin=50):
    out = []
    for dx in range(-span, span + 1):
        r = np.abs(a[:, margin:-margin] - np.roll(b, dx, axis=1)[:, margin:-margin]).mean()
        out.append((dx, r))
    return min(out, key=lambda t: t[1]), dict(out)

def main(a_path, b_path, y0=280, y1=660, x0=680, x1=1220):
    alo, ahi = bands(a_path)
    blo, bhi = bands(b_path)
    ys, xs = slice(y0, y1), slice(x0, x1)
    (gdx, gres), _ = best_shift(alo[ys, xs], blo[ys, xs])
    (tdx, tres), curve = best_shift(ahi[ys, xs], bhi[ys, xs])
    print(f"geometry (low-freq)  best shift {gdx:+d} px  residual {gres:.2f}")
    print(f"texture  (high-freq) best shift {tdx:+d} px  residual {tres:.2f}")
    print("high-freq neighbourhood:",
          " ".join(f"{d:+d}:{curve[d]:.2f}" for d in (tdx - 10, tdx - 3, tdx, tdx + 3, tdx + 10) if d in curve))
    gap = abs(gdx - tdx)
    print(f"\ngap {gap} px — ", end="")
    print("texture tracks the geometry" if gap <= 3 else "TEXTURE IS NOT TRACKING THE GEOMETRY")
    return 0 if gap <= 3 else 1

if __name__ == "__main__":
    sys.exit(main(*(sys.argv[1:3] or ["frameA.png", "frameB.png"])))
