#!/usr/bin/env python3
"""Elementals — the island Thomas knows.  Illustrative, not photoreal.

The book's Visual Language section asks for cool greys, salt, wind, organic
shape and handheld looseness for the ISLAND, against the citadel's hard
symmetry. Procedural sine-field water gives the opposite of that — it reads
as corduroy, because real regularity is exactly what "handheld looseness"
is not. So the swell here is a set of individually drawn, broken strokes
with jitter, and the rock is a smooth organic mass rather than a noise
silhouette. Fewer elements, each deliberate.
"""
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

W, H = 1200, 1800
rng = np.random.default_rng(19)
HZ = int(H * 0.50)

def fbm(h, w, octaves, seed, lo=2):
    r = np.random.default_rng(seed)
    out = np.zeros((h, w))
    for o in range(octaves):
        gh, gw = max(lo, h >> (octaves - o + 2)), max(lo, w >> (octaves - o + 2))
        g = r.normal(0, 1, (gh, gw))
        g = np.asarray(Image.fromarray(((g - g.min()) / np.ptp(g) * 255).astype("uint8"))
                       .resize((w, h), Image.BICUBIC), float)
        out += (g / 255 - 0.5) * (0.5 ** o)
    return out

# ── sky: high cloud mass, a squall trailing left, light pooling at horizon ──
ys = np.linspace(0, 1, HZ)[:, None]
sky = (np.array([ 99, 107, 118]) * (1 - ys ** 1.4) + np.array([201, 204, 201]) * ys ** 1.4)
sky = np.repeat(sky[:, None, :], W, axis=1)
c = fbm(HZ, W, 5, 3); c -= c.min(); c /= c.max()
sky -= (c * 52 * np.clip(1.3 - ys * 1.7, 0, 1))[:, :, None]
xs1 = np.linspace(0, 1, W)[None, :]
sq = np.clip(1 - np.abs(xs1 - 0.22) / 0.30, 0, 1) ** 1.3
sq = sq * np.clip((ys - 0.18) * 1.6, 0, 1) * (0.7 + 0.3 * fbm(HZ, W, 3, 12))
sky -= (sq * 20)[:, :, None]

# ── sea: flat base. All structure is drawn, not synthesised. ───────────────
SH = H - HZ
yw = np.linspace(0, 1, SH)[:, None]
sea = (np.array([190, 195, 193]) * (1 - yw ** .8) + np.array([ 96, 106, 107]) * yw ** .8)
sea = np.repeat(sea[:, None, :], W, axis=1)

im = Image.fromarray(np.clip(np.vstack([sky, sea]), 0, 255).astype("uint8"))
d = ImageDraw.Draw(im, "RGBA")

# swell: broken strokes, spacing opening up toward the viewer, each one
# wandering. ROCK_TOP is where the near rock will cut them off.
ROCK_TOP = H * 0.735
for i in range(34):
    u = (i / 33) ** 1.9                       # perspective: tight at horizon
    y0 = HZ + 6 + u * (ROCK_TOP - HZ - 6)
    if y0 > ROCK_TOP: break
    lift = 0.12 + 0.88 * u
    x = -30.0
    while x < W + 30:
        seg = rng.uniform(40, 230) * (0.35 + 0.9 * lift)
        if rng.random() < 0.30 + 0.35 * (1 - lift):        # gaps
            x += seg * rng.uniform(0.4, 1.3); continue
        n = max(2, int(seg / 14))
        pts = []
        for k in range(n + 1):
            xx = x + seg * k / n
            yy = y0 + 5.0 * lift * np.sin(xx / 95.0 + i * 0.9) + rng.normal(0, 0.7 * lift)
            pts.append((xx, yy))
        a = int(np.clip(30 + 120 * lift, 0, 200) * rng.uniform(0.5, 1.0))
        d.line(pts, fill=(238, 242, 240, a), width=max(1, int(1 + 2.2 * lift)))
        if lift > 0.55 and rng.random() < 0.5:             # shadow under a crest
            d.line([(p[0], p[1] + 2 + 2 * lift) for p in pts],
                   fill=(60, 70, 74, int(60 * lift)), width=max(1, int(1 + lift)))
        x += seg * rng.uniform(1.0, 1.6)

# salt haze over the horizon, heaviest at the join
band = im.crop((0, HZ - 44, W, HZ + 26)).filter(ImageFilter.GaussianBlur(13))
im.paste(band, (0, HZ - 44))
d = ImageDraw.Draw(im, "RGBA")

# ── rock: smooth organic mass. Sea rock is worn, not spiky. ───────────────
def mass(y_base, amp, seed, col):
    r = np.random.default_rng(seed)
    X = np.linspace(-60, W + 60, 260)
    h = np.zeros_like(X)
    for f, a in [(0.55, 1.0), (1.3, 0.62), (2.6, 0.34), (5.1, 0.16)]:
        h += a * np.sin(2 * np.pi * f * X / W + r.uniform(0, 6.3))
    h /= np.abs(h).max()
    pts = [(float(x), float(y_base + amp * v)) for x, v in zip(X, h)]
    d.polygon(pts + [(W + 60, H + 80), (-60, H + 80)], fill=col)
    return pts

mass(H * 0.700, 22, 5, (112, 120, 125, 255))     # far stack, deep in haze
mass(H * 0.762, 30, 8, ( 74,  81,  86, 255))     # mid
near = mass(H * 0.862, 40, 2, ( 33,  38,  41, 255))
nx = np.array([p[0] for p in near]); ny = np.array([p[1] for p in near])

