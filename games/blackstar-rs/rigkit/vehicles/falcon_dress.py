"""falcon_dress.py -- the Falcon as named groups, in the shape orion_dress
returns, so everything downstream of `groups()` works unchanged."""
import math, os, pickle, struct, json, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))   # the kit root: rigpaths
import falcon_catalogue as FC, falcon_panels as FP, panels

SRC = os.environ.get('FALCON_GLB', __import__('rigpaths').inp('falcon_oriented.glb'))


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
        pane = panels.cap_loop(V, T, FP.windscreen)

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


# ── how the body comes apart ─────────────────────────────────────────────────
#
# STREAM: the top shell and the back of the GT do not travel to a seat on the
# skeleton and scale down, which is what every reclaiming part did and what
# read as the car being slid and shrunk away. They STAY on the car, tremble,
# and come apart there into small cubes that stream into the core one at a
# time -- converging, overlapping, eating each other's volume on the way.
STREAM = {'roof', 'deck', 'quarter_l', 'quarter_r', 'trim',
          'fender_l', 'fender_r', 'glass_front'}

# And they go EARLIER than orion's shell, because they are what the robot has
# to come up through. The body starts rising at 0.42; by then the roof is
# about half gone, so the machine surfaces through a shell that is still
# trembling apart around it rather than punching a hole in an intact one.
DISSOLVE = {
    # windows now END spread from 0.60 to 0.92 -- they used to cluster at
    # 0.64-0.70, so several parts fell off the same cliff together
    'glass_front': (0.20, 0.40),
    'roof':        (0.26, 0.50),
    'deck':        (0.30, 0.50),
    'quarter_l':   (0.30, 0.56), 'quarter_r': (0.31, 0.56),
    'trim':        (0.32, 0.50),
    'fender_l':    (0.36, 0.54), 'fender_r':  (0.37, 0.54),
}

# Everything that is not armour compacts in place: the interiors used the old
# travel-and-scale path (the "sucking in"), and the frame parts rode to the
# body and stayed there, the floor pan among them at 325 long.
STREAM_ALL = True
DISSOLVE.update({
    'wing':      (0.32, 0.48),    # the front bumper, not orion's late splitter
    'floorpan':  (0.34, 0.58),
    'frame_l':   (0.36, 0.46), 'frame_r': (0.36, 0.46),
    'scoop':     (0.34, 0.48),
    'cockpit':   (0.18, 0.40), 'seat_l': (0.18, 0.38), 'seat_r': (0.19, 0.38),
    'radiator':  (0.24, 0.42), 'exhaust': (0.26, 0.44), 'undertray': (0.30, 0.48),
})

# MORE OF THE SHELL STAYS. It was being eaten away almost entirely -- only the
# bonnet halves, doors, lamps and wheels survived. The roof becomes a back
# plate, the boot lid sits on the lower back, the front fenders become forearm
# gauntlets and the rear quarters thigh plates.
# (An ARMOUR_EXTRA set here once promoted these panels to full-size armour.
# That brought back the big panes that took over the body; they now compact
# into cube armour instead -- see KEEP_CUBES below.)

# longer windows, so the per-cube stagger has room: ends now run 0.78-0.97
DISSOLVE.update({
    'glass_front': (0.20, 0.58),
    'trim':        (0.28, 0.66),
    'wing':        (0.30, 0.64),
    'floorpan':    (0.32, 0.64),
    'frame_l':     (0.34, 0.60), 'frame_r': (0.34, 0.60),
    'scoop':       (0.32, 0.60),
    'cockpit':     (0.18, 0.62), 'seat_l': (0.18, 0.60), 'seat_r': (0.19, 0.60),
    'radiator':    (0.24, 0.62), 'exhaust': (0.26, 0.64), 'undertray': (0.28, 0.64),
})




# CUBES OFF. The instanced cube particles read worse than orion's skinned
# shader dissolve, so the Falcon uses the same path: panels crack, their
# fragments ride the nearest bone and snap onto it, then decay.
CUBES = False

# ── shell parts that come through as ARMOUR, reshaped ────────────────────────
# Doors and bonnet halves arrive as themselves. These arrive as the robot's own
# trim: while each rides to its seat, the shader morphs it --
#   'spike'  tapers to a point along its long axis, stretches, thins: a blade
#   'trim'   narrows to a bar: an armour strip
# Same metal, reshaped, so it still reads as the car it came from.
ARMOUR_MORPH = {
    'fender_l': 'spike', 'fender_r': 'spike',      # forearm blades
    'quarter_l': 'trim', 'quarter_r': 'trim',      # thigh trim bars
}


# The fender blades ride the OUTSIDE OF THE UPPER ARMS: the forearms now carry
# the spinning front wheels, and a blade and a wheel on one forearm overlapped
# 70% of their boxes.
SEATS = {
    # the FRONT OF THE SHINS: on the upper arms they met the doors
    'fender_l': ('shin_l', (-14, 0.38, 0), (0, 0, 0), 0.72),
    'fender_r': ('shin_r', (-14, 0.38, 0), (0, 0, 0), 0.72),
}

# WEAPON HAND: the right wrist wheel becomes a saw over the retracted fist
WEAPONS = [{'part': 'wheel_fr', 'side': 'r', 'mode': 'saw', 'slide': 0.38}]
