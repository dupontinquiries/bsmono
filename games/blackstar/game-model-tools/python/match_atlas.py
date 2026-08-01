#!/usr/bin/env python3
"""match_atlas.py — four views, separable colours, and the motion field.

Three fixes over the single-view version:

  * FOUR ANGLES (front, side, top, back). One projection hides whichever axis it
    collapses, and a match that looks wrong from the side is often correct seen
    from above.
  * COLOURS THAT SEPARATE. Golden-angle hue rotation with alternating value and
    saturation, so adjacent indices never land on neighbouring shades.
  * THE MOTION FIELD. Each matched pair has a displacement from its tank position
    to its robot position. Where NEIGHBOURING parts share a displacement, they are
    moving as one body — which is a joint, derived from the correspondence rather
    than authored. Lines are drawn per pair and coherent groups get a shared hue,
    so the transform path shows itself.
"""
import sys, os, math, colorsys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from transform_collage import load_iqm, weld, components, bbox, centre
from match_view import load_parts, describe, dist

def sep_colour(n):
    """Golden-angle hue, alternating value/saturation — adjacent indices never
    look alike, which single-hue-rotation cannot promise."""
    h = (n * 0.61803398875) % 1.0
    v = [1.0, 0.72, 0.88, 0.60][n % 4]
    s = [0.85, 1.0, 0.62, 0.95][n % 4]
    r, g, b = colorsys.hsv_to_rgb(h, s, v)
    return (int(r*255), int(g*255), int(b*255))

VIEWS = [("front", 0, 1), ("side", 2, 1), ("top", 0, 2), ("back", 0, 1)]

def project(p, lo, span, ax, ay, W, H, flip):
    fx = (p[ax]-lo[ax])/span[ax]
    fy = (p[ay]-lo[ay])/span[ay]
    if flip: fx = 1.0 - fx
    return int(fx*(W-12))+6, int((1.0-fy)*(H-12))+6

def draw_model(v, comps, colours, W, H, ax, ay, flip):
    lo, hi = bbox(v); span = [max(hi[i]-lo[i],1e-9) for i in range(3)]
    img = [[(11,12,17)]*W for _ in range(H)]
    for ci in sorted(range(len(comps)), key=lambda c: -len(comps[c])):
        col = colours[ci]
        for i in comps[ci]:
            x, y = project(v[i], lo, span, ax, ay, W, H, flip)
            for dy in (-1,0,1):
                for dx in (-1,0,1):
                    xx, yy = x+dx, y+dy
                    if 0 <= xx < W and 0 <= yy < H: img[yy][xx] = col
    return img, lo, span

def motion_groups(pairs, ca, cb, va, vb, tol=0.28):
    """Cluster matched pairs whose parts are NEIGHBOURS and whose displacements
    agree. Each cluster is a rigid body moving together — a joint.

    This is the transform path arriving for free: the correspondence already
    encodes where every part went, and parts that went the same way while
    sitting next to each other were attached."""
    # Normalise each part's position to ITS OWN model's bounds before comparing.
    # The first version differenced raw coordinates between two models with
    # different origins and scales, so almost every part came out "barely moved"
    # (33 of 38 in one group at displacement 0.15) — it was measuring the gap
    # between two coordinate systems, not the motion of a part.
    la, ha = bbox(va); lb, hb = bbox(vb)
    sa = [max(ha[k]-la[k], 1e-9) for k in range(3)]
    sb = [max(hb[k]-lb[k], 1e-9) for k in range(3)]
    info = []
    for i, j, d in pairs:
        pa_raw = centre(*bbox([va[x] for x in ca[i]]))
        pb_raw = centre(*bbox([vb[x] for x in cb[j]]))
        pa = [(pa_raw[k]-la[k])/sa[k] for k in range(3)]   # 0..1 within the tank
        pb = [(pb_raw[k]-lb[k])/sb[k] for k in range(3)]   # 0..1 within the robot
        info.append({"i": i, "j": j, "from": pa_raw, "to": pb_raw,
                     "nfrom": pa, "nto": pb,
                     "disp": [pb[k]-pa[k] for k in range(3)]})
    if not info: return [], info
    scale = 1.0   # already normalised
    n = len(info)
    parent = list(range(n))
    def find(x):
        while parent[x] != x: parent[x] = parent[parent[x]]; x = parent[x]
        return x
    for a in range(n):
        for b in range(a+1, n):
            near = math.dist(info[a]["nfrom"], info[b]["nfrom"]) / scale
            same = math.dist(info[a]["disp"], info[b]["disp"]) / scale
            if near < 0.30 and same < tol:
                ra, rb = find(a), find(b)
                if ra != rb: parent[ra] = rb
    groups = {}
    for a in range(n): groups.setdefault(find(a), []).append(a)
    return sorted(groups.values(), key=len, reverse=True), info

