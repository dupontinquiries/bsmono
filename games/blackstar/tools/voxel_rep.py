#!/usr/bin/env python3
"""voxel_rep.py -- a representative lossy model: coloured voxels, trimmed palette.

The part-web reconstructs a model exactly when its parts are in the library. The
new-model test showed the other case: an Optimus Prime from a different asset
lineage shared **19 of 157 part types** with a 40,742-part library, and the best
geometric match for its unmatched parts was 0.32 IoU. Parts do not transfer
across lineages, so a library that only stores parts cannot keep a model it has
never seen anything like.

This is the fallback for exactly that case: keep the model, lossily, at a size
where keeping it costs nothing.

## What SVOX contributed

The SVOX format (github: SVOX, a compact MagicaVoxel .vox alternative) makes one
point worth taking: **.vox always writes the full 256-colour palette even when
the model uses eight**, so a trimmed per-model palette is most of the saving on
small assets. Its own layout is not reusable here — `voxelAmount` is a single
byte, capping a file at 255 voxels, where our models want tens of thousands —
but the palette idea is exactly what our voxel probe was missing. That probe
measured geometry only, and a grey voxel blob is not a "still looks good" model.

## The encoding

    header    magic, version, grid, palette count, voxel count
    palette   3 bytes per entry, trimmed to what the model uses
    voxels    x, y, z packed to the grid, plus a palette index

Colour comes from the model's own textures by UV, so the palette is genuinely
the model's, not a guess. Median-cut rather than k-means: it is deterministic,
which matters when the output is committed and diffed, and on a model with a few
strong material colours it lands in the same place anyway.
"""
import sys, os, gzip, math, struct, argparse, collections


def load_obj_uv(path):
    verts, uvs, tris = [], [], []
    for line in open(path):
        if line.startswith("v "):
            verts.append(tuple(float(x) for x in line.split()[1:4]))
        elif line.startswith("vt "):
            uvs.append(tuple(float(x) for x in line.split()[1:3]))
        elif line.startswith("f "):
            f = []
            for tok in line.split()[1:]:
                parts = tok.split("/")
                vi = int(parts[0]) - 1
                ti = int(parts[1]) - 1 if len(parts) > 1 and parts[1] else -1
                f.append((vi, ti))
            for i in range(1, len(f) - 1):
                tris.append((f[0], f[i], f[i + 1]))
    return verts, uvs, tris


def sample(img, u, v):
    if img is None:
        return (128, 128, 128)
    w, h = img.size
    # OBJ V runs upward, image rows run downward; and these rips carry UVs
    # outside 0..1, so wrap rather than clamp — clamping smears an edge pixel
    # across every out-of-range face, which reads as a coloured stripe.
    x = int((u % 1.0) * (w - 1))
    y = int((1.0 - (v % 1.0)) * (h - 1))
    return img.getpixel((x, y))[:3]


def voxelise_coloured(objs, texture, n=48):
    from PIL import Image
    img = Image.open(texture).convert("RGB") if texture else None
    allv = []
    data = []
    for p in objs:
        V, T, F = load_obj_uv(p)
        data.append((V, T, F))
        allv.extend(V)
    lo = [min(v[i] for v in allv) for i in range(3)]
    hi = [max(v[i] for v in allv) for i in range(3)]
    span = max(hi[i] - lo[i] for i in range(3)) or 1.0
    step = span / n
    cells = {}
    for V, T, F in data:
        for tri in F:
            pts = [V[a] for a, _ in tri]
            uvi = [t for _, t in tri]
            e = max(math.dist(pts[0], pts[1]), math.dist(pts[1], pts[2]),
                    math.dist(pts[2], pts[0]))
            k = max(1, int(e / step) + 1)
            for i in range(k + 1):
                for j in range(k + 1 - i):
                    a, b = i / k, j / k
                    c = 1.0 - a - b
                    P = [pts[0][d] * c + pts[1][d] * a + pts[2][d] * b for d in range(3)]
                    cell = tuple(min(n - 1, max(0, int((P[d] - lo[d]) / step)))
                                 for d in range(3))
                    if cell in cells:
                        continue
                    col = (128, 128, 128)
                    if T and all(t >= 0 and t < len(T) for t in uvi):
                        u = T[uvi[0]][0] * c + T[uvi[1]][0] * a + T[uvi[2]][0] * b
                        v = T[uvi[0]][1] * c + T[uvi[1]][1] * a + T[uvi[2]][1] * b
                        col = sample(img, u, v)
                    cells[cell] = col
    return cells, lo, step


def median_cut(colours, k=32):
    """Trim the palette to k entries, deterministically.

    Median cut rather than k-means: the output is committed and diffed, so a
    stable result matters more than a marginally better one, and on a model with
    a few strong material colours the two land in the same place.
    """
    boxes = [list(colours)]
    while len(boxes) < k:
        boxes.sort(key=lambda b: -(len(b) and max(
            max(c[i] for c in b) - min(c[i] for c in b) for i in range(3))))
        big = boxes.pop(0)
        if len(big) < 2:
            boxes.append(big)
            break
        ch = max(range(3), key=lambda i: max(c[i] for c in big) - min(c[i] for c in big))
        big.sort(key=lambda c: c[ch])
        boxes.append(big[:len(big) // 2])
        boxes.append(big[len(big) // 2:])
    pal = []
    for b in boxes:
        if not b:
            continue
        pal.append(tuple(sum(c[i] for c in b) // len(b) for i in range(3)))
    return pal


def encode(cells, lo, step, n, palette):
    """Pack to the SVOX-inspired layout, widened where SVOX is too narrow.

    SVOX spends one byte on the voxel count, capping a file at 255 voxels. Our
    models want tens of thousands, so counts are u32 and coordinates u16 — the
    idea travels, the field widths do not.
    """
    def nearest(c):
        return min(range(len(palette)),
                   key=lambda i: sum((palette[i][d] - c[d]) ** 2 for d in range(3)))
    out = bytearray(b"SVXR\x01")
    out += struct.pack("<HHI", n, len(palette), len(cells))
    out += struct.pack("<3f", *lo)
    out += struct.pack("<f", step)
    for c in palette:
        out += bytes(c)
    for cell, col in sorted(cells.items()):
        out += struct.pack("<3H", *cell)
        out += bytes([nearest(col)])
    return bytes(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("objs", nargs="+")
    ap.add_argument("--texture", default=None)
    ap.add_argument("--grid", type=int, default=48)
    ap.add_argument("--palette", type=int, default=32)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    cells, lo, step = voxelise_coloured(a.objs, a.texture, a.grid)
    pal = median_cut(list(cells.values()), a.palette)
    blob = encode(cells, lo, step, a.grid, pal)
    gz = gzip.compress(blob, 9)
    src = sum(os.path.getsize(p) for p in a.objs)
    print("voxels       : %d at %d^3" % (len(cells), a.grid))
    print("palette      : %d entries" % len(pal))
    print("raw          : %d bytes" % len(blob))
    print("gzipped      : %d bytes  (%.2f%% of %.1f MB of .obj)"
          % (len(gz), 100.0 * len(gz) / src, src / 1e6))
    if a.out:
        open(a.out, "wb").write(gz)
    return 0


if __name__ == "__main__":
    sys.exit(main())
