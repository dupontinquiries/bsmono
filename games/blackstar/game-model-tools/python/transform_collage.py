#!/usr/bin/env python3
"""transform_collage.py — vehicle to bot, in five stages, as one image per model.

The corpus is full of flat, static vehicle meshes. The goal is not to replace
them (their authored detail is the thing worth keeping) but to DERIVE a bot mode
from each one and show the transformation, so a character reads as a real object
that folds rather than two unrelated models swapped at runtime.

The method, and why each step is what it is:

1.  WELD, then split into connected components. Half this corpus is unwelded, so
    without welding a mesh looks like a pile of loose triangles and no structure
    can be found in it at all.

2.  Keep the TOP-K regions by vertex count, and merge the rest into their nearest
    keeper. Every component matters visually but not structurally: a vehicle has
    perhaps eight meaningful masses and two hundred greebles, and a skeleton with
    two hundred joints is neither riggable nor animatable. Merging rather than
    discarding is what preserves the silhouette — the greebles still travel, they
    just travel with a parent.

3.  Assign each region a SKELETON SLOT from where it sits in the vehicle's own
    frame. Wheels become limbs because wheels are already at the corners; the
    long central mass becomes the torso because that is what it is. This is
    derived from the geometry rather than hand-authored, which is the only way it
    scales to 2,133 models.

4.  Pose each slot at vehicle and bot extremes, and interpolate rigidly. Every
    stage is the SAME geometry under a rigid transform — nothing is deformed,
    stretched or rebuilt, so the bot mode contains exactly the vehicle's detail.
    That is also what makes it verifiable by model-verify.

Run:
    python3 transform_collage.py MODEL.iqm [MODEL2.iqm ...] --out DIR
"""
import struct, sys, math, os, argparse, colorsys

# ---------------------------------------------------------------- IQM loading
def load_iqm(path):
    b = open(path, 'rb').read()
    if b[:15] != b'INTERQUAKEMODEL':
        return None, None
    g = lambda o: struct.unpack_from('<I', b, o)[0]
    nva, nv, ofs_va = g(44), g(48), g(52)
    nt, ofs_tri = g(56), g(60)
    pos = None
    for i in range(nva):
        e = ofs_va + i * 20
        ty, fmt, size, off = g(e), g(e + 8), g(e + 12), g(e + 16)
        if ty == 0 and fmt == 7:
            import array
            a = array.array('f'); a.frombytes(b[off:off + nv * size * 4])
            pos = [(a[k], a[k+1], a[k+2]) for k in range(0, len(a), size)]
    import array
    tri = array.array('I'); tri.frombytes(b[ofs_tri:ofs_tri + nt * 12])
    return pos, tri

