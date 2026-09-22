"""falcon_dress.py -- the Falcon as named groups, in the shape orion_dress
returns, so everything downstream of `groups()` works unchanged."""
import math, os, pickle, struct, json, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))   # the kit root: rigpaths
import hover_catalogue as FC, hover_panels as FP, panels

SRC = os.environ.get('HOVER_GLB', __import__('rigpaths').inp('folly_oriented.glb'))


def _load():
    b = open(SRC, 'rb').read()
    ln = struct.unpack('<I', b[12:16])[0]
    J = json.loads(b[20:20 + ln]); bn = b[20 + ln + 8:]
    n = J['accessors'][0]['count']
    P = [struct.unpack_from('<3f', bn, k * 12) for k in range(n)]
    io = J['bufferViews'][1]['byteOffset']
    I = list(struct.unpack_from(f"<{J['accessors'][1]['count']}I", bn, io))
    return P, I


def groups():
    from isl import islands
    P, I = _load()
    L = islands(P, I)
    named = {}
    for g, ids in FC.CATALOGUE.items():
        for i in ids:
            named[i] = g
    out = {}

    def add(name, verts, tris):
        d = out.setdefault(name, {'v': [], 'f': []})
        base = len(d['v'])
        d['v'] += list(verts)
        d['f'] += [(a + base, b + base, c + base) for a, b, c in tris]

    pane = ([], [])
    # the shell, cut into panels along authored planes with exact clipping
    for i in FC.CATALOGUE['shell']:
        sh = L[i]; vs = sh['verts']; rm = {o: k for k, o in enumerate(vs)}
        V = [P[j] for j in vs]
        T = [(rm[a], rm[b], rm[c]) for a, b, c in sh['tris']]
        for name, (pv, pt) in panels.split(V, T, FP.PLANES, FP.classify).items():
            add(name, pv, pt)
        # the windscreen pane is added AFTER loose parts are placed: its
        # bounding box is a slab across the whole cabin, and as a target it
        # swallowed the dashboard, mirror and wipers behind it -- 504
        # triangles for a 30-triangle pane. A pane contains nothing.
        pane = ([], [])

    # named islands first
    for d in L:
        if d['id'] in FC.CATALOGUE['shell'] or d['id'] not in named:
            continue
        vs = d['verts']; rm = {o: k for k, o in enumerate(vs)}
        add(named[d['id']], [P[j] for j in vs],
            [(rm[a], rm[b], rm[c]) for a, b, c in d['tris']])

    # LOOSE ISLANDS JOIN A PART ONLY IF THEY SIT INSIDE IT.
    #
    # Nearest-centre attachment glued everything loose at the front onto the
    # lamps -- 5,446 triangles spanning half the car -- and brake and
    # suspension clips onto the front wheels, which came out 79 deep where a
    # tyre is 23. Wheels, doors and panels are the ARMOUR; they have to stay
    # the shape they are. A door handle is inside the door's bounds, so it
    # belongs to the door. A brake caliper beside a wheel is not, so it goes
    # to a region -- and every region is interior that reclaims into the core
    # anyway, so nothing visible is lost.
    boxes = {}
    for g, d in out.items():
        lo = [min(v[k] for v in d['v']) for k in range(3)]
        hi = [max(v[k] for v in d['v']) for k in range(3)]
        boxes[g] = (lo, hi)
    PAD = 4.0
    for d in L:
        if d['id'] in FC.CATALOGUE['shell'] or d['id'] in named:
            continue
        c = d['ctr']
        inside = [g for g, (lo, hi) in boxes.items()
                  if all(lo[k] - PAD <= c[k] <= hi[k] + PAD for k in range(3))]
        if inside:
            # the tightest box that contains it is the part it is bolted to
            g = min(inside, key=lambda g: math.prod(
                max(1.0, boxes[g][1][k] - boxes[g][0][k]) for k in range(3)))
        elif -110.0 < c[0] < 55.0 and c[1] > 25.0 and abs(c[2]) < 72.0:
            g = 'cockpit'
        elif c[0] >= 55.0:
            g = 'radiator'
        else:
            g = 'undertray'
        vs = d['verts']; rm = {o: k for k, o in enumerate(vs)}
        add(g, [P[j] for j in vs], [(rm[a], rm[b], rm[c2]) for a, b, c2 in d['tris']])

    if pane[1]:
        add('glass_front', pane[0], pane[1])

    for g, d in out.items():
        lo = [min(v[a] for v in d['v']) for a in range(3)]
        hi = [max(v[a] for v in d['v']) for a in range(3)]
        d['c'] = [(lo[a] + hi[a]) * 0.5 for a in range(3)]
        d['size'] = [hi[a] - lo[a] for a in range(3)]
    return out


# ── how the hover car comes apart ────────────────────────────────────────
STREAM_ALL = True
CUBES = False

