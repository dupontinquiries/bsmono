#!/usr/bin/env python3
"""prop_view.py -- render a prop IQM with DIRECTIONAL LIGHTING (vertex normals),
so surface detail/facets are visible. The silhouette renderer shades by depth
(good for profile, blind to surface relief); this one shades by n.l so a faceted
rock reads as faceted, not a smooth blob.

  python3 tools/prop_view.py engine_src/data/rock.iqm --out /tmp/rock.png
"""
import struct, sys, os, zlib, math, argparse

def load_iqm(path):
    """Return (verts, norms, tris). Matches gen_props/silhouette IQM layout."""
    d = open(path, "rb").read()
    assert d[:16] == b"INTERQUAKEMODEL\x00", path
    n_va, n_vert, ofs_va = struct.unpack_from("<3I", d, 44)
    n_tri, ofs_tri, ofs_adj = struct.unpack_from("<3I", d, 56)
    pos_ofs = nrm_ofs = None
    for a in range(n_va):
        typ, flg, fmt, size, off = struct.unpack_from("<5I", d, ofs_va + a*20)
        if typ == 0: pos_ofs = off
        elif typ == 4: nrm_ofs = off
    verts = []; norms = []
    for i in range(n_vert):
        verts.append(struct.unpack_from("<3f", d, pos_ofs + i*12))
        norms.append(struct.unpack_from("<3f", d, nrm_ofs + i*12) if nrm_ofs is not None else (0, 1, 0))
    tris = [struct.unpack_from("<3I", d, ofs_tri + i*12) for i in range(n_tri)]
    return verts, norms, tris

def render(verts, norms, tris, W=340, H=340, yaw=0.7, pitch=0.5):
    # rotate for a 3/4 view
    cy, sy = math.cos(yaw), math.sin(yaw)
    cp, sp = math.cos(pitch), math.sin(pitch)
    def xf(v):
        x, y, z = v
        x, z = x*cy - z*sy, x*sy + z*cy      # yaw
        y, z = y*cp - z*sp, y*sp + z*cp      # pitch
        return (x, y, z)
    tv = [xf(v) for v in verts]
    tn = [xf(n) for n in norms]
    xs = [v[0] for v in tv]; ys = [v[1] for v in tv]
    minx, maxx = min(xs), max(xs); miny, maxy = min(ys), max(ys)
    pad = 0.12*max(maxx-minx, maxy-miny)
    minx -= pad; maxx += pad; miny -= pad; maxy += pad
    span = max(maxx-minx, maxy-miny); sc = (min(W, H)-1)/span
    img = bytearray([18, 20, 26]*(W*H)); zb = [1e9]*(W*H)
    L = (0.5, 0.6, 0.62)  # light dir (normalized-ish)
    Ln = math.sqrt(sum(c*c for c in L)); L = tuple(c/Ln for c in L)
    def proj(v):
        px = (v[0]-minx)*sc; py = (H-1)-(v[1]-miny)*sc; return px, py, v[2]
    for (ia, ib, ic) in tris:
        pa, pb, pc = proj(tv[ia]), proj(tv[ib]), proj(tv[ic])
        na, nb, nc = tn[ia], tn[ib], tn[ic]
        minX = max(0, int(min(pa[0], pb[0], pc[0]))); maxX = min(W-1, int(max(pa[0], pb[0], pc[0]))+1)
        minY = max(0, int(min(pa[1], pb[1], pc[1]))); maxY = min(H-1, int(max(pa[1], pb[1], pc[1]))+1)
        if maxX < minX or maxY < minY: continue
        x0, y0 = pa[0], pa[1]; x1, y1 = pb[0], pb[1]; x2, y2 = pc[0], pc[1]
        den = (y1-y2)*(x0-x2)+(x2-x1)*(y0-y2)
        if abs(den) < 1e-9: continue
        for py in range(minY, maxY+1):
            for px in range(minX, maxX+1):
                w0 = ((y1-y2)*(px-x2)+(x2-x1)*(py-y2))/den
                w1 = ((y2-y0)*(px-x2)+(x0-x2)*(py-y2))/den
                w2 = 1-w0-w1
                if w0 < 0 or w1 < 0 or w2 < 0: continue
                z = w0*pa[2]+w1*pb[2]+w2*pc[2]; idx = py*W+px
                if z < zb[idx]:
                    zb[idx] = z
                    nx = w0*na[0]+w1*nb[0]+w2*nc[0]
                    ny = w0*na[1]+w1*nb[1]+w2*nc[1]
                    nz = w0*na[2]+w1*nb[2]+w2*nc[2]
                    nl = math.sqrt(nx*nx+ny*ny+nz*nz) or 1.0
                    diff = max(0.0, (nx*L[0]+ny*L[1]+nz*L[2])/nl)
                    sh = 0.20 + 0.80*diff            # ambient + diffuse
                    r = int(120*sh+30); g = int(125*sh+28); b = int(140*sh+34)
                    o = idx*3; img[o] = min(255, r); img[o+1] = min(255, g); img[o+2] = min(255, b)
    return img, W, H

