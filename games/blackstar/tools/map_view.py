#!/usr/bin/env python3
"""map_view.py -- top-down collider map visualizer.

Reads a .col file (cx cy cz hx hy hz per line) and renders an orthographic
top-down PNG of the collider footprints, shaded by height, so a map's spatial
layout + spacing can be INSPECTED for legibility (the Destiny principle:
players must be able to read the space). Complements map_score.py, which gives
numbers; this shows the shape.

  python3 tools/map_view.py engine_src/data/wastes.col --out /tmp/wastes.png
"""
import struct, zlib, sys, argparse, os


def load_col(path):
    boxes = []
    for line in open(path):
        p = line.split()
        if len(p) < 6:
            continue
        boxes.append(tuple(float(x) for x in p[:6]))  # cx cy cz hx hy hz
    return boxes


def write_png(path, W, H, rgb):
    def chunk(typ, data):
        c = typ + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xffffffff)
    raw = bytearray()
    for y in range(H):
        raw.append(0)
        raw += rgb[y*W*3:(y+1)*W*3]
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    open(path, "wb").write(png)


def render(boxes, S=520, pad=6.0):
    xs = [b[0] for b in boxes]; zs = [b[2] for b in boxes]
    hx = [b[3] for b in boxes]; hz = [b[5] for b in boxes]
    minx = min(x-h for x, h in zip(xs, hx)) - pad
    maxx = max(x+h for x, h in zip(xs, hx)) + pad
    minz = min(z-h for z, h in zip(zs, hz)) - pad
    maxz = max(z+h for z, h in zip(zs, hz)) + pad
    span = max(maxx-minx, maxz-minz)
    sc = (S-1)/span
    img = bytearray([22, 24, 30] * (S*S))   # dark floor

    heights = [b[1]+b[4] for b in boxes]
    hmax = max(heights) or 1.0
    # draw taller boxes last so they sit on top
    order = sorted(range(len(boxes)), key=lambda i: heights[i])
    for i in order:
        cx, cy, cz, bx, by, bz = boxes[i]
        top = cy + by
        t = top / hmax
        # low cover cool/dim, tall structures bright/warm -- height reads as value
        r = int(60 + 150*t); g = int(90 + 120*t); b = int(130 + 40*(1-t))
        x0 = int((cx-bx - minx)*sc); x1 = int((cx+bx - minx)*sc)
        z0 = int((cz-bz - minz)*sc); z1 = int((cz+bz - minz)*sc)
        for py in range(max(0, z0), min(S-1, z1)+1):
            row = py*S*3
            for px in range(max(0, x0), min(S-1, x1)+1):
                o = row + px*3
                img[o] = min(255, r); img[o+1] = min(255, g); img[o+2] = min(255, b)
    return img, S


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("col")
    ap.add_argument("--out", default="/tmp/map_view.png")
    args = ap.parse_args()
    boxes = load_col(args.col)
    if not boxes:
        print("no colliders in", args.col); sys.exit(1)
    img, S = render(boxes)
    write_png(args.out, S, S, img)
    print("wrote %s  (%d colliders from %s)" % (args.out, len(boxes), os.path.basename(args.col)))


if __name__ == "__main__":
    main()
