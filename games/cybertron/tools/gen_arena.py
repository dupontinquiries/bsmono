#!/usr/bin/env python3
"""gen_arena.py -- generates data/arena.iqm, a large space-outpost arena.

IQM v2, static (no joints/anims): positions, texcoords, normals + triangles,
meshes keyed by material name so the engine's texture cache picks up the
existing floor.png / wall.png / rock.png (+ spec_/norm_ variants).

Layout (120x120 units, ~3x the demo level):
  - big tiled floor
  - 4 perimeter walls
  - central raised platform with 4 wedge ramps
  - scattered rock pillars as landmarks/hard cover anchors

Regen:  python3 tools/gen_arena.py   (writes engine_src/data/arena.iqm)
"""
import struct, os, sys, math, random


# --- theme params (argv: name floortex walltex rocktex pillars seed) ---
NAME   = sys.argv[1] if len(sys.argv)>1 else "arena"
FLOORT = sys.argv[2] if len(sys.argv)>2 else "floor.png"
WALLT  = sys.argv[3] if len(sys.argv)>3 else "wall.png"
ROCKT  = sys.argv[4] if len(sys.argv)>4 else "rock.png"
NPILL  = int(sys.argv[5]) if len(sys.argv)>5 else 10
SEED   = int(sys.argv[6]) if len(sys.argv)>6 else 7

verts = []   # (pos3, uv2, normal3)
tris  = []   # (a, b, c) CCW viewed from outside (front face)
meshes = []  # (material, first_vertex, num_vertexes, first_tri, num_tris)

def quad(mat_bucket, a, b, c, d, n, uvscale=0.25):
    """Add quad a-b-c-d (CCW from the normal side). UVs from world coords."""
    base = len(verts)
    ax = 0 if abs(n[0]) < 0.9 else 2   # pick two world axes for UVs
    ay = 1 if abs(n[1]) < 0.9 else 2
    for p in (a, b, c, d):
        verts.append((p, (p[ax]*uvscale, p[ay]*uvscale), n))
    tris.append((base, base+1, base+2))
    tris.append((base, base+2, base+3))
    mat_bucket.append((base, 4, len(tris)-2, 2))

def box(mat_bucket, cx, cy, cz, hx, hy, hz):
    x0,x1,y0,y1,z0,z1 = cx-hx,cx+hx,cy-hy,cy+hy,cz-hz,cz+hz
    quad(mat_bucket,(x0,y1,z0),(x0,y1,z1),(x1,y1,z1),(x1,y1,z0),(0,1,0))    # top
    quad(mat_bucket,(x0,y0,z1),(x0,y0,z0),(x1,y0,z0),(x1,y0,z1),(0,-1,0))   # bottom
    quad(mat_bucket,(x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1),(0,0,1))    # +z
    quad(mat_bucket,(x1,y0,z0),(x0,y0,z0),(x0,y1,z0),(x1,y1,z0),(0,0,-1))   # -z
    quad(mat_bucket,(x1,y0,z1),(x1,y0,z0),(x1,y1,z0),(x1,y1,z1),(1,0,0))    # +x
    quad(mat_bucket,(x0,y0,z0),(x0,y0,z1),(x0,y1,z1),(x0,y1,z0),(-1,0,0))   # -x

def wedge(mat_bucket, cx, cz, w, run, h, facing):
    """Ramp rising from ground to h over `run`, sloping toward `facing` (+z/-z/+x/-x)."""
    def rot(p):
        x,y,z = p
        if facing == '+z': return (x, y, z)
        if facing == '-z': return (-x, y, -z)
        if facing == '+x': return (z, y, x)
        return (-z, y, -x)
    def P(x,y,z):
        r = rot((x,y,z)); return (cx+r[0], y, cz+r[2])
    lo0,lo1 = P(-w,0,run), P(w,0,run)         # ground edge (far)
    hi0,hi1 = P(-w,h,0),  P(w,h,0)            # top edge (near platform)
    g0,g1   = P(-w,0,0),  P(w,0,0)            # ground under top edge
    # slope normal (unnormalised is fine for lighting-ish)
    quad(mat_bucket, lo0, lo1, hi1, hi0, (0, 0.8, 0.6) if facing in ('+z','-z') else (0.6,0.8,0))
    # sides
    tris_side = [(lo0, hi0, g0), (lo1, g1, hi1)]
    for a,b,c in tris_side:
        base = len(verts)
        for p in (a,b,c):
            verts.append((p, (p[0]*0.25, p[1]*0.25), (1,0,0)))
        tris.append((base, base+1, base+2))
        mat_bucket.append((base, 3, len(tris)-1, 1))

floor_b, wall_b, rock_b = [], [], []

