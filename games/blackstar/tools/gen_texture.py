#!/usr/bin/env python3
"""gen_texture.py -- procedural, seamlessly TILEABLE 256x256 textures.

A companion to the geometry pipeline (gen_world / gen_spine): generate material
diffuse maps procedurally instead of hand-authoring PNGs, for fast artistic
iteration and new map/prop palettes.

Patterns:
  plating   -- riveted metal panels (seams + corner rivets)
  hexmesh   -- hexagonal tech mesh
  circuit   -- rectilinear circuit traces
  rock      -- rough noisy stone
  sediment  -- fine abyssal seabed with cracks
  crystal   -- faceted crystal with bright veins (pair with an emit map)

Usage:
  python3 tools/gen_texture.py <pattern> <out.png> \
      [--base R,G,B] [--accent R,G,B] [--seed N] [--emit OUT]  (emit: glow map)

Tileability: all noise is band-limited in the frequency domain (FFT low-pass),
so it wraps perfectly at the edges; grid patterns use modular coordinates.
"""
import sys, argparse, numpy as np
from PIL import Image

N = 256

def tileable_fbm(seed, octaves=5, persistence=0.55):
    """Seamless fractal noise in [0,1] via FFT band-limiting (wraps at edges)."""
    rng = np.random.default_rng(seed)
    fy = np.fft.fftfreq(N)[:, None] * N
    fx = np.fft.fftfreq(N)[None, :] * N
    r = np.sqrt(fx * fx + fy * fy)
    acc = np.zeros((N, N)); amp = 1.0; tot = 0.0
    for o in range(octaves):
        freq = 2.0 ** (o + 1)
        F = np.fft.fft2(rng.standard_normal((N, N)))
        F[r > freq] = 0                      # keep only frequencies <= this octave
        img = np.fft.ifft2(F).real
        img = (img - img.min()) / (np.ptp(img) + 1e-9)
        acc += img * amp; tot += amp; amp *= persistence
    return acc / tot

def lerp_rgb(t, a, b):
    a = np.array(a, float); b = np.array(b, float)
    return a[None, None, :] + (b - a)[None, None, :] * t[:, :, None]

def shade(img_t, base, accent):
    return np.clip(lerp_rgb(img_t, base, accent), 0, 255).astype(np.uint8)

def pat_ground(seed, base, accent):
    """Detailed terrain floor: undulation + pebbles/grit + erosion rills + patchiness.
    Far richer than plain FBM so walked-on ground doesn't read flat."""
    def norm(a):                                               # stretch to full 0..1
        lo, hi = a.min(), a.max()
        return (a - lo) / (hi - lo + 1e-6)
    base_und = norm(tileable_fbm(seed, 6, 0.55))               # coarse ground undulation
    patch = norm(tileable_fbm(seed + 21, 2, 0.7))              # large-scale lighter/darker patches
    grit = tileable_fbm(seed + 5, 7, 0.75)                     # fine high-freq grit
    rill_n = tileable_fbm(seed + 13, 5, 0.6)
    # thresholds relative to each field's own distribution (fbm is narrow-ranged)
    peb_light = (grit > np.percentile(grit, 90)).astype(float) # top 10% -> bright pebbles
    peb_dark  = (grit < np.percentile(grit, 12)).astype(float) # low grit -> dark flecks
    rills     = (rill_n < np.percentile(rill_n, 14)).astype(float)  # thin erosion channels
    t = base_und * 0.62 + patch * 0.22 + 0.16
    t = t + peb_light * 0.16 - peb_dark * 0.13 - rills * 0.20
    t = t + norm(grit) * 0.10 - 0.05                           # overall micro-grain
    t = np.clip(t, 0, 1)
    return shade(t, base, accent)

def pat_rock(seed, base, accent):
    n = tileable_fbm(seed, 6, 0.6)
    return shade(n, base, accent)

def pat_sediment(seed, base, accent):
    n = tileable_fbm(seed, 6, 0.5) * 0.7 + tileable_fbm(seed + 9, 3, 0.7) * 0.3
    cracks = (tileable_fbm(seed + 3, 5, 0.6) < 0.18).astype(float)  # thin dark cracks
    t = np.clip(n - cracks * 0.5, 0, 1)
    return shade(t, base, accent)

