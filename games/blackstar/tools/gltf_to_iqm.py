#!/usr/bin/env python3
"""gltf_to_iqm.py -- convert a glTF 2.0 / GLB model (e.g. a Blender export) into the
engine's IQM part format, so hand-modelled assets drop straight into data/ and get
used exactly like the procedural parts.

glTF is the FOSS interchange standard and Blender's cleanest export ("glTF 2.0 (.glb)").
This reads a .glb (single binary file) or a .gltf + .bin pair, flattens the scene graph
(baking every node transform into world space), merges all mesh primitives, and writes
a static IQM using the SAME writer as obj_to_iqm (one canonical IQM format in the repo).

Usage:
  python3 tools/gltf_to_iqm.py model.glb --out engine_src/data/phead.iqm --tex hexmesh.png
  python3 tools/gltf_to_iqm.py model.gltf --out data/thing.iqm --raw     # keep coords
  python3 tools/gltf_to_iqm.py model.glb  --out data/thing.iqm --z-up    # export was Z-up

Notes:
- Blender's glTF exporter already converts Z-up -> glTF Y-up, which matches the engine's
  Y-up parts, so the default needs no axis flag. Pass --z-up only if you exported with
  "+Y up" disabled.
- Default (no --raw) re-centres + scales the mesh into the engine's 2-unit part slot,
  the same convention every procedural part + obj_to_iqm uses, so a Blender head named
  phead.iqm simply overrides the procedural head. Validate the result with
  tests/test_spine_iqm.c (structural) exactly like any other part.
"""
import sys, os, json, struct, math, argparse
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from obj_to_iqm import write_iqm, normalize_slot   # one canonical IQM writer

# ---- glTF accessor plumbing -------------------------------------------------
_CT = {5120:("b",1), 5121:("B",1), 5122:("h",2), 5123:("H",2), 5125:("I",4), 5126:("f",4)}
_NC = {"SCALAR":1, "VEC2":2, "VEC3":3, "VEC4":4, "MAT4":16}

def load_gltf(path):
    """Return (gltf_dict, [buffer_bytes,...]) for a .glb or .gltf file."""
    data = open(path, "rb").read()
    if data[:4] == b"glTF":                                  # binary GLB container
        _, _, total = struct.unpack("<4sII", data[:12])[0:3] if False else (None,None,None)
        ver, total = struct.unpack("<II", data[4:12])
        off = 12; gltf = None; bin_chunk = b""
        while off < len(data):
            clen, ctype = struct.unpack("<II", data[off:off+8]); off += 8
            chunk = data[off:off+clen]; off += clen
            if ctype == 0x4E4F534A:      gltf = json.loads(chunk.decode("utf-8"))
            elif ctype == 0x004E4942:    bin_chunk = chunk
        return gltf, [bin_chunk]
    # text .gltf: buffers reference external .bin (or embedded base64 data: URIs)
    gltf = json.loads(data.decode("utf-8"))
    bufs = []
    for b in gltf.get("buffers", []):
        uri = b.get("uri", "")
        if uri.startswith("data:"):
            import base64
            bufs.append(base64.b64decode(uri.split(",", 1)[1]))
        elif uri:
            bufs.append(open(os.path.join(os.path.dirname(path), uri), "rb").read())
        else:
            bufs.append(b"")
    return gltf, bufs

def accessor(gltf, bufs, idx):
    """Read accessor idx -> list of tuples (or scalars for SCALAR)."""
    acc = gltf["accessors"][idx]
    ncomp = _NC[acc["type"]]; fmt, sz = _CT[acc["componentType"]]
    count = acc["count"]; a_off = acc.get("byteOffset", 0)
    bv = gltf["bufferViews"][acc["bufferView"]]
    buf = bufs[bv.get("buffer", 0)]
    base = bv.get("byteOffset", 0) + a_off
    stride = bv.get("byteStride", 0) or ncomp * sz
    out = []
    for i in range(count):
        p = base + i * stride
        vals = struct.unpack("<" + fmt * ncomp, buf[p:p + ncomp * sz])
        out.append(vals[0] if ncomp == 1 else vals)
    return out

# ---- 4x4 column-major matrix helpers (stdlib only) --------------------------
def mat_id(): return [1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1]
def mat_mul(a, b):
    r = [0.0]*16
    for c in range(4):
        for row in range(4):
            r[c*4+row] = sum(a[k*4+row]*b[c*4+k] for k in range(4))
    return r