# ------------------------------------------------------------------- welding
def weld(verts, eps):
    cell = max(eps, 1e-9) * 2.0
    buckets, remap = {}, list(range(len(verts)))
    for i, p in enumerate(verts):
        k = (int(p[0]//cell), int(p[1]//cell), int(p[2]//cell))
        hit = None
        for dx in (-1,0,1):
            for dy in (-1,0,1):
                for dz in (-1,0,1):
                    for j in buckets.get((k[0]+dx,k[1]+dy,k[2]+dz), ()):
                        q = verts[j]
                        if (p[0]-q[0])**2 + (p[1]-q[1])**2 + (p[2]-q[2])**2 <= eps*eps:
                            hit = remap[j]; break
                    if hit is not None: break
                if hit is not None: break
            if hit is not None: break
        remap[i] = i if hit is None else hit
        buckets.setdefault(k, []).append(i)
    return remap

def components(nv, tris, remap):
    par = list(range(nv))
    def find(x):
        while par[x] != x: par[x] = par[par[x]]; x = par[x]
        return x
    for i in range(0, len(tris), 3):
        a, b, c = remap[tris[i]], remap[tris[i+1]], remap[tris[i+2]]
        for u, v in ((a,b),(b,c)):
            ru, rv = find(u), find(v)
            if ru != rv: par[ru] = rv
    groups = {}
    for i in range(nv): groups.setdefault(find(remap[i]), []).append(i)
    return [g for g in groups.values() if len(g) >= 3]

# ------------------------------------------------------------------- regions
def bbox(pts):
    lo = [min(p[i] for p in pts) for i in range(3)]
    hi = [max(p[i] for p in pts) for i in range(3)]
    return lo, hi

def centre(lo, hi): return [(lo[i]+hi[i])*0.5 for i in range(3)]

def top_k_regions(verts, comps, k):
    """Keep the k largest components; merge every other into its nearest keeper.

    Merging rather than dropping is the point: the greebles carry most of a
    vehicle's character, and a silhouette that loses them stops looking like the
    thing it came from."""
    comps = sorted(comps, key=len, reverse=True)
    keepers = comps[:k]
    kc = [centre(*bbox([verts[i] for i in c])) for c in keepers]
    merged = [list(c) for c in keepers]
    for c in comps[k:]:
        cc = centre(*bbox([verts[i] for i in c]))
        best = min(range(len(kc)), key=lambda j: sum((cc[a]-kc[j][a])**2 for a in range(3)))
        merged[best].extend(c)
    out = []
    for m in merged:
        pts = [verts[i] for i in m]
        lo, hi = bbox(pts)
        out.append({"idx": m, "lo": lo, "hi": hi, "c": centre(lo, hi), "n": len(m)})
    return out

# ------------------------------------------------------------------ skeleton
SLOTS = ["torso", "head", "arm_l", "arm_r", "leg_l", "leg_r", "pack", "greeble"]

def assign_slots(regions, lo, hi):
    """Derive a skeleton slot per region from where it sits in the vehicle.

    A vehicle's own frame already encodes the body plan: the long central mass is
    a torso, the corner masses are limbs, the top-front is a head. Deriving it
    beats hand-authoring 2,133 times."""
    span = [max(hi[i]-lo[i], 1e-6) for i in range(3)]
    # longest axis is the vehicle's length; the shortest is usually its height
    length_axis = max(range(3), key=lambda i: span[i])
    up_axis = min(range(3), key=lambda i: span[i])
    side_axis = 3 - length_axis - up_axis
    regions = sorted(regions, key=lambda r: -r["n"])
    for rank, r in enumerate(regions):
        f = [(r["c"][i] - lo[i]) / span[i] for i in range(3)]   # 0..1 in each axis
        fwd, side, up = f[length_axis], f[side_axis], f[up_axis]
        if rank == 0:
            r["slot"] = "torso"
        elif up > 0.62 and fwd > 0.5:
            r["slot"] = "head"
        elif up > 0.6:
            r["slot"] = "pack"
        elif fwd > 0.55:
            r["slot"] = "arm_l" if side < 0.5 else "arm_r"
        elif fwd < 0.45:
            r["slot"] = "leg_l" if side < 0.5 else "leg_r"
        else:
            r["slot"] = "greeble"
        r["axes"] = (length_axis, side_axis, up_axis)
    return regions

# ---------------------------------------------------------------- transform
def bot_pose(r, lo, hi):
    """Where this region ends up in bot mode, as a rigid transform.

    Deliberately modest rotations: a transformation that flings parts across the
    frame stops reading as the same object folding, which is the whole illusion."""
    la, sa, ua = r["axes"]
    span = [max(hi[i]-lo[i], 1e-6) for i in range(3)]
    t = [0.0, 0.0, 0.0]
    ang = 0.0
    slot = r["slot"]
    if slot == "torso":
        t[ua] += span[ua] * 0.35            # stand up
        ang = 0.12
    elif slot == "head":
        t[ua] += span[ua] * 0.95
        t[la] -= span[la] * 0.08
    elif slot == "pack":
        t[ua] += span[ua] * 0.55
        t[la] -= span[la] * 0.18
    elif slot.startswith("arm"):
        t[ua] += span[ua] * 0.55
        t[sa] += span[sa] * (0.42 if slot.endswith("_r") else -0.42)
        t[la] -= span[la] * 0.05
        ang = -0.55
    elif slot.startswith("leg"):
        t[ua] -= span[ua] * 0.30
        t[sa] += span[sa] * (0.16 if slot.endswith("_r") else -0.16)
        t[la] -= span[la] * 0.22
        ang = 0.35
    else:
        t[ua] += span[ua] * 0.12
    return t, ang, ua

def stage_transform(r, lo, hi, s, n_stages):
    """Stage s of n: interpolate the rigid pose. Ease-in-out, because a linear
    blend reads as a slideshow rather than a mechanism."""
    u = s / max(n_stages - 1, 1)
    e = u * u * (3.0 - 2.0 * u)
    t, ang, ua = bot_pose(r, lo, hi)
    return [x * e for x in t], ang * e, ua

def apply(p, c, t, ang, ua):
    """Rotate about the region's own centre, then translate. Rigid: the geometry
    is never deformed, so every stage holds the model's full authored detail."""
    a, b = [i for i in range(3) if i != ua]
    x, y = p[a] - c[a], p[b] - c[b]
    ca, sa_ = math.cos(ang), math.sin(ang)
    q = list(p)
    q[a] = c[a] + x * ca - y * sa_
    q[b] = c[b] + x * sa_ + y * ca
    return [q[i] + t[i] for i in range(3)]

# ------------------------------------------------------------------ raster
def render(verts, tris, regions, lo, hi, stage, n_stages, W, H, axes):
    la, sa, ua = axes
    posed, colour = [None]*len(verts), [0]*len(verts)
    for ri, r in enumerate(regions):
        t, ang, u = stage_transform(r, lo, hi, stage, n_stages)
        for i in r["idx"]:
            posed[i] = apply(verts[i], r["c"], t, ang, u)
            colour[i] = ri
    pts = [p for p in posed if p]
    if not pts: return None
    plo, phi = bbox(pts)
    # a shared frame across stages would be ideal, but per-stage keeps the
    # subject filling the tile; the caller pads to make the sequence readable
    # SIDE PROFILE: length across, up vertical, depth along the side axis.
    # Projecting front-on (side x up) shows a truck as a narrow slab and hides
    # the whole transformation; the profile is the view that reads.
    def proj(p):
        sx = (p[la] - plo[la]) / max(phi[la]-plo[la], 1e-6)
        sy = (p[ua] - plo[ua]) / max(phi[ua]-plo[ua], 1e-6)
        return int(sx * (W-8)) + 4, int((1.0-sy) * (H-8)) + 4
    img = [[(14, 16, 22)] * W for _ in range(H)]
    zbuf = [[1e18] * W for _ in range(H)]
    pal = {}
    for ri in range(len(regions)):
        h = (ri * 0.153) % 1.0
        rr, gg, bb = colorsys.hsv_to_rgb(h, 0.45, 0.95)
        pal[ri] = (int(rr*255), int(gg*255), int(bb*255))
    for i in range(0, len(tris), 3):
        a, b, c = tris[i], tris[i+1], tris[i+2]
        if posed[a] is None or posed[b] is None or posed[c] is None: continue
        P = [proj(posed[a]), proj(posed[b]), proj(posed[c])]
        depth = (posed[a][sa] + posed[b][sa] + posed[c][sa]) / 3.0
        col = pal[colour[a]]
        # flat shade by facing, so the form reads instead of being a silhouette
        e1 = [posed[b][k]-posed[a][k] for k in range(3)]
        e2 = [posed[c][k]-posed[a][k] for k in range(3)]
        nz = e1[la]*e2[ua] - e1[ua]*e2[la]
        nl = math.sqrt(sum(x*x for x in (e1[0]*e2[1]-e1[1]*e2[0], nz, 1e-6))) or 1.0
        sh = 0.55 + 0.45 * min(1.0, abs(nz) / (nl + 1e-6))
        col = tuple(min(255, int(v * sh)) for v in col)
        xs = [p[0] for p in P]; ys = [p[1] for p in P]
        x0, x1 = max(0, min(xs)), min(W-1, max(xs))
        y0, y1 = max(0, min(ys)), min(H-1, max(ys))
        if x1 < x0 or y1 < y0: continue
        d = (P[1][1]-P[2][1])*(P[0][0]-P[2][0]) + (P[2][0]-P[1][0])*(P[0][1]-P[2][1])
        if abs(d) < 1e-9: continue
        for y in range(y0, y1+1):
            for x in range(x0, x1+1):
                w0 = ((P[1][1]-P[2][1])*(x-P[2][0]) + (P[2][0]-P[1][0])*(y-P[2][1])) / d
                w1 = ((P[2][1]-P[0][1])*(x-P[2][0]) + (P[0][0]-P[2][0])*(y-P[2][1])) / d
                w2 = 1 - w0 - w1
                if w0 < -0.002 or w1 < -0.002 or w2 < -0.002: continue
                if depth < zbuf[y][x]:
                    zbuf[y][x] = depth; img[y][x] = col
    return img

def write_png(path, tiles, W, H, labels):
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        return write_ppm(path.replace('.png', '.ppm'), tiles, W, H)
    n = len(tiles)
    out = Image.new('RGB', (W*n, H), (10, 11, 16))
    for i, t in enumerate(tiles):
        im = Image.new('RGB', (W, H))
        im.putdata([px for row in t for px in row])
        out.paste(im, (i*W, 0))
    d = ImageDraw.Draw(out)
    for i, lab in enumerate(labels):
        d.text((i*W + 6, H - 14), lab, fill=(150, 160, 180))
        if i: d.line([(i*W, 0), (i*W, H)], fill=(40, 44, 56))
    out.save(path)
    return path

def write_ppm(path, tiles, W, H):
    n = len(tiles)
    with open(path, 'wb') as f:
        f.write(f"P6\n{W*n} {H}\n255\n".encode())
        for y in range(H):
            row = bytearray()
            for t in tiles:
                for x in range(W): row += bytes(t[y][x])
            f.write(row)
    return path

def load_model(paths):
    """Assemble every part file of a model into one mesh.

    A "part" in this corpus is a fragment, not a component: Optimus is 13 files
    and grimorph is 90. Deriving a bot from a single fragment is meaningless —
    the body plan only exists once the model is whole."""
    verts, tris = [], []
    for p in sorted(paths):
        v, t = load_iqm(p)
        if not v: continue
        base = len(verts)
        verts.extend(v)
        tris.extend(x + base for x in t)
    return verts, tris

def process(paths, outdir, k=8, stages=5, W=260, H=260, name=None):
    if isinstance(paths, str): paths = [paths]
    verts, tris = load_model(paths)
    if not verts: return None
    lo, hi = bbox(verts)
    span = max(hi[i]-lo[i] for i in range(3)) or 1.0
    remap = weld(verts, span * 1e-5)
    comps = components(len(verts), tris, remap)
    if not comps: return None
    regions = top_k_regions(verts, comps, k)
    regions = assign_slots(regions, lo, hi)
    axes = regions[0]["axes"]
    tiles, labels = [], []
    for s in range(stages):
        img = render(verts, tris, regions, lo, hi, s, stages, W, H, axes)
        if img is None: return None
        tiles.append(img)
        labels.append(["vehicle", "unlock", "unfold", "rise", "bot"][s] if stages == 5 else f"s{s}")
    name = name or os.path.basename(paths[0]).replace('.iqm', '')
    os.makedirs(outdir, exist_ok=True)
    out = write_png(os.path.join(outdir, f"{name}_transform.png"), tiles, W, H, labels)
    slots = {}
    for r in regions: slots[r["slot"]] = slots.get(r["slot"], 0) + 1
    return out, len(comps), len(regions), slots

if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('models', nargs='+', help='IQM files; parts of one model are grouped automatically')
    ap.add_argument('--out', default='transform_sheets')
    ap.add_argument('-k', type=int, default=8, help='regions kept as skeleton parts')
    a = ap.parse_args()
    # group by model name: imported_<model>_partN.iqm are one model
    import re, collections
    groups = collections.defaultdict(list)
    for m in a.models:
        b = os.path.basename(m)
        mm = re.match(r'imported_(.+?)_part\d+\.iqm$', b)
        groups[mm.group(1) if mm else b[:-4]].append(m)
    for name, paths in sorted(groups.items()):
        r = process(paths, a.out, k=a.k, name=name)
        if r:
            out, nc, nr, slots = r
            print(f"{name:44} {len(paths):3} parts {nc:5} comps -> {nr} regions  {slots}")
            print(f"  -> {out}")
        else:
            print(f"{name:44} skipped (no geometry)")