def pat_plating(seed, base, accent):
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:N, 0:N]
    cell = 64
    cx, cy = xx % cell, yy % cell
    panel_id = (xx // cell) + (yy // cell) * (N // cell)
    tint = (np.sin(panel_id * 2.399) * 0.5 + 0.5) * 0.25 + 0.6      # per-panel shade
    seam = ((cx < 2) | (cy < 2) | (cx > cell - 3) | (cy > cell - 3)).astype(float)
    rivet = (((cx - 8) ** 2 + (cy - 8) ** 2 < 9) |
             ((cx - (cell - 8)) ** 2 + (cy - 8) ** 2 < 9) |
             ((cx - 8) ** 2 + (cy - (cell - 8)) ** 2 < 9) |
             ((cx - (cell - 8)) ** 2 + (cy - (cell - 8)) ** 2 < 9)).astype(float)
    grain = tileable_fbm(seed + 1, 5, 0.5) * 0.17
    scratch = (tileable_fbm(seed + 7, 6, 0.65) - 0.5) * 0.11   # fine grit/wear
    t = np.clip(tint + grain + scratch - seam * 0.35 + rivet * 0.3, 0, 1)
    return shade(t, base, accent)

def pat_hexmesh(seed, base, accent):
    yy, xx = np.mgrid[0:N, 0:N].astype(float)
    # hex lattice distance field (two offset rows)
    hx = 64.0; hy = 32.0                       # 4 cols x 8 rows -> divides 256 (tiles)
    row = np.floor(yy / hy)
    ox = (row % 2) * (hx / 2)
    gx = ((xx - ox) % hx) - hx / 2
    gy = (yy % hy) - hy / 2
    d = np.maximum(np.abs(gx) / (hx / 2), np.abs(gy) / (hy / 2))
    edge = (d > 0.82).astype(float)
    grain = tileable_fbm(seed, 4, 0.5) * 0.20
    scratch = (tileable_fbm(seed + 7, 6, 0.65) - 0.5) * 0.10
    t = np.clip(0.55 + grain + scratch + edge * 0.35, 0, 1)
    return shade(t, base, accent)

def pat_circuit(seed, base, accent):
    rng = np.random.default_rng(seed)
    t = np.full((N, N), 0.35) + tileable_fbm(seed, 5, 0.5) * 0.1
    for _ in range(40):                       # traces (wrap via modulo)
        horiz = rng.random() < 0.5
        p = rng.integers(0, N); a = rng.integers(0, N); b = a + rng.integers(20, 120)
        idx = np.arange(a, b) % N
        if horiz: t[p, idx] = 0.85
        else:     t[idx, p] = 0.85
    for _ in range(60):                       # pads/vias
        y, x = rng.integers(0, N, 2)
        yy, xx = np.mgrid[-3:4, -3:4]
        t[(y + yy) % N, (x + xx) % N] = 0.9
    return shade(np.clip(t, 0, 1), base, accent)

def pat_crystal(seed, base, accent):
    rng = np.random.default_rng(seed)
    pts = rng.random((14, 2)) * N
    yy, xx = np.mgrid[0:N, 0:N]
    d1 = np.full((N, N), 1e9); d2 = np.full((N, N), 1e9)
    for (py, px) in pts:                       # toroidal voronoi (tileable)
        dy = np.minimum(np.abs(yy - py), N - np.abs(yy - py))
        dx = np.minimum(np.abs(xx - px), N - np.abs(xx - px))
        d = np.sqrt(dx * dx + dy * dy)
        closer = d < d1
        d2 = np.where(closer, d1, np.minimum(d2, d)); d1 = np.where(closer, d, d1)
    edge = np.clip(1.0 - (d2 - d1) / 6.0, 0, 1)  # bright facet seams (veins)
    facet = (d1 / d1.max())
    t = np.clip(facet * 0.5 + edge * 0.6, 0, 1)
    return shade(t, base, accent), edge   # edge doubles as the glow mask

def pat_ice(seed, base, accent):
    # cracked frost: frosted fbm sheet fractured by sharp toroidal-voronoi seams; the
    # cracks darken (deep ice) and carry a faint glow (frozen energon). Cryotek's domain.
    rng = np.random.default_rng(seed)
    frost = tileable_fbm(seed, 5, 0.55)
    pts = rng.random((22, 2)) * N
    yy, xx = np.mgrid[0:N, 0:N]
    d1 = np.full((N, N), 1e9); d2 = np.full((N, N), 1e9)
    for (py, px) in pts:
        dy = np.minimum(np.abs(yy - py), N - np.abs(yy - py))
        dx = np.minimum(np.abs(xx - px), N - np.abs(xx - px))
        d = np.sqrt(dx * dx + dy * dy); closer = d < d1
        d2 = np.where(closer, d1, np.minimum(d2, d)); d1 = np.where(closer, d, d1)
    cracks = np.clip(1.0 - (d2 - d1) / 3.0, 0, 1)
    rime = (frost > 0.72).astype(float) * 0.14
    t = np.clip(0.58 + frost * 0.32 - cracks * 0.5 + rime, 0, 1)
    glow = cracks * 0.28
    return shade(t, base, accent), glow

def pat_glyph(seed, base, accent):
    # Cybertronian glyphs: plating etched with angular rune strokes that glow (energon
    # -lit script). Strokes stay inside cell margins so the sheet tiles cleanly.
    grain = tileable_fbm(seed + 1, 5, 0.5) * 0.15
    t = np.full((N, N), 0.44) + grain
    glow = np.zeros((N, N))
    yy, xx = np.mgrid[0:N, 0:N]
    cell = 64; m = 10
    for gy in range(0, N, cell):
        for gx in range(0, N, cell):
            r = np.random.default_rng(seed * 131 + gy * 7 + gx + 1)
            for _ in range(int(r.integers(3, 7))):
                if r.random() < 0.5:   # horizontal stroke
                    y0 = gy + int(r.integers(m, cell - m)); x0 = gx + int(r.integers(m, cell - 2 * m)); x1 = min(gx + cell - m, x0 + int(r.integers(8, cell - 2 * m)))
                    mask = (yy >= y0) & (yy < y0 + 3) & (xx >= x0) & (xx < x1)
                else:                  # vertical stroke
                    x0 = gx + int(r.integers(m, cell - m)); y0 = gy + int(r.integers(m, cell - 2 * m)); y1 = min(gy + cell - m, y0 + int(r.integers(8, cell - 2 * m)))
                    mask = (xx >= x0) & (xx < x0 + 3) & (yy >= y0) & (yy < y1)
                t = np.where(mask, 0.72, t); glow = np.where(mask, 1.0, glow)
    return shade(np.clip(t, 0, 1), base, accent), glow

def pat_cryoglass(seed, base, accent):
    """Frosted cryo-pod glass: fine frost fern noise over a cold gradient, with a
    faint inner glow ellipse (the sleeper's silhouette light)."""
    g = tileable_fbm(seed, octaves=5)
    xx, yy = np.meshgrid(np.arange(N), np.arange(N))
    cx, cy = (xx-N/2)/(N/2), (yy-N/2)/(N/2)
    core = np.clip(1.0 - (cx*cx*2.2 + cy*cy*1.1), 0, 1)      # inner ellipse glow
    frost = np.clip(g*1.4 - 0.25, 0, 1)**1.5                 # fern crystallization
    img = np.zeros((N, N, 3))
    for c in range(3):
        img[:, :, c] = base[c]*(0.55+0.30*frost) + accent[c]*core*0.35*(0.6+0.4*(1-frost))
    return np.clip(img,0,255).astype(np.uint8)

def pat_tholin(seed, base, accent):
    """Pluto tholin deposits: reddish-brown photochemical fallout in soft drifts
    with darker pooled crevices (Cthulhu Regio's paint)."""
    g  = tileable_fbm(seed, octaves=4)
    g2 = tileable_fbm(seed+7, octaves=6)
    drifts = np.clip(g*1.2-0.1, 0, 1)
    pools  = np.clip(0.5-g2, 0, 1)*1.6
    img = np.zeros((N, N, 3))
    for c in range(3):
        img[:, :, c] = base[c]*(0.5+0.45*drifts) - np.array(accent[c])*pools*0.25
    return np.clip(img,0,255).astype(np.uint8)

def pat_hazard(seed, base, accent):
    """Diagonal warning chevrons over worn plating -- industrial kit + kibble accents."""
    g = tileable_fbm(seed, octaves=4)
    xx, yy = np.meshgrid(np.arange(N), np.arange(N))
    stripe = (((xx + yy) // 24) % 2).astype(float)          # 45-degree chevrons
    wear = np.clip(tileable_fbm(seed + 5, octaves=5) * 1.6 - 0.35, 0, 1)
    img = np.zeros((N, N, 3))
    for c in range(3):
        col = stripe * accent[c] + (1 - stripe) * base[c]   # accent/dark bands
        img[:, :, c] = col * (0.55 + 0.45 * g) * (1.0 - wear * 0.45)
    return np.clip(img, 0, 255).astype(np.uint8)

def pat_weave(seed, base, accent):
    """Carbon twill weave: alternating warp/weft sheen blocks -- a customization finish."""
    g = tileable_fbm(seed, octaves=3)
    xx, yy = np.meshgrid(np.arange(N), np.arange(N))
    bx, by = (xx // 16) % 2, (yy // 16) % 2
    horiz = ((bx + by) % 2).astype(float)                    # twill checker
    sheenH = 0.5 + 0.5 * np.sin(yy * (2 * np.pi / 16.0))     # weft catches light across
    sheenV = 0.5 + 0.5 * np.sin(xx * (2 * np.pi / 16.0))     # warp catches light along
    sheen = horiz * sheenH + (1 - horiz) * sheenV
    img = np.zeros((N, N, 3))
    for c in range(3):
        img[:, :, c] = base[c] * (0.5 + 0.28 * sheen) + accent[c] * sheen * 0.22 * (0.7 + 0.3 * g)
    return np.clip(img, 0, 255).astype(np.uint8)

def to_normal_map(img, strength=2.2):
    """Derive a tileable tangent-space normal map from a diffuse's luminance
    height field. np.roll keeps the gradient seamless, matching our tileable art."""
    h = img.astype(float).mean(axis=2) / 255.0
    gx = (np.roll(h, -1, axis=1) - np.roll(h, 1, axis=1)) * strength
    gy = (np.roll(h, -1, axis=0) - np.roll(h, 1, axis=0)) * strength
    nx, ny, nz = -gx, -gy, np.ones_like(h)
    ln = np.sqrt(nx*nx + ny*ny + nz*nz); ln[ln == 0] = 1
    rgb = np.stack([nx/ln*0.5+0.5, ny/ln*0.5+0.5, nz/ln*0.5+0.5], axis=2)
    return np.clip(rgb*255, 0, 255).astype(np.uint8)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pattern", choices=["plating","hexmesh","circuit","rock","ground","sediment","crystal","ice","glyph","hazard","weave","cryoglass","tholin"])
    ap.add_argument("out")
    ap.add_argument("--base", default="40,44,52")
    ap.add_argument("--accent", default="150,160,180")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--emit", default=None, help="also write a glow/emission map here")
    ap.add_argument("--norm", default=None, help="also write a tangent-space normal map here")
    a = ap.parse_args()
    base = [int(x) for x in a.base.split(",")]
    accent = [int(x) for x in a.accent.split(",")]
    emit = None
    if a.pattern in ("crystal", "ice", "glyph"):
        img, glow = globals()["pat_" + a.pattern](a.seed, base, accent); emit = glow
    else:
        img = globals()["pat_" + a.pattern](a.seed, base, accent)
    Image.fromarray(img, "RGB").save(a.out)
    print(f"wrote {a.out}: {a.pattern} 256x256")
    if a.emit is not None:
        g = emit if emit is not None else np.zeros((N, N))
        gcol = (np.clip(g, 0, 1)[:, :, None] * np.array(accent)[None, None, :]).astype(np.uint8)
        Image.fromarray(gcol, "RGB").save(a.emit)
        print(f"wrote {a.emit}: emission map")
    if a.norm is not None:
        Image.fromarray(to_normal_map(img), "RGB").save(a.norm)
        print(f"wrote {a.norm}: normal map")

if __name__ == "__main__":
    main()
