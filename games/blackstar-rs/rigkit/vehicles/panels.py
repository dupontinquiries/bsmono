"""panels.py -- cut a welded body shell into panels along authored planes.

The first attempt at this assigned each triangle WHOLE to whichever side of a
cut its centroid fell on. Triangles straddling the line went entirely to one
side, so every seam came out as a sawtooth -- and on a thin shell that read as
spines growing out of flat plates.

This clips instead. A triangle crossing a plane is split at the plane into
pieces that lie wholly on one side, with new vertices exactly on the cut. Only
THEN is each piece assigned to a panel, and since no piece crosses a cut any
more, every seam is a straight line.
"""


def clip(verts, tris, axis, value):
    """Split every triangle that crosses `axis = value`."""
    verts = list(verts)
    out = []
    cache = {}

    def cut(i, j):
        key = (min(i, j), max(i, j))
        if key in cache:
            return cache[key]
        a, b = verts[i], verts[j]
        t = (value - a[axis]) / (b[axis] - a[axis])
        p = tuple(a[k] + (b[k] - a[k]) * t for k in range(3))
        verts.append(p)
        cache[key] = len(verts) - 1
        return cache[key]

    for tri in tris:
        side = [verts[i][axis] - value for i in tri]
        # on the plane counts as whichever side the rest are on
        pos = [s > 1e-9 for s in side]
        neg = [s < -1e-9 for s in side]
        if not any(pos) or not any(neg):
            out.append(tri)
            continue
        # rotate so the lone vertex is first
        for r in range(3):
            a, b, c = tri[r], tri[(r + 1) % 3], tri[(r + 2) % 3]
            sa, sb, sc = side[r], side[(r + 1) % 3], side[(r + 2) % 3]
            if (sa > 0) != (sb > 0) and (sa > 0) != (sc > 0):
                break
        ab, ac = cut(a, b), cut(a, c)
        # the lone vertex keeps a triangle; the other two keep a quad
        out.append((a, ab, ac))
        out.append((ab, b, c))
        out.append((ab, c, ac))
    return verts, out


def cut_all(verts, tris, planes):
    for axis, value in planes:
        verts, tris = clip(verts, tris, axis, value)
    return verts, tris


def split(verts, tris, planes, classify):
    """Clip along every plane, then give each piece to a panel by centroid.

    Returns {panel: (verts, tris)} with each panel's vertices re-indexed, so
    a panel is a standalone mesh that can be moved on its own."""
    verts, tris = cut_all(verts, tris, planes)
    bucket = {}
    for t in tris:
        c = tuple(sum(verts[i][k] for i in t) / 3.0 for k in range(3))
        bucket.setdefault(classify(c), []).append(t)
    out = {}
    for name, ts in bucket.items():
        used = sorted({i for t in ts for i in t})
        remap = {o: n for n, o in enumerate(used)}
        out[name] = ([verts[i] for i in used],
                     [(remap[a], remap[b], remap[c]) for a, b, c in ts])
    return out


def cap_loop(verts, tris, select):
    """Fill a hole in a shell with a pane: a fan from the loop's centroid.

    For openings a model was built without -- the Falcon has no windscreen,
    just a 160-wide gap at the base of the roof. `select(centre, size)` picks
    which boundary loops to fill, so the choice stays authored per vehicle
    rather than filling every opening, most of which (wheel wells, the floor)
    are meant to be open.
    """
    from collections import defaultdict
    q = 1e-3
    key = {}
    rep = []
    for v in verts:
        k = (round(v[0] / q), round(v[1] / q), round(v[2] / q))
        rep.append(key.setdefault(k, len(key)))
    # directed edges: a boundary edge appears once, and keeping its direction
    # lets the pane be wound to match the shell it closes
    und = defaultdict(int)
    first = {}
    for a, b, c in tris:
        for x, y in ((a, b), (b, c), (c, a)):
            rx, ry = rep[x], rep[y]
            und[(min(rx, ry), max(rx, ry))] += 1
            first[(rx, ry)] = (x, y)
    nxt = {}
    for (rx, ry), (x, y) in first.items():
        if und[(min(rx, ry), max(rx, ry))] == 1:
            nxt[ry] = (rx, x, y)      # walk the loop backwards along the shell
    panes = []
    seen = set()
    for start in list(nxt):
        if start in seen:
            continue
        loop = []
        cur = start
        while cur in nxt and cur not in seen:
            seen.add(cur)
            rx, x, y = nxt[cur]
            loop.append(y)
            cur = rx
        if len(loop) < 3:
            continue
        pts = [verts[i] for i in loop]
        lo = [min(p[k] for p in pts) for k in range(3)]
        hi = [max(p[k] for p in pts) for k in range(3)]
        c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
        if select(c, [hi[k] - lo[k] for k in range(3)]):
            panes.append(loop)
    pv, pt = [], []
    for loop in panes:
        base = len(pv)
        pts = [verts[i] for i in loop]
        c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
        pv += pts + [c]
        n = len(loop)
        for i in range(n):
            pt.append((base + i, base + (i + 1) % n, base + n))
    return pv, pt