S = 60.0            # half-size -> 120x120 arena
quad(floor_b, (-S,0,-S), (-S,0,S), (S,0,S), (S,0,-S), (0,1,0), 0.125)   # floor
WH, WT = 8.0, 1.0
box(wall_b,  0, WH/2,  S+WT, S+2*WT, WH/2, WT)   # north
box(wall_b,  0, WH/2, -S-WT, S+2*WT, WH/2, WT)   # south
box(wall_b,  S+WT, WH/2, 0, WT, WH/2, S)          # east
box(wall_b, -S-WT, WH/2, 0, WT, WH/2, S)          # west
box(rock_b, 0, 1.0, 0, 8.0, 1.0, 8.0)             # central platform 16x16 h=2
for f,(cx,cz) in zip(('+z','-z','+x','-x'), ((0,11.5),(0,-11.5),(11.5,0),(-11.5,0))):
    wedge(rock_b, cx, cz, 3.0, 3.5, 2.0, f)       # ramps onto platform
import random
random.seed(SEED)
for i in range(NPILL):                                # landmark pillars
    a = random.uniform(0, 6.283); r = random.uniform(22, 48)
    px, pz = r*__import__('math').cos(a), r*__import__('math').sin(a)
    h = random.uniform(3.0, 7.0)
    box(rock_b, px, h/2, pz, random.uniform(1.2,2.4), h/2, random.uniform(1.2,2.4))

# ---- assemble meshes per material (contiguous vertex/tri ranges required) ----
# rebuild vertex/tri arrays grouped by material for contiguous mesh ranges.
# DOUBLE-SIDED: every triangle is emitted both windings so faces are visible
# from inside the arena too (you stand inside the walls) -- exengine culls
# back faces, so a single winding would leave interior faces invisible (void).
groups = [(FLOORT, floor_b), (WALLT, wall_b), (ROCKT, rock_b)]
new_verts, new_tris, mesh_recs = [], [], []
for mat, bucket in groups:
    fv, ft = len(new_verts), len(new_tris)
    for (v0, nv, t0, nt) in bucket:
        remap = {}
        for i in range(nv):
            remap[v0+i] = len(new_verts); new_verts.append(verts[v0+i])
        for t in range(nt):
            a,b,c = tris[t0+t]
            new_tris.append((remap[a], remap[b], remap[c]))       # front
            new_tris.append((remap[a], remap[c], remap[b]))       # back (reversed)
    mesh_recs.append((mat, fv, len(new_verts)-fv, ft, len(new_tris)-ft))
verts, tris = new_verts, new_tris

# ---- text table ----
text = b"\x00"  # index 0 = empty name
offs = {}
for mat,_,_,_,_ in mesh_recs:
    offs[mat] = len(text); text += mat.encode() + b"\x00"
name_off = len(text); text += b"arena\x00"

HDR = 27*4 + 16 + 4  # magic16 + version + 26 uints... compute via struct below
def u(*a): return struct.pack("<" + "I"*len(a), *a)

num_v, num_t, num_m = len(verts), len(tris), len(mesh_recs)
ofs = 16 + 4 + 4*26                       # header size: magic + version + 26 uints
ofs_text = ofs;                ofs += len(text)
ofs += (-ofs) % 4
ofs_meshes = ofs;              ofs += num_m * 6*4
ofs_va = ofs;                  ofs += 3 * 5*4
ofs_pos = ofs;                 ofs += num_v * 12
ofs_uv  = ofs;                 ofs += num_v * 8
ofs_nrm = ofs;                 ofs += num_v * 12
ofs_tri = ofs;                 ofs += num_t * 12
filesize = ofs

out  = b"INTERQUAKEMODEL\x00" + u(2, filesize, 0)
out += u(len(text), ofs_text)
out += u(num_m, ofs_meshes)
out += u(3, num_v, ofs_va)
out += u(num_t, ofs_tri, 0)          # no adjacency
out += u(0,0) + u(0,0) + u(0,0)      # joints, poses, anims
out += u(0,0,0,0)                     # frames, framechannels, ofs_frames, ofs_bounds
out += u(0,0) + u(0,0)                # comment, extensions
out += text + b"\x00" * ((-len(out+text)) % 4 if False else ((-(16+4+4*26+len(text))) % 4))
for mat, fv, nv, ft, nt in mesh_recs:
    out += u(name_off, offs[mat], fv, nv, ft, nt)
out += u(0,0,7,3,ofs_pos)  # POSITION float size3
out += u(1,0,7,2,ofs_uv)   # TEXCOORD float size2
out += u(2,0,7,3,ofs_nrm)  # NORMAL float size3
for p,_,_ in verts: out += struct.pack("<3f", *p)
for _,uv,_ in verts: out += struct.pack("<2f", *uv)
for _,_,n in verts: out += struct.pack("<3f", *n)
for t in tris: out += u(*t)

assert len(out) == filesize, (len(out), filesize)
dst = os.path.join(os.path.dirname(__file__), "..", "engine_src", "data", NAME + ".iqm")
open(dst, "wb").write(out)
print(f"wrote {dst}: {num_v} verts, {num_t} tris, {num_m} meshes, {filesize} bytes")