# wet sheen: the near rock catches a little sky on its upper faces
for _ in range(240):
    x = rng.uniform(0, W); base = np.interp(x, nx, ny)
    yy = base + rng.uniform(4, 60); ln = rng.uniform(12, 70)
    d.line([(x, yy), (x + ln, yy + rng.uniform(-2, 2))],
           fill=(120, 130, 136, int(rng.uniform(8, 34))), width=rng.integers(1, 3))

# ── tide pools: the soft places where the tide got in ────────────────────
def pool(cx, cy, rx, ry, seed):
    r = np.random.default_rng(seed); n = 40
    a = np.linspace(0, 2 * np.pi, n, endpoint=False)
    rad = np.convolve(np.r_[(g := 1 + .2 * r.normal(0, 1, n))[-4:], g, g[:4]],
                      np.ones(6) / 6, "same")[4:-4]
    outer = [(cx + rx * rad[i] * np.cos(t), cy + ry * rad[i] * np.sin(t)) for i, t in enumerate(a)]
    d.polygon(outer, fill=(18, 22, 25, 255))
    inner = [(cx + rx * .70 * rad[i] * np.cos(t), cy - ry * .16 + ry * .58 * rad[i] * np.sin(t))
             for i, t in enumerate(a)]
    d.polygon(inner, fill=(139, 149, 155, 205))
    d.line(inner[: n // 2] , fill=(190, 197, 199, 130), width=2)

pool(252, 1700, 108, 26, 1); pool(736, 1754, 142, 31, 2); pool(1020, 1676, 58, 14, 4)

# ── foam: gathers in the hollows, in gusts ───────────────────────────────
for _ in range(1000):
    x = rng.uniform(0, W); base = np.interp(x, nx, ny)
    hollow = np.clip((base - ny.min()) / max(1e-3, np.ptp(ny)), 0, 1)
    gust = 0.3 + 0.7 * (0.5 + 0.5 * np.sin(x / W * 9.0 + 2.1))
    if rng.random() > hollow * gust: continue
    yy = base - abs(rng.normal(0, 10)); rr = rng.uniform(0.7, 2.4)
    al = int(145 * np.exp(-((base - yy) / 15) ** 2) * rng.uniform(0.35, 1.0))
    d.ellipse([x - rr, yy - rr, x + rr, yy + rr], fill=(240, 243, 241, al))

# ── the figure: small, low, facing the water ─────────────────────────────
fx = W * 0.335
fy = float(np.interp(fx, nx, ny)) + 16
s = 34.0
ink = (17, 20, 22, 255)
d.ellipse([fx - s*.15, fy - s*1.00, fx + s*.15, fy - s*.70], fill=ink)
d.polygon([(fx - s*.23, fy - s*.70), (fx + s*.23, fy - s*.70),
           (fx + s*.17, fy), (fx - s*.17, fy)], fill=ink)
d.line([(fx - s*.08, fy), (fx - s*.14, fy + s*.46)], fill=ink, width=5)
d.line([(fx + s*.08, fy), (fx + s*.15, fy + s*.46)], fill=ink, width=5)
d.line([(fx + s*.21, fy - s*.58), (fx + s*.40, fy - s*.16)], fill=ink, width=5)

# ── grade ────────────────────────────────────────────────────────────────
a = np.asarray(im, float)
g = a.mean(axis=2, keepdims=True); a = g + (a - g) * 0.50
a = (a / 255) ** 0.96 * 255
a[:, :, 2] *= 1.075; a[:, :, 1] *= 1.012; a[:, :, 0] *= 0.955
yv, xv = np.mgrid[0:H, 0:W]
a *= np.clip(1 - 0.24 * (((xv - W/2)/(W/2))**2 + ((yv - H*0.44)/(H/2))**2) ** 1.25, 0, 1)[:, :, None]
a += rng.normal(0, 2.6, a.shape)
im = Image.fromarray(np.clip(a, 0, 255).astype("uint8"))

# ── type: set in the sky, where the book's own note says to let people be
#    small. Letterspaced caps, low contrast — the citadel is hard symmetry,
#    so the title is the one ordered thing in an organic frame.
from PIL import ImageFont
d = ImageDraw.Draw(im, "RGBA")
TT = "/usr/share/fonts/truetype/crosextra/Caladea-Regular.ttf"
TB = "/usr/share/fonts/truetype/crosextra/Caladea-Bold.ttf"

def track(draw, xy, text, font, fill, sp):
    x, y = xy
    for ch in text:
        draw.text((x, y), ch, font=font, fill=fill)
        x += draw.textlength(ch, font=font) + sp
    return x - xy[0] - sp

def centred(draw, y, text, font, fill, sp):
    w = sum(draw.textlength(c, font=font) + sp for c in text) - sp
    track(draw, ((W - w) / 2, y), text, font, fill, sp)
    return w

ink = (238, 241, 240, 236)
f_title = ImageFont.truetype(TB, 104)
f_sub   = ImageFont.truetype(TT, 38)
f_auth  = ImageFont.truetype(TT, 34)

centred(d, 196, "ELEMENTALS", f_title, ink, 13)
w = centred(d, 336, "BOOK TWO", f_sub, (226, 231, 231, 190), 9)
d.line([(W/2 - w/2 - 54, 355), (W/2 - w/2 - 18, 355)], fill=(226, 231, 231, 120), width=1)
d.line([(W/2 + w/2 + 18, 355), (W/2 + w/2 + 54, 355)], fill=(226, 231, 231, 120), width=1)
centred(d, 392, "THE ABSENCE", f_sub, (226, 231, 231, 214), 9)
centred(d, H - 150, "KESSLER DUPONT", f_auth, (231, 235, 234, 196), 8)

im.save("cover_beach.png")
print("wrote cover_beach.png")
