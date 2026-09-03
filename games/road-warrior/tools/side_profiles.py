#!/usr/bin/env python3
"""Extract side-view silhouettes from .glb car models.

Road Warrior draws its cars as a handful of rotated boxes in the fragment
shader. That is legible and cheap, and it is also the reason every car
reads as the same vehicle in a different colour. blackstar-rs ships five
real car meshes; this pulls a 2D side profile out of each so the shader
can have an actual outline to work with.

WHAT COMES OUT

For each model, `samples` columns along the car's length, each holding the
lowest and highest surface at that column, normalised so the car is 1.0
long and sits with its lowest point at y=0. That is the same shape as the
terrain columns already in the shader: an array the fragment stage can
index and interpolate. It is a SILHOUETTE, not a mesh — a side-view game
does not need the geometry it is throwing away.

WHY OFFLINE

The .glb files total 15MB and parsing them needs a glTF reader. Doing this
at startup would mean shipping the models and a parser with the game to
recompute a few hundred floats that never change. This runs once, by hand,
and its output is checked in.

Deliberately dependency-free: GLB is a 12-byte header followed by a JSON
chunk and a binary chunk, and the accessor indirection is about thirty
lines. Adding pygltflib to a repo that has no Python dependencies to
extract five outlines is not a trade worth making.

USAGE
    python3 side_profiles.py <out.json> <model.glb> [model.glb ...]
"""

import json
import struct
import sys

# glTF component types we need to read positions and indices.
COMPONENT = {
    5120: ("b", 1),   # BYTE
    5121: ("B", 1),   # UNSIGNED_BYTE
    5122: ("h", 2),   # SHORT
    5123: ("H", 2),   # UNSIGNED_SHORT
    5125: ("I", 4),   # UNSIGNED_INT
    5126: ("f", 4),   # FLOAT
}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}

SAMPLES = 48


def read_glb(path):
    """Return (gltf_json, binary_chunk)."""
    with open(path, "rb") as fh:
        data = fh.read()
    magic, _version, length = struct.unpack("<III", data[:12])
    if magic != 0x46546C67:
        raise ValueError(f"{path}: not a GLB (magic {magic:#x})")
    js, binary = None, b""
    off = 12
    while off < length:
        clen, ctype = struct.unpack("<II", data[off : off + 8])
        body = data[off + 8 : off + 8 + clen]
        if ctype == 0x4E4F534A:      # 'JSON'
            js = json.loads(body)
        elif ctype == 0x004E4942:    # 'BIN'
            binary = body
        off += 8 + clen
    if js is None:
        raise ValueError(f"{path}: no JSON chunk")
    return js, binary


def accessor(gltf, binary, index):
    """Read one accessor into a list of tuples."""
    acc = gltf["accessors"][index]
    fmt, size = COMPONENT[acc["componentType"]]
    n = NCOMP[acc["type"]]
    view = gltf["bufferViews"][acc["bufferView"]]
    base = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    # An interleaved buffer gives a stride; a tightly packed one does not.
    stride = view.get("byteStride") or size * n
    out = []
    for i in range(acc["count"]):
        start = base + i * stride
        out.append(struct.unpack_from("<" + fmt * n, binary, start))
    return out


def triangles(gltf, binary):
    """Every triangle in the file, as ((x,y,z), (x,y,z), (x,y,z))."""
    tris = []
    for mesh in gltf.get("meshes", []):
        for prim in mesh.get("primitives", []):
            pos_idx = prim.get("attributes", {}).get("POSITION")
            if pos_idx is None:
                continue
            pos = accessor(gltf, binary, pos_idx)
            if "indices" in prim:
                idx = [i[0] for i in accessor(gltf, binary, prim["indices"])]
            else:
                idx = list(range(len(pos)))
            for i in range(0, len(idx) - 2, 3):
                tris.append((pos[idx[i]], pos[idx[i + 1]], pos[idx[i + 2]]))
    return tris


def positions(gltf, binary):
    """Every vertex position in the file, across all meshes and primitives."""
    pts = []
    for mesh in gltf.get("meshes", []):
        for prim in mesh.get("primitives", []):
            idx = prim.get("attributes", {}).get("POSITION")
            if idx is not None:
                pts.extend(accessor(gltf, binary, idx))
    return pts


