"""falcon_dress.py -- the Falcon as named groups, in the shape orion_dress
returns, so everything downstream of `groups()` works unchanged."""
import math, os, pickle, struct, json, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))   # the kit root: rigpaths
import flyer_catalogue as FC, flyer_panels as FP, panels

SRC = os.environ.get('FLYER_GLB', __import__('rigpaths').inp('flyer_oriented.glb'))


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


# ── how the jet comes apart ──────────────────────────────────────────────
STREAM_ALL = True
CUBES = False
DISSOLVE = {
    'glass_front': (0.20, 0.40), 'roof': (0.26, 0.50), 'deck': (0.30, 0.50),
    'missile_l': (0.28, 0.44), 'missile_r': (0.29, 0.44),
    'trim': (0.30, 0.46), 'cockpit': (0.18, 0.40),
}
# shell parts that come through reshaped: fins as spikes, nacelles as trim
ARMOUR_MORPH = {'fender_l': 'spike', 'fender_r': 'spike',
                'quarter_l': 'trim', 'quarter_r': 'trim'}
# jet parts with no car equivalent, kept as armour
ARMOUR_EXTRA = {'engine_l', 'engine_r', 'gun_l', 'gun_r', 'nose'}
SEATS = {
    # DASH THRUSTERS on the lower back. Jumping belongs to the foot thrusters
    # the rig already has in the shins; the jet's main engines push the
    # machine forward, so they go where a forward dash comes from.
    # on the CHEST, low, on the side the wings prove is the back (+x). On the
    # spine they landed on the BELLY with either sign -- the "thrusters on
    # the chest" seen in the render -- so they use the bone whose convention
    # is measured rather than guessed.
    # pushed back and out, clear of the folded wings hanging over the lower
    # back (they overlapped 33% each)
    'engine_l': ('pelvis', (26, 0.50, -16), (0, 0, 0), 0.80, (16.0, 0.0)),
    'engine_r': ('pelvis', (26, 0.50,  16), (0, 0, 0), 0.80, (16.0, 0.0)),
    # the whole nose cone as one chest plate, down the front like a keel
    'nose': ('chest', (-26, 0.50, 0), (0, 0, 0), 0.80),
    # cannons along the forearms
    'gun_l': ('forearm_l', (0, 0.55, -14), (0, 0, 0), 0.85),
    'gun_r': ('forearm_r', (0, 0.55,  14), (0, 0, 0), 0.85),
    # tail fins as spikes on the outside of the upper arms
    # the FRONT OF THE SHINS: on the upper arms the full-size folded wings
    # swept straight through them (49%); the shins are free now the engines
    # moved to the back
    'fender_l': ('shin_l', (-14, 0.40, 0), (0, 0, 0), 0.70),
    'fender_r': ('shin_r', (-14, 0.40, 0), (0, 0, 0), 0.70),
}


# WINGS FOLD, they do not detach. Each stays hinged at its root: the hinge rides
# the chest and migrates onto the upper back while the wing swings about its
# root line.   part: (bone, hinge seat on that bone (x, along, z), fold degrees)
#   ... and an optional 4th: the part's size at the end. A folding wing is NOT
#   fitted to its bone like a detached plate -- that shrank these to 32%, about
#   85 long on a robot ~390 tall, and hid them behind the engines.
HINGES = {
    # hinged high on the back (0.95): at 0.78 their lower edge met the engines
    'door_l': ('chest', (22, 0.95, -14), 35.0, 0.62),
    'door_r': ('chest', (22, 0.95,  14), -35.0, 0.62),
}

# A TIGHTER FOLD for a jet: the arms roll in and tuck back along the body.
# Measured against the jet's real shape (surface-sampled, not vertices):
# 19.4% of the folded body stuck out -- all of it arms, past a fuselage ~45
# wide at the front -- and this fold leaves 5.0%. Torso and legs fit as is.
# The shoulders were the last of it (upper arms 35-42% out where the nose
# narrows): collarbones pulled in, and the arms squeezed to 65% thickness while
# folded -- they return to full size as the robot rises. 5.0% -> 2.4%.
FOLD = {'uparm_l': (40, -170, 0), 'uparm_r': (-40, 170, 0),
        'forearm_l': (0, -130, 0), 'forearm_r': (0, 130, 0),
        'clav_l': (30, -20, 0), 'clav_r': (-30, 20, 0),
        '_bulk': {n: (0.65, 1.0, 0.65) for n in ('clav_l', 'clav_r', 'uparm_l', 'uparm_r',
                                                'forearm_l', 'forearm_r', 'hand_l', 'hand_r')}}

# IN THE AIR a jet barrel-rolls to transform: it bleeds speed and unfolds at
# once. The ground push-up stays for landings. The page toggles between them.
AIR = True

# END FOR END: the seating aligns a part's long axis with its bone but not
# which end is up, and the engines arrived nozzles UP. Turned 180 about their
# sideways axis, ramping in as they attach, so exhaust points down and back.
FLIP = {'engine_l', 'engine_r'}

# WEAPON HAND: the right hand retracts and the forearm cannons slide forward
# past it -- a cannon, with recoil
WEAPONS = [{'part': 'gun_r', 'side': 'r', 'mode': 'cannon', 'slide': 0.55}]
