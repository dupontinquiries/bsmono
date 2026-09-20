#!/usr/bin/env python3
"""look.py — rasterise a Mesh so a human can see it.

PIPELINE.md's closing note is that nothing in the model pipeline was ever
checked by eye, and the one time visual feedback arrived it revealed ribs
sealed inside an opaque chest that no measurement had caught. So this exists
before any model does: z-buffered flat shading, three-quarter view, one
call. Not pretty — sufficient to catch inside-out normals, collapsed rings
and parts floating in the wrong place.
"""
import math
from PIL import Image


def render(mesh, size=(520, 360), yaw=0.62, pitch=0.30, bg=(24, 26, 30),
           tint=(190, 196, 202), pad=1.14, label=None):
    W, H = size
    (lo, hi) = mesh.bounds()
    ctr = [(lo[i] + hi[i]) * 0.5 for i in range(3)]
    ext = max(hi[i] - lo[i] for i in range(3)) or 1.0

    cy, sy = math.cos(yaw), math.sin(yaw)
    cp, sp = math.cos(pitch), math.sin(pitch)

    def view(p):
        x, y, z = p[0] - ctr[0], p[1] - ctr[1], p[2] - ctr[2]
        x, z = x * cy - z * sy, x * sy + z * cy      # yaw about Y
        y, z = y * cp - z * sp, y * sp + z * cp      # pitch about X
        return x, y, z

    pts = [view(p) for p in mesh.v]
    scale = min(W, H) / (ext * pad)
    scr = [(W * 0.5 + x * scale, H * 0.5 - y * scale, z) for (x, y, z) in pts]

    img = Image.new("RGB", (W, H), bg)
    px = img.load()
    zbuf = [[1e30] * W for _ in range(H)]
    light = (-0.42, 0.72, -0.55)
    ln = math.sqrt(sum(c * c for c in light))
    light = tuple(c / ln for c in light)

    for (a, b, c) in mesh.f:
        p0, p1, p2 = scr[a], scr[b], scr[c]
        # winding in screen space: cull back faces, which is also how an
        # inside-out mesh announces itself (it renders hollow)
        area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p2[0] - p0[0]) * (p1[1] - p0[1])
        if area >= 0:
            continue
        v0, v1, v2 = pts[a], pts[b], pts[c]
        ux, uy, uz = (v1[i] - v0[i] for i in range(3))
        vx, vy, vz = (v2[i] - v0[i] for i in range(3))
        nx, ny, nz = uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx
        nl = math.sqrt(nx * nx + ny * ny + nz * nz) or 1.0
        nx, ny, nz = nx / nl, ny / nl, nz / nl
        d = max(0.0, nx * light[0] + ny * light[1] + nz * light[2])
        sh = 0.22 + 0.78 * d
        col = tuple(min(255, int(t * sh)) for t in tint)

        minx = max(0, int(min(p0[0], p1[0], p2[0])))
        maxx = min(W - 1, int(max(p0[0], p1[0], p2[0])) + 1)
        miny = max(0, int(min(p0[1], p1[1], p2[1])))
        maxy = min(H - 1, int(max(p0[1], p1[1], p2[1])) + 1)
        if minx > maxx or miny > maxy:
            continue
        denom = area
        for yy in range(miny, maxy + 1):
            for xx in range(minx, maxx + 1):
                w0 = ((p1[0] - p0[0]) * (yy + .5 - p0[1]) - (xx + .5 - p0[0]) * (p1[1] - p0[1])) / denom
                w1 = ((p2[0] - p1[0]) * (yy + .5 - p1[1]) - (xx + .5 - p1[0]) * (p2[1] - p1[1])) / denom
                w2 = 1.0 - w0 - w1
                if w0 < 0 or w1 < 0 or w2 < 0:
                    continue
                # w0 is the weight of p2, w1 of p0, w2 of p1 — the edge
                # functions above are built on edges p0->p1, p1->p2, so the
                # weight each produces belongs to the OPPOSITE vertex.
                # Getting this rotated renders far geometry in front of near.
                z = w1 * p0[2] + w2 * p1[2] + w0 * p2[2]
                if z < zbuf[yy][xx]:
                    zbuf[yy][xx] = z
                    px[xx, yy] = col
    if label:
        from PIL import ImageDraw
        ImageDraw.Draw(img).text((8, H - 16), label, fill=(150, 156, 162))
    return img


def sheet(items, cols=3, size=(400, 280), **kw):
    """Contact sheet: name -> mesh."""
    ims = [render(m, size=size, label=n, **kw) for n, m in items]
    rows = (len(ims) + cols - 1) // cols
    W, H = size
    out = Image.new("RGB", (cols * W, rows * H), (18, 20, 23))
    for i, im in enumerate(ims):
        out.paste(im, ((i % cols) * W, (i // cols) * H))
    return out