def profile(pts, tris, samples=SAMPLES):
    """Lower and upper silhouette across the car's length.

    Axes are chosen by EXTENT, not by convention: length is the longest
    span, height the shortest, width whatever is left. A car is longer than
    it is wide and wider than it is tall, and that ordering holds however
    the model was authored.

    Assuming Y-up would have been wrong. Four of blackstar's five cars are
    Y-up with length along X or Z; `backfire` is authored with its LENGTH
    along Y, and reading Y as height gave it an aspect ratio of 1.89 — a
    car nearly twice as tall as it is long. That number is what caught it,
    which is why `aspect` is printed per model and sanity-checked below.
    """
    cols = list(zip(*pts))[:3]
    spans = [max(c) - min(c) for c in cols]
    length_axis = spans.index(max(spans))
    height_axis = spans.index(min(spans))
    along = cols[length_axis]
    ys = cols[height_axis]
    axis = "xyz"[length_axis]

    lo_a, hi_a = min(along), max(along)
    lo_y, hi_y = min(ys), max(ys)
    length = hi_a - lo_a
    height = hi_y - lo_y
    if length <= 0 or height <= 0:
        raise ValueError("degenerate model bounds")

    # Normalise: 1.0 long, lowest point at 0, height in the same units as
    # length so the aspect ratio survives.
    lower = [None] * samples
    upper = [None] * samples

    def mark(col, ny):
        if lower[col] is None or ny < lower[col]:
            lower[col] = ny
        if upper[col] is None or ny > upper[col]:
            upper[col] = ny

    # Walk EDGES, not vertices.
    #
    # Sampling vertices alone leaves columns empty wherever a low-poly hull
    # has no vertex ring, and an empty column inheriting its neighbour
    # punched deep notches through three of the five cars — visible as
    # vertical spikes in the first render of these profiles. An edge that
    # spans several columns contributes to all of them, which is what makes
    # a coarse mesh give a solid outline.
    for tri in tris:
        for a_pt, b_pt in ((tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])):
            a0, y0 = a_pt[length_axis], a_pt[height_axis]
            a1, y1 = b_pt[length_axis], b_pt[height_axis]
            c0 = (a0 - lo_a) / length * (samples - 1)
            c1 = (a1 - lo_a) / length * (samples - 1)
            steps = max(1, int(abs(c1 - c0)) + 1)
            for k in range(steps + 1):
                f = k / steps
                col = min(int(round(c0 + (c1 - c0) * f)), samples - 1)
                ny = ((y0 + (y1 - y0) * f) - lo_y) / length
                mark(max(col, 0), ny)

    # Anything still empty is outside the mesh entirely; carry the last
    # known value so the outline closes rather than dropping to zero.
    for arr in (lower, upper):
        last = next((v for v in arr if v is not None), 0.0)
        for i, v in enumerate(arr):
            if v is None:
                arr[i] = last
            else:
                last = v

    aspect = height / length
    # A road car sits somewhere around a quarter to a third as tall as it is
    # long. Well outside that means the axes were picked wrong, and a
    # silently mis-oriented silhouette looks like a modelling problem rather
    # than a tooling one.
    if not 0.10 <= aspect <= 0.60:
        print(
            f"  WARNING: aspect {aspect:.3f} is not car-shaped; check the axes",
            file=sys.stderr,
        )

    return {
        "axis": axis,
        "aspect": round(aspect, 4),
        "lower": [round(v, 4) for v in lower],
        "upper": [round(v, 4) for v in upper],
    }


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    out_path, models = argv[1], argv[2:]
    result = {"samples": SAMPLES, "cars": {}}
    for path in models:
        name = path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
        gltf, binary = read_glb(path)
        pts = positions(gltf, binary)
        if not pts:
            print(f"  {name}: no POSITION data, skipped", file=sys.stderr)
            continue
        prof = profile(pts, triangles(gltf, binary))
        result["cars"][name] = prof
        print(f"  {name}: {len(pts)} verts, axis {prof['axis']}, aspect {prof['aspect']:.3f}")
    with open(out_path, "w") as fh:
        json.dump(result, fh, indent=1)
    print(f"wrote {out_path} ({len(result['cars'])} cars)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
