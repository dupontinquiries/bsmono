#!/usr/bin/env python3
"""silhouette.py -- offline bot silhouette renderer.

Composites the JSON-compiled part meshes at the ENGINE'S real per-part layout
(from the pose_dump harness) and rasterizes an orthographic front+side view to
a PNG, with simple depth shading. Lets us SEE proportions -- the standing
"screenshot-unverified" gap from Updates 209-215 -- without a GPU/shader.

  # build the pose harness once:
  gcc -o /tmp/pose_dump tools/pose_dump.c engine_src/exengine/robot_parts.c \
      -I engine_src/exengine -lm

  # render the player body with the prime frame:
  python3 tools/silhouette.py --kind 0 --frame prime --out /tmp/prime.png

Part->mesh map mirrors game.c part_template() + the variant swaps.
"""
import struct, sys, os, subprocess, zlib, math, argparse

DATA = os.path.join(os.path.dirname(__file__), "..", "engine_src", "data")

# part index -> base mesh file (mirrors part_template() in game.c)
PART_MESH = {
    0:  "pchest.iqm",   1:  "pbevel.iqm",  2:  "phead.iqm",   3:  None,       # 3 visor = tiny box, skip
    4:  "ppauldron.iqm",5:  "ppauldron.iqm",
    6:  "plimb.iqm",    7:  "plimb.iqm",   8:  "plimb.iqm",   9:  "plimb.iqm",
    10: "plimb.iqm",    11: "plimb.iqm",   12: "spine_wing_cannon.iqm",
    13: "phand.iqm",    14: "phand.iqm",   15: "pbevel.iqm",  16: "pbevel.iqm",
}
# which slots the named frame variants replace
VARIANT_SLOTS = {2: "phead", 0: "pchest", 4: "ppauldron", 5: "ppauldron"}


def load_iqm(path):
    """Return (verts, tris) from an IQM written by gen_props.write_iqm."""
    d = open(path, "rb").read()
    assert d[:16] == b"INTERQUAKEMODEL\x00", path
    (ver, filesize, flags) = struct.unpack_from("<3I", d, 16)
    n_text, ofs_text = struct.unpack_from("<2I", d, 28)
    n_mesh, ofs_mesh = struct.unpack_from("<2I", d, 36)
    n_va, n_vert, ofs_va = struct.unpack_from("<3I", d, 44)
    n_tri, ofs_tri, ofs_adj = struct.unpack_from("<3I", d, 56)
    # vertex arrays: find position array (type 0)
    pos_ofs = None
    for a in range(n_va):
        typ, flg, fmt, size, off = struct.unpack_from("<5I", d, ofs_va + a * 20)
        if typ == 0:
            pos_ofs = off
    verts = []
    for i in range(n_vert):
        x, y, z = struct.unpack_from("<3f", d, pos_ofs + i * 12)
        verts.append((x, y, z))
    tris = []
    for i in range(n_tri):
        a, b, c = struct.unpack_from("<3I", d, ofs_tri + i * 12)
        tris.append((a, b, c))
    return verts, tris


def get_poses(kind, vehicle=0, alt=0, cstage=0):
    out = subprocess.check_output(["/tmp/pose_dump", str(kind), str(vehicle), str(alt), str(cstage)]).decode()
    poses = {}
    for line in out.strip().splitlines():
        p = line.split("\t")
        i = int(p[0])
        poses[i] = tuple(float(x) for x in p[1:7]) + tuple(int(x) for x in p[7:10])  # +rgb
    return poses


def part_mesh_path(i, frame):
    base = PART_MESH.get(i)
    if base is None:
        return None
    if frame and i in VARIANT_SLOTS:
        cand = os.path.join(DATA, base.replace(".iqm", "_%s.iqm" % frame))
        if os.path.exists(cand):
            return cand
    return os.path.join(DATA, base)


def build_scene(kind, frame, alt=0, colored=False, cstage=0):
    """Return list of world-space triangles: (v0,v1,v2[,rgb]) each vert (x,y,z)."""
    poses = get_poses(kind, 0, alt, cstage)
    tris_world = []
    for i, pose in poses.items():
        mp = part_mesh_path(i, frame)
        if not mp or not os.path.exists(mp):
            continue
        ox, oy, oz, sx, sy, sz = pose[:6]
        rgb = pose[6:9] if len(pose) >= 9 else (150, 170, 200)
        if sx == 0 and sy == 0 and sz == 0:
            continue
        verts, tris = load_iqm(mp)
        wv = [(vx * sx + ox, vy * sy + oy, vz * sz + oz) for (vx, vy, vz) in verts]
        for (a, b, c) in tris:
            if colored:
                tris_world.append((wv[a], wv[b], wv[c], rgb))
            else:
                tris_world.append((wv[a], wv[b], wv[c]))
    return tris_world


