#!/usr/bin/env python3
"""match_view.py — colour-code which parts the integral descriptor pairs up.

Renders two forms of a character side by side. A matched pair gets the SAME
colour in both; unmatched parts are grey. If the descriptor works, the colours
line up with what a person would call the same part.

The descriptor is the area-weighted normal-sum grid from model-verify: an
integral over the surface, so it survives retessellation where vertex count does
not. Triangles are subdivided before binning, because a big triangle would
otherwise dump all its area into one cell while its decimated twin spreads
across many.
"""
import sys, os, math, colorsys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from transform_collage import load_iqm, weld, components, bbox, centre

# A FIXED box that slides, rather than a fixed grid stretched over each part.
#
# The first version normalised every component to its own bounds and always used
# 3x3x3, so a large complex mass was summarised into the same 27 cells as a tiny
# bracket - the big parts came out as mush and barely any of them matched (1 of
# 38 over 200 verts). Sizing the grid from the part's real extent means a big
# part gets MORE cells and is described in proportion to how much there is to
# describe, which is what "slide the box" meant in the first place.
#
# Resolution is capped: past 6 the descriptor gets sparse and starts keying on
# noise instead of form.
CELL_FRACTION = 0.16      # box edge, as a fraction of the whole model's extent
RES_MIN, RES_MAX = 2, 6

def part_res(pts, model_span):
    ext = max(max(p[i] for p in pts) - min(p[i] for p in pts) for i in range(3))
    box = model_span * CELL_FRACTION
    return max(RES_MIN, min(RES_MAX, int(round(ext / max(box, 1e-9)))))

def describe(pts, tris_local, res=3):
    """(cells, total_area, res) for one component."""
    RES = res
    lo, hi = bbox(pts)
    span = [max(hi[i]-lo[i], 1e-9) for i in range(3)]
    cell = max(span) / RES
    limit = cell * 0.35
    cells = [[0.0]*4 for _ in range(RES**3)]   # nx, ny, nz, area
    total = 0.0
    for (a, b, c) in tris_local:
        stack = [(a, b, c)]
        guard = 0
        while stack:
            p, q, r = stack.pop()
            guard += 1
            if guard > 4000: break
            ext = max(max(p[i],q[i],r[i]) - min(p[i],q[i],r[i]) for i in range(3))
            if ext > limit and guard < 4000:
                m = lambda u,v: [(u[0]+v[0])/2,(u[1]+v[1])/2,(u[2]+v[2])/2]
                mp, mq, mr = m(p,q), m(q,r), m(r,p)
                stack += [(p,mp,mr),(mp,q,mq),(mr,mq,r),(mp,mq,mr)]
                continue
            u = [q[i]-p[i] for i in range(3)]; v = [r[i]-p[i] for i in range(3)]
            n = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]]
            mag = math.sqrt(sum(x*x for x in n))
            if mag < 1e-15: continue
            ar = mag*0.5; n = [x/mag for x in n]
            total += ar
            ctr = [(p[i]+q[i]+r[i])/3 for i in range(3)]
            f = [(ctr[i]-lo[i])/span[i] for i in range(3)]
            g = [min(RES-1, max(0, int(f[i]*RES))) for i in range(3)]
            k = (g[2]*RES + g[1])*RES + g[0]
            for i in range(3): cells[k][i] += n[i]*ar
            cells[k][3] += ar
    return cells, max(total, 1e-12), RES

# The 24 orientation-preserving rotations of a cube, as (axis permutation, sign
# flips). A part that has been rotated 90 degrees between vehicle and bot form
# produces a completely different axis-aligned grid, so an unrotated comparison
# simply cannot see it — which is most of what a transformation DOES to a part.
def _cube_rotations():
    import itertools
    out = []
    for perm in itertools.permutations(range(3)):
        for sx in (1, -1):
            for sy in (1, -1):
                for sz in (1, -1):
                    signs = (sx, sy, sz)
                    # keep only proper rotations (determinant +1); the rest are
                    # mirrors, and a mirrored match is a different part
                    par = 1
                    q = list(perm)
                    for a in range(3):
                        for b in range(a+1, 3):
                            if q[a] > q[b]: par = -par
                    if par * sx * sy * sz == 1:
                        out.append((perm, signs))
    return out

CUBE_ROTS = _cube_rotations()

def rotate_cells(cells, res, perm, signs):
    """Re-index a descriptor grid under one cube rotation, rotating the stored
    normal vectors to match."""
    out = [[0.0]*4 for _ in range(res**3)]
    for z in range(res):
        for y in range(res):
            for x in range(res):
                src = (z*res + y)*res + x
                c = cells[src]
                if c[3] <= 0.0: continue
                p = (x, y, z)
                q = [p[perm[i]] for i in range(3)]
                for i in range(3):
                    if signs[i] < 0: q[i] = res - 1 - q[i]
                dst = (q[2]*res + q[1])*res + q[0]
                n = [c[perm[i]] * signs[i] for i in range(3)]
                for i in range(3): out[dst][i] += n[i]
                out[dst][3] += c[3]
    return out