if __name__ == '__main__':
    ap, bp, out = sys.argv[1], sys.argv[2], sys.argv[3]
    va, ca, ta = load_parts(ap)
    vb, cb, tb = load_parts(bp)
    from match_view import part_res
    la_, ha_ = bbox(va); lb_, hb_ = bbox(vb)
    span_a = max(ha_[k]-la_[k] for k in range(3)) or 1.0
    span_b = max(hb_[k]-lb_[k] for k in range(3)) or 1.0
    da = [describe([va[i] for i in c], tt, part_res([va[i] for i in c], span_a))
          for c, tt in zip(ca, ta)]
    db = [describe([vb[i] for i in c], tt, part_res([vb[i] for i in c], span_b))
          for c, tt in zip(cb, tb)]
    cand = sorted((dist(da[i], db[j]), i, j) for i in range(len(da)) for j in range(len(db)))
    pairs, used = [], set()
    for d, i, j in cand:
        if d > 0.55: break
        if any(p[0] == i for p in pairs) or j in used: continue
        pairs.append((i, j, d)); used.add(j)
    groups, info = motion_groups(pairs, ca, cb, va, vb)
    print(f"{len(pairs)} matched pairs -> {len(groups)} coherent motion groups")
    for gi, g in enumerate(groups[:6]):
        d = info[g[0]]["disp"]
        mag = math.sqrt(sum(x*x for x in d))
        print(f"  group {gi}: {len(g):2} parts moving together, displacement {mag:.2f}")

    # colour by MOTION GROUP, so the eye sees bodies rather than parts
    GREY = (66, 70, 80)
    colA = [GREY]*len(ca); colB = [GREY]*len(cb)
    member = {}
    for gi, g in enumerate(groups):
        c = sep_colour(gi)
        for k in g:
            colA[info[k]["i"]] = c; colB[info[k]["j"]] = c
            member[k] = gi

    W = H = 300
    try:
        from PIL import Image, ImageDraw
        sheet = Image.new('RGB', (W*4, H*2 + 26), (7,8,12))
        d = ImageDraw.Draw(sheet)
        for vi, (name, ax, ay) in enumerate(VIEWS):
            flip = (name == "back")
            ia, loa, spa = draw_model(va, ca, colA, W, H, ax, ay, flip)
            ib, lob, spb = draw_model(vb, cb, colB, W, H, ax, ay, flip)
            for row, tile in ((0, ia), (1, ib)):
                im = Image.new('RGB', (W,H)); im.putdata([px for r in tile for px in r])
                sheet.paste(im, (vi*W, row*H + 13))
            d.text((vi*W+6, 2), f"{name}  (tank above / robot below)", fill=(150,160,180))
        # motion lines: where a group's parts sit in the tank, and where they go
        for gi, g in enumerate(groups[:12]):
            c = sep_colour(gi)
            for k in g:
                x0, y0 = project(info[k]["from"], loa, spa, 0, 1, W, H, False)
                x1, y1 = project(info[k]["to"], lob, spb, 0, 1, W, H, False)
                d.line([(x0, y0+13), (x1, y1+13+H)], fill=c, width=1)
        d.text((6, H*2+15), f"{len(pairs)} pairs, {len(groups)} motion groups — lines join a part to where it goes",
               fill=(160,170,190))
        sheet.save(out); print(f"-> {out}")
    except ImportError:
        print("PIL missing")