# ---- tiny software rasterizer (filled triangles, depth-shaded) ----
def render_view(tris, W, H, axis_x, axis_y, axis_depth, flip_x=False):
    xs = [t[k][axis_x] for t in tris for k in range(3)]
    ys = [t[k][axis_y] for t in tris for k in range(3)]
    minx, maxx = min(xs), max(xs)
    miny, maxy = min(ys), max(ys)
    pad = 0.12 * max(maxx - minx, maxy - miny)
    minx -= pad; maxx += pad; miny -= pad; maxy += pad
    span = max(maxx - minx, maxy - miny)
    sc = (min(W, H) - 1) / span

    img = bytearray([18, 20, 26] * (W * H))   # dark bg
    zbuf = [1e9] * (W * H)

    def proj(v):
        px = (v[axis_x] - minx) * sc
        if flip_x: px = (W - 1) - px
        py = (H - 1) - (v[axis_y] - miny) * sc   # y up
        return px, py, v[axis_depth]

    dvals = [t[k][axis_depth] for t in tris for k in range(3)]
    dmin, dmax = min(dvals), max(dvals)
    drange = (dmax - dmin) or 1.0

    for tri in tris:
        a, b, c = tri[0], tri[1], tri[2]
        tri_rgb = tri[3] if len(tri) > 3 else None
        pa, pb, pc = proj(a), proj(b), proj(c)
        # backface-agnostic: fill all; depth test resolves overlap
        minX = max(0, int(min(pa[0], pb[0], pc[0])))
        maxX = min(W - 1, int(max(pa[0], pb[0], pc[0])) + 1)
        minY = max(0, int(min(pa[1], pb[1], pc[1])))
        maxY = min(H - 1, int(max(pa[1], pb[1], pc[1])) + 1)
        if maxX < minX or maxY < minY:
            continue
        x0, y0 = pa[0], pa[1]; x1, y1 = pb[0], pb[1]; x2, y2 = pc[0], pc[1]
        denom = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2)
        if abs(denom) < 1e-9:
            continue
        for py in range(minY, maxY + 1):
            for px in range(minX, maxX + 1):
                w0 = ((y1 - y2) * (px - x2) + (x2 - x1) * (py - y2)) / denom
                w1 = ((y2 - y0) * (px - x2) + (x0 - x2) * (py - y2)) / denom
                w2 = 1 - w0 - w1
                if w0 < 0 or w1 < 0 or w2 < 0:
                    continue
                z = w0 * pa[2] + w1 * pb[2] + w2 * pc[2]
                idx = py * W + px
                if z < zbuf[idx]:
                    zbuf[idx] = z
                    shade = 0.55 + 0.45 * (1.0 - (z - dmin) / drange)  # nearer = brighter
                    if tri_rgb is not None:
                        r = int(tri_rgb[0] * shade); g = int(tri_rgb[1] * shade); b = int(tri_rgb[2] * shade)
                    else:
                        r = int(90 * shade + 60); g = int(150 * shade + 40); b = int(210 * shade + 30)
                    o = idx * 3
                    img[o] = min(255, r); img[o + 1] = min(255, g); img[o + 2] = min(255, b)
    return img


def write_png(path, W, H, rgb):
    def chunk(typ, data):
        c = typ + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xffffffff)
    raw = bytearray()
    for y in range(H):
        raw.append(0)
        raw += rgb[y * W * 3:(y + 1) * W * 3]
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    open(path, "wb").write(png)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", type=int, default=0, help="body kind 0..5")
    ap.add_argument("--frame", default="", help="'', prime, corsair, watch")
    ap.add_argument("--alt", type=int, default=0, help="alt-mode remnant family (0..19)")
    ap.add_argument("--stage", type=int, default=0, help="corruption stage 1..3 (kind 2)")
    ap.add_argument("--color", action="store_true", help="render with engine per-part tint/weathering")
    ap.add_argument("--out", default="/tmp/silhouette.png")
    ap.add_argument("--size", type=int, default=360)
    args = ap.parse_args()

    tris = build_scene(args.kind, args.frame or None, args.alt, args.color, args.stage)
    if not tris:
        print("no geometry"); sys.exit(1)
    S = args.size
    # front view (x right, y up, z depth) | side view (z right, y up, x depth)
    front = render_view(tris, S, S, 0, 1, 2)
    side = render_view(tris, S, S, 2, 1, 0, flip_x=True)
    W = S * 2 + 12
    canvas = bytearray([18, 20, 26] * (W * S))
    for y in range(S):
        canvas[y * W * 3: y * W * 3 + S * 3] = front[y * S * 3:(y + 1) * S * 3]
        o = (y * W + S + 12) * 3
        canvas[o:o + S * 3] = side[y * S * 3:(y + 1) * S * 3]
    write_png(args.out, W, S, canvas)
    print("wrote %s  (%d triangles, front|side)" % (args.out, len(tris)))


if __name__ == "__main__":
    main()