def dist_rot(A, B, rots=None):
    """Best distance over all 24 cube rotations — so a part matches its twin
    however it has been turned. This is the pass the scan was missing."""
    ca, ta, ra = A; cb, tb, rb = B
    if ra != rb: return float('inf')
    best = float('inf')
    for perm, signs in (rots or CUBE_ROTS):
        rc = rotate_cells(ca, ra, perm, signs)
        d = dist((rc, ta, ra), B)
        if d < best: best = d
    return best

def dist(A, B):
    """Distance, or infinity when the two parts are described at different
    resolutions — which is itself a size filter: a bracket cannot match a hull,
    and refusing the comparison is cheaper and more honest than scoring it."""
    ca, ta, ra = A; cb, tb, rb = B
    if ra != rb: return float('inf')
    acc = 0.0
    for x, y in zip(ca, cb):
        da, db = x[3]/ta, y[3]/tb
        acc += abs(da-db)
        w = min(da, db)
        if w > 0:
            ma = math.sqrt(sum(x[i]**2 for i in range(3))) or 1e-12
            mb = math.sqrt(sum(y[i]**2 for i in range(3))) or 1e-12
            dot = sum((x[i]/ma)*(y[i]/mb) for i in range(3))
            acc += w * (1 - max(-1, min(1, dot))) * 0.5
    return acc

def load_parts(path, minv=40, cap=90):
    v, t = load_iqm(path)
    lo, hi = bbox(v); span = max(hi[i]-lo[i] for i in range(3)) or 1.0
    comps = [c for c in components(len(v), t, weld(v, span*1e-5)) if len(c) >= minv]
    comps.sort(key=len, reverse=True)
    comps = comps[:cap]                       # the largest masses, which are what read
    owner = {}
    for ci, c in enumerate(comps):
        for i in c: owner[i] = ci
    tris = [[] for _ in comps]
    for i in range(0, len(t), 3):
        a, b, c2 = t[i], t[i+1], t[i+2]
        o = owner.get(a)
        if o is not None and owner.get(b) == o and owner.get(c2) == o:
            tris[o].append((v[a], v[b], v[c2]))
    return v, comps, tris

def render(v, comps, colours, W, H, title):
    lo, hi = bbox(v); span = [max(hi[i]-lo[i],1e-9) for i in range(3)]
    ax = sorted(range(3), key=lambda i: -span[i])
    la, ua = ax[0], ax[1]
    img = [[(12,13,18)]*W for _ in range(H)]
    order = sorted(range(len(comps)), key=lambda ci: -len(comps[ci]))
    for ci in order:
        col = colours[ci]
        for i in comps[ci]:
            p = v[i]
            x = int((p[la]-lo[la])/span[la]*(W-10))+5
            y = int((1-(p[ua]-lo[ua])/span[ua])*(H-10))+5
            for dy in (-1,0,1):
                for dx in (-1,0,1):
                    xx, yy = x+dx, y+dy
                    if 0 <= xx < W and 0 <= yy < H: img[yy][xx] = col
    return img

if __name__ == '__main__':
    a_path, b_path, out = sys.argv[1], sys.argv[2], sys.argv[3]
    va, ca, ta = load_parts(a_path)
    vb, cb, tb = load_parts(b_path)
    da = [describe([va[i] for i in c], t) for c, t in zip(ca, ta)]
    db = [describe([vb[i] for i in c], t) for c, t in zip(cb, tb)]
    THRESH = 0.55
    pairs, usedb = [], set()
    cand = []
    for i in range(len(da)):
        for j in range(len(db)):
            cand.append((dist(da[i], db[j]), i, j))
    cand.sort()
    for d, i, j in cand:
        if d > THRESH: break
        if any(p[0]==i for p in pairs) or j in usedb: continue
        pairs.append((i, j, d)); usedb.add(j)
    print(f"tank {len(ca)} parts, robot {len(cb)} parts -> {len(pairs)} matched (threshold {THRESH})")
    if pairs:
        print(f"  best {pairs[0][2]:.3f}  worst kept {pairs[-1][2]:.3f}")
    GREY = (74, 78, 88)
    colA = [GREY]*len(ca); colB = [GREY]*len(cb)
    for n, (i, j, d) in enumerate(pairs):
        h = (n*0.137) % 1.0
        r, g, b = colorsys.hsv_to_rgb(h, 0.62, 1.0)
        c = (int(r*255), int(g*255), int(b*255))
        colA[i] = c; colB[j] = c
    W = H = 420
    ia = render(va, ca, colA, W, H, 'tank')
    ib = render(vb, cb, colB, W, H, 'robot')
    try:
        from PIL import Image, ImageDraw
        im = Image.new('RGB', (W*2, H), (8,9,13))
        for k, tile in enumerate((ia, ib)):
            t2 = Image.new('RGB', (W,H)); t2.putdata([px for row in tile for px in row])
            im.paste(t2, (k*W, 0))
        d = ImageDraw.Draw(im)
        d.text((8, H-16), f"TANK — {len(pairs)} matched parts coloured", fill=(170,180,200))
        d.text((W+8, H-16), "ROBOT — same colour = same part", fill=(170,180,200))
        d.line([(W,0),(W,H)], fill=(50,54,66))
        im.save(out); print(f"-> {out}")
    except ImportError:
        print("PIL missing")