def write_png(path, img, W, H):
    def chunk(typ, data):
        c = typ+data; return struct.pack(">I", len(data))+c+struct.pack(">I", zlib.crc32(c) & 0xffffffff)
    raw = bytearray()
    for y in range(H):
        raw.append(0); raw += img[y*W*3:(y+1)*W*3]
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9)); png += chunk(b"IEND", b"")
    open(path, "wb").write(png)

def surface_roughness(verts, norms, tris):
    """A number for 'how faceted': mean angle between adjacent face normals.
    Higher = more surface detail/relief; a smooth sphere -> low."""
    import math
    fn = []
    for (a, b, c) in tris:
        va, vb, vc = verts[a], verts[b], verts[c]
        e1 = (vb[0]-va[0], vb[1]-va[1], vb[2]-va[2]); e2 = (vc[0]-va[0], vc[1]-va[1], vc[2]-va[2])
        n = (e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0])
        m = math.sqrt(sum(k*k for k in n)) or 1.0
        fn.append(tuple(k/m for k in n))
    # variance of face-normal directions = overall bumpiness
    mx = sum(n[0] for n in fn)/len(fn); my = sum(n[1] for n in fn)/len(fn); mz = sum(n[2] for n in fn)/len(fn)
    var = sum((n[0]-mx)**2+(n[1]-my)**2+(n[2]-mz)**2 for n in fn)/len(fn)
    return var

def local_faceting(verts, tris):
    """Mean dihedral angle (deg) between triangles sharing an edge -- fine relief,
    not overall shape. A smooth sphere ~ a few deg; a jagged rock much higher."""
    import math
    fn = {}
    edge = {}
    for ti, (a, b, c) in enumerate(tris):
        va, vb, vc = verts[a], verts[b], verts[c]
        e1 = (vb[0]-va[0], vb[1]-va[1], vb[2]-va[2]); e2 = (vc[0]-va[0], vc[1]-va[1], vc[2]-va[2])
        n = (e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0])
        m = math.sqrt(sum(k*k for k in n)) or 1.0
        fn[ti] = tuple(k/m for k in n)
        for (u, w) in ((a, b), (b, c), (c, a)):
            key = (min(u, w), max(u, w))
            edge.setdefault(key, []).append(ti)
    angs = []
    for key, tl in edge.items():
        if len(tl) == 2:
            d = max(-1.0, min(1.0, sum(fn[tl[0]][k]*fn[tl[1]][k] for k in range(3))))
            angs.append(math.degrees(math.acos(d)))
    return sum(angs)/len(angs) if angs else 0.0

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("iqm")
    ap.add_argument("--out", default="/tmp/prop.png")
    ap.add_argument("--yaw", type=float, default=0.7)
    ap.add_argument("--pitch", type=float, default=0.4)
    a = ap.parse_args()
    v, n, t = load_iqm(a.iqm)
    rough = surface_roughness(v, n, t)
    facet = local_faceting(v, t)
    img, W, H = render(v, n, t, yaw=a.yaw, pitch=a.pitch)
    write_png(a.out, img, W, H)
    print("wrote %s  (%d verts, %d tris, roughness=%.4f, faceting=%.2f deg)" % (a.out, len(v), len(t), rough, facet))