def mat_from_trs(t, q, s):
    x,y,z,w = q; xx,yy,zz = x*x,y*y,z*z; xy,xz,yz = x*y,x*z,y*z; wx,wy,wz = w*x,w*y,w*z
    R = [1-2*(yy+zz), 2*(xy+wz),   2*(xz-wy),   0,
         2*(xy-wz),   1-2*(xx+zz), 2*(yz+wx),   0,
         2*(xz+wy),   2*(yz-wx),   1-2*(xx+yy), 0,
         0,0,0,1]
    S = [s[0],0,0,0, 0,s[1],0,0, 0,0,s[2],0, 0,0,0,1]
    RS = mat_mul(R, S); RS[12],RS[13],RS[14] = t[0],t[1],t[2]
    return RS
def node_local(n):
    if "matrix" in n: return list(n["matrix"])
    t = n.get("translation", [0,0,0]); q = n.get("rotation", [0,0,0,1]); s = n.get("scale", [1,1,1])
    return mat_from_trs(t, q, s)
def xform_pt(m, p):
    return (m[0]*p[0]+m[4]*p[1]+m[8]*p[2]+m[12],
            m[1]*p[0]+m[5]*p[1]+m[9]*p[2]+m[13],
            m[2]*p[0]+m[6]*p[1]+m[10]*p[2]+m[14])
def xform_nrm(m, v):   # rotation/scale only, then renormalise
    x = m[0]*v[0]+m[4]*v[1]+m[8]*v[2]; y = m[1]*v[0]+m[5]*v[1]+m[9]*v[2]; z = m[2]*v[0]+m[6]*v[1]+m[10]*v[2]
    l = math.sqrt(x*x+y*y+z*z) or 1.0
    return (x/l, y/l, z/l)

# ---- flatten scene -> merged triangle soup ----------------------------------
def collect(gltf, bufs):
    verts, norms, uvs, tris = [], [], [], []
    def emit_mesh(mi, world):
        for prim in gltf["meshes"][mi].get("primitives", []):
            if prim.get("mode", 4) != 4:       # 4 = TRIANGLES; skip lines/points
                continue
            at = prim["attributes"]
            P = accessor(gltf, bufs, at["POSITION"])
            N = accessor(gltf, bufs, at["NORMAL"]) if "NORMAL" in at else None
            T = accessor(gltf, bufs, at["TEXCOORD_0"]) if "TEXCOORD_0" in at else None
            idx = accessor(gltf, bufs, prim["indices"]) if "indices" in prim else list(range(len(P)))
            base = len(verts)
            for i, p in enumerate(P):
                verts.append(xform_pt(world, p))
                norms.append(xform_nrm(world, N[i]) if N else (0.0,1.0,0.0))
                uv = T[i] if T else (0.0,0.0)
                uvs.append((uv[0], uv[1]))
            for k in range(0, len(idx) - 2, 3):
                tris.append((base+idx[k], base+idx[k+1], base+idx[k+2]))
    def walk(ni, parent):
        n = gltf["nodes"][ni]; world = mat_mul(parent, node_local(n))
        if "mesh" in n: emit_mesh(n["mesh"], world)
        for c in n.get("children", []): walk(c, world)
    scene = gltf.get("scene", 0)
    roots = gltf.get("scenes", [{}])[scene].get("nodes", list(range(len(gltf.get("nodes", [])))))
    for r in roots: walk(r, mat_id())
    return verts, norms, uvs, tris

def main():
    ap = argparse.ArgumentParser(description="glTF/GLB -> IQM (engine part format)")
    ap.add_argument("gltf")
    ap.add_argument("--out", required=True)
    ap.add_argument("--tex", default="hexmesh.png")
    ap.add_argument("--raw", action="store_true", help="keep coords (skip unit-slot normalise)")
    ap.add_argument("--z-up", action="store_true", help="export was Z-up: rotate into engine Y-up")
    a = ap.parse_args()
    gltf, bufs = load_gltf(a.gltf)
    if gltf is None or not gltf.get("meshes"):
        raise SystemExit("gltf_to_iqm: no meshes found in " + a.gltf)
    verts, norms, uvs, tris = collect(gltf, bufs)
    if not tris: raise SystemExit("gltf_to_iqm: no triangles found in " + a.gltf)
    if a.z_up:   # (x,y,z)_zup -> (x,z,-y)_yup
        verts = [(v[0], v[2], -v[1]) for v in verts]
        norms = [(n[0], n[2], -n[1]) for n in norms]
    if not a.raw: verts = normalize_slot(verts)
    nv, nt, sz = write_iqm(a.out, verts, norms, uvs, tris, a.tex)
    print(f"wrote {a.out}: {nv} verts {nt} tris {sz} bytes  (from {len(gltf['meshes'])} glTF mesh(es))")

if __name__ == "__main__":
    main()