# ── the floating mage ─────────────────────────────────────────────────────
# Most parts were decided by nearest known part (hover_direct.json, from
# partnet/decide.py). These are the deliberate choices a lookup cannot know:
SEATS = {
    # the hover pods ride the lower legs: the thrusters it floats on
    # low on the shin, toward the ankle: at 0.80 they met the shin spikes
    'pod_l': ('shin_l', (0, 0.95, -18), (0, 0, 0), 0.80),
    'pod_r': ('shin_r', (0, 0.95,  18), (0, 0, 0), 0.80),
    # the right cannon becomes a STAFF, straight through the right fist
    'rod_r': ('hand_r', (0, 0.50, 0), (0, 0, 0), 1.60),
    # the hover dome becomes a helm; the rear wing a mantle across the upper back
    # a HELM, not a hat: at 0.90 the dome was ~72 wide on a narrower head and
    # overlapped the mantle (40%) and the chest plate (19%)
    'dome': ('head', (0, 0.80, 0), (0, 0, 0), 0.60),
    'spoiler': ('chest', (26, 0.62, 0), (0, 0, 0), 0.80),   # at the shoulder blades, clear of the helm
    # overruling the lookup: right way up, the nose resembles orion's wide low
    # lamp and was sent to the head -- where the helm already sits. For this
    # robot it is the chest plate, as the flyer's nose is.
    'nose': ('chest', (-26, 0.50, 0), (0, 0, 0), 0.80),
}
ARMOUR_EXTRA = {'pod_l', 'pod_r', 'rod_r', 'dome', 'spoiler', 'nose'}
# it does not stand: it LEVITATES out of the car, and floats this high
LEVITATE = True
HOVER = 70.0

# The folded robot lies on the BODY's floor (y 10..20 along its length, right way up), not on
# the lowest part: the splitter reaches y~10, so the fold lift used that and
# the robot hung beneath the hovering body. (The shape-envelope check said so
# -- "no surface at this height" was the robot below the shell -- and I first
# dismissed it as a sampling hole.)
FOLD_FLOOR = 20.0


# ── a THICK shell ─────────────────────────────────────────────────────────
# Folly's body is a single skin: panels cut from it were paper, and the fold
# check read its hollow as "no surface". Every panel cut from the body becomes
# a thin solid: the skin offset INWARD (toward the car's centre line, decided
# per face so the inside is consistent) along smoothed normals, the inner face
# added reversed, and every open edge -- the panel cuts included -- closed by
# a rim.
SHELL_PANELS = {'nose', 'tail', 'deck', 'fin_l', 'fin_r', 'side_fl', 'side_fr', 'side_rl', 'side_rr'}
THICK = 3.0


def thicken(V, F, t, axis_y):
    import numpy as np
    V = np.asarray(V, float); F = np.asarray(F, int)
    # WELD coincident vertices first: the skin's internal seams are unwelded,
    # so every seam read as an open edge and grew a rim -- 4x the triangles
    key = np.round(V / 0.05).astype(np.int64)
    uniq, remap = np.unique(key, axis=0, return_inverse=True)
    remap = remap.reshape(-1)
    Vw = np.zeros((len(uniq), 3)); np.add.at(Vw, remap, V)
    Vw /= np.bincount(remap, minlength=len(uniq))[:, None]
    F = remap[F]; F = F[(F[:, 0] != F[:, 1]) & (F[:, 1] != F[:, 2]) & (F[:, 0] != F[:, 2])]
    V = Vw
    a, b, c = V[F[:, 0]], V[F[:, 1]], V[F[:, 2]]
    fn = np.cross(b - a, c - a)
    ctr = (a + b + c) / 3
    ref = np.stack([ctr[:, 0], np.full(len(F), axis_y), np.zeros(len(F))], 1)
    inward = np.einsum('ij,ij->i', fn, ref - ctr) > 0
    fn[~inward] *= -1                                   # every face normal points in
    vn = np.zeros_like(V)
    for k in range(3):
        np.add.at(vn, F[:, k], fn)
    vn /= np.maximum(np.linalg.norm(vn, axis=1, keepdims=True), 1e-9)
    n = len(V)
    V2 = np.concatenate([V, V + vn * t])
    F2 = [F, F[:, ::-1] + n]
    from collections import Counter
    und = Counter(tuple(sorted(e)) for f in F for e in ((f[0], f[1]), (f[1], f[2]), (f[2], f[0])))
    rim = [(e0, e1) for f in F for (e0, e1) in ((f[0], f[1]), (f[1], f[2]), (f[2], f[0]))
           if und[tuple(sorted((e0, e1)))] == 1]
    if rim:
        R = np.array(rim)
        F2.append(np.stack([R[:, 0], R[:, 1], R[:, 1] + n], 1))
        F2.append(np.stack([R[:, 0], R[:, 1] + n, R[:, 0] + n], 1))
    return V2.tolist(), np.concatenate(F2).tolist(), len(rim)


_groups_skin = groups


def groups():
    import numpy as np
    G = _groups_skin()
    allv = np.concatenate([np.asarray(G[g]['v']) for g in G])
    axis_y = float((allv[:, 1].min() + allv[:, 1].max()) / 2)
    for g in SHELL_PANELS & set(G):
        v, f, nrim = thicken(G[g]['v'], G[g]['f'], THICK, axis_y)
        V = np.asarray(v)
        G[g] = {**G[g], 'v': v, 'f': f, 'c': ((V.min(0) + V.max(0)) / 2).tolist(),
                'size': (V.max(0) - V.min(0)).tolist()}
    return G

# WEAPON HAND: the staff extends from the right hand and thrusts
WEAPONS = [{'part': 'rod_r', 'side': 'r', 'mode': 'staff', 'slide': 0.30}]
