#!/usr/bin/env python3
"""build_viewer.py — generate rig-viewer.html from the Python sources.

Written after a patch-on-a-patch deleted the FK solver and the whole scene
setup: an edit replaced everything between two markers and the renderer,
camera, lights and `solve()` all happened to live in between. Six rounds of
string surgery on a generated file is how that happens.

The page is now BUILT, not edited. The rig, the body meshes and the analysis
constants all come straight from the modules that produce them, so the viewer
cannot drift from what the renders and the tests are using.
"""
import json, sys
import rigpaths  # the repo's tools/forge, and this kit, on the path
import orion_rig as R, orion_body as BD, orion_balance as BAL, orion_volume as VOL


def _align(size, bone_dir, out_dir):
    """Rotation that lays a panel ON a limb instead of near it.

    The panels were oriented by hand-authored eulers that knew nothing about
    the panel's own shape, so a flat door met a round arm at whatever angle
    the number happened to be -- which is the orthogonal protrusion. A panel
    is a slab: its THINNEST axis is its normal and its LONGEST runs along
    whatever it clads. Align those two and the fit is derived, not guessed.

    Returns a 3x3 mapping the panel's local axes onto (along, out, third).
    """
    import math as _m
    order = sorted(range(3), key=lambda i: size[i])
    thin, mid, long_ = order[0], order[1], order[2]

    def norm(v):
        L = _m.sqrt(sum(x*x for x in v)) or 1.0
        return [x / L for x in v]

    along = norm(list(bone_dir))
    # Gram-Schmidt `out` against the bone, with a fallback that is GUARANTEED
    # perpendicular. The first version fell back to a fixed [0,0,1], which is
    # parallel to a bone pointing along z -- the cross product then collapsed
    # to zero and twelve panels got a degenerate frame that scaled them to
    # nothing on one axis.
    def orth(v):
        d = sum(v[i] * along[i] for i in range(3))
        w = [v[i] - along[i] * d for i in range(3)]
        return w if sum(x*x for x in w) > 1e-8 else None
    out = orth(list(out_dir))
    if out is None:
        # pick whichever world axis is least aligned with the bone; its
        # rejection cannot be zero
        k = min(range(3), key=lambda i: abs(along[i]))
        basis = [0.0, 0.0, 0.0]; basis[k] = 1.0
        out = orth(basis)
    out = norm(out)
    third = norm([along[1]*out[2] - along[2]*out[1],
                  along[2]*out[0] - along[0]*out[2],
                  along[0]*out[1] - along[1]*out[0]])
    # columns: where each of the panel's local axes goes
    col = [None, None, None]
    col[long_] = along
    col[thin] = out
    col[mid] = third
    M = [[col[0][r], col[1][r], col[2][r]] for r in range(3)]

    # RIGHT-HANDED FIRST. Which local axis is longest decides which column
    # gets `along`, `out` and `third` -- so the handedness fell out of the
    # panel's proportions, and 20 of them came out left-handed. A left-handed
    # frame MIRRORS the panel: a door becomes its own opposite, which is
    # invisible on a symmetric plate and obviously wrong on anything with a
    # feature. Only column norms were being checked, never the determinant.
    def _det(m):
        return (m[0][0]*(m[1][1]*m[2][2] - m[1][2]*m[2][1])
                - m[0][1]*(m[1][0]*m[2][2] - m[1][2]*m[2][0])
                + m[0][2]*(m[1][0]*m[2][1] - m[1][1]*m[2][0]))

    if _det(M) < 0:
        for r in range(3):
            M[r][mid] = -M[r][mid]

    # TAKE THE SHORT WAY. A plate is the same plate flipped: mapping its
    # local axes onto the target frame has four valid answers, and the naive
    # one asked some panels for a full 180-degree reorientation. Symmetric
    # pairs then turned opposite ways, because the arbitrary choice differed
    # per side. Flipping two columns keeps the determinant positive and cuts
    # the angle; doing it for both candidate pairs and keeping the smallest
    # bounds the worst case at 90 degrees.
    def _angle(m):
        tr = m[0][0] + m[1][1] + m[2][2]
        return _m.degrees(_m.acos(max(-1.0, min(1.0, (tr - 1.0) / 2.0))))

    best = M
    for fa, fb in ((0, 1), (0, 2), (1, 2)):
        C = [row[:] for row in M]
        for r in range(3):
            C[r][fa] = -C[r][fa]
            C[r][fb] = -C[r][fb]
        if _angle(C) < _angle(best):
            best = C
    M = best
    # a panel placed through a degenerate frame is silently flattened, so
    # check rather than hope
    for c in range(3):
        L = sum(M[r][c]**2 for r in range(3)) ** 0.5
        assert abs(L - 1.0) < 1e-6, f"column {c} is not unit: {L}"
    assert _det(M) > 0.99, f"frame is left-handed: det {_det(M):.3f}"
    return M


# WHICH VEHICLE. The rig, cladding and fold are shared; only the source car
# differs, because every vehicle's catalogue maps its parts onto the same
# named vocabulary. `VEHICLE=falcon python3 build_viewer.py` builds the Falcon.
import os as _os
VEHICLE = _os.environ.get('VEHICLE', 'orion')


def _dress():
    if VEHICLE == 'orion':
        import orion_dress as m
    else:
        import sys as _sys
        _sys.path.insert(0, _os.path.join(_os.path.dirname(_os.path.abspath(__file__)),
                                          'vehicles'))
        m = __import__(f'{VEHICLE}_dress')
    return m


def _seat(name, default):
    """A vehicle can re-seat a part without touching the shared table."""
    import orion_cladding as CL
    return getattr(_dress(), 'SEATS', {}).get(name) or CL.CLAD.get(name, default)


def _ground_poses():
    """The shared get-up, with a vehicle's own fold patched into pose 0."""
    fold = dict(getattr(_dress(), 'FOLD', {}))
    extra_bulk = fold.pop('_bulk', {})

    def patch(p):
        # _bulk MERGES: a plain update would replace the fold's torso squeeze
        return {**p, **fold, '_bulk': {**p.get('_bulk', {}), **extra_bulk}}
    return [(n, patch(p)) if n == 'prone' else (n, p) for n, p in R.POSES]


def _air_poses():
    """A BARREL ROLL: the jet rolls about its flight axis (world x, the rig's
    outermost `_roll`) while it unfolds, pitches up and lands. Root positions
    are interpolated between already-baked ones, so a tucked pose (which
    carries the fold's forward shift) never jumps against an untucked one."""
    if getattr(_dress(), 'LEVITATE', False):
        return _levitate_poses()
    # every other vehicle keeps the levitate-and-spin transform for ZERO
    # GRAVITY, where a push-up has nothing to push against
    zg = _levitate_poses()
    zero = {'poses_zerog': zg['poses_air'], 'weights_zerog': zg['weights_air']}
    if not getattr(_dress(), 'AIR', False):
        return zero
    P = dict(_ground_poses())
    fold, spread, rise, settle = P['prone'], P['spread'], P['rise'], P['settle']
    bp = {n: R._scaled_pos(P[n]) for n in ('prone', 'spread', 'rise', 'settle')}

    def blend(a, b, f):
        o = {}
        for k in sorted(set(a) | set(b)):   # sorted: a set's order changes per run
            if k.startswith('_'):
                continue
            va, vb = a.get(k, (0, 0, 0)), b.get(k, (0, 0, 0))
            o[k] = tuple(x + (y - x) * f for x, y in zip(va, vb))
        return o

    def lerp3(a, b, f, up=0.0):
        return [a[i] + (b[i] - a[i]) * f + (up if i == 1 else 0.0) for i in range(3)]

    seq = [
        ('fold',    {**{k: v for k, v in fold.items() if not k.startswith('_')},
                     '_rot': fold['_rot'], '_bulk': fold.get('_bulk', {})}, list(bp['prone'])),
        ('roll 180', {**blend(fold, spread, 0.35), '_rot': (0, 0, -90), '_roll': 180,
                      '_bulk': fold.get('_bulk', {})}, lerp3(bp['prone'], bp['spread'], 0.35, 60)),
        ('roll 360', {**blend(spread, rise, 0.45), '_rot': (0, 0, -50), '_roll': 360},
                     lerp3(bp['spread'], bp['rise'], 0.45, 90)),
        ('upright', {**blend(rise, settle, 0.0), '_rot': (0, 0, 0), '_roll': 360},
                    lerp3(bp['rise'], bp['rise'], 0, 70)),
        ('land',    {**blend(settle, settle, 0.0), '_rot': (0, 0, 0), '_roll': 360}, list(bp['settle'])),
    ]
    return {'poses_air': [{'n': n, 'a': {**{k: _jsonable(v) for k, v in a.items()}, '_pos': pos}}
                          for n, a, pos in seq],
            'weights_air': [0.30, 0.28, 0.24, 0.18], **zero}


def _levitate_poses():
    """A hover vehicle does not stand up: it LEVITATES -- lifts out of the car,
    pitches upright and opens into a floating mage. No push-up, no roll. The
    hover height itself is added after grounding in the page (grounding pulls
    the lowest point to the floor, so a raised root alone would be undone)."""
    P = dict(_ground_poses())
    fold, spread, rise = P['prone'], P['spread'], P['rise']
    bp = {n: R._scaled_pos(P[n]) for n in ('prone', 'spread', 'rise')}

    def blend(a, b, f):
        return {k: tuple(x + (y - x) * f for x, y in zip(a.get(k, (0, 0, 0)), b.get(k, (0, 0, 0))))
                for k in sorted(set(a) | set(b)) if not k.startswith('_')}

    def lerp3(a, b, f, up=0.0):
        return [a[i] + (b[i] - a[i]) * f + (up if i == 1 else 0.0) for i in range(3)]
    # the mage's legs: together, knees soft, toes pointed down (measured axes:
    # thigh +z swings back, shin +z bends the knee, foot +z points the toes)
    mage = dict(blend(rise, rise, 0))
    for sd, m in (('l', 1), ('r', 1)):
        for k, d in (('thigh', (0, 0, -8)), ('shin', (0, 0, 24)), ('foot', (0, 0, 34))):
            a = mage.get(f'{k}_{sd}', (0, 0, 0)); mage[f'{k}_{sd}'] = (a[0] + d[0], a[1] + d[1], a[2] + d[2])
    seq = [
        ('fold', {**{k: v for k, v in fold.items() if not k.startswith('_')},
                  '_rot': fold['_rot'], '_bulk': fold.get('_bulk', {})}, list(bp['prone'])),
        ('lift', {**blend(fold, spread, 0.3), '_rot': (0, 0, -70), '_bulk': fold.get('_bulk', {})},
                 lerp3(bp['prone'], bp['spread'], 0.3, 50)),
        # a SPIN while hovering: half a turn, so it ends facing the way the
        # vehicle drove -- it had come out of the car facing backwards
        ('upright', {**blend(spread, mage, 0.7), '_rot': (0, 90, -20)}, lerp3(bp['spread'], bp['rise'], 0.7, 30)),
        ('mage', {**mage, '_rot': (0, 180, 0)}, list(bp['rise'])),
    ]
    return {'poses_air': [{'n': n, 'a': {**{k: _jsonable(v) for k, v in a.items()}, '_pos': pos}}
                          for n, a, pos in seq],
            'weights_air': [0.34, 0.33, 0.33], 'air_label': 'levitate',
            'fold_floor': getattr(_dress(), 'FOLD_FLOOR', None),
            'hover': float(getattr(_dress(), 'HOVER', 0.0))}


def _hinge(g, grp):
    """A folding part's hinge: its root line, measured from its inner edge.

    The root is the edge nearest the centreline (smallest |z|): the vertices
    within a few units of it, their centre is the hinge point and their
    principal direction the hinge axis -- read off the geometry, because a
    wing's root line is swept and not guessable.
    """
    h = getattr(_dress(), 'HINGES', {}).get(g)
    if not h:
        return None
    import numpy as _np
    V = _np.array(grp['v'], dtype=float)
    az = _np.abs(V[:, 2])
    edge = V[az <= az.min() + 6.0]
    root = edge.mean(0)
    if len(edge) >= 2:
        _u, _s, vt = _np.linalg.svd(edge - root)
        axis = vt[0] / _np.linalg.norm(vt[0])
        if axis[0] < 0:
            axis = -axis
    else:
        axis = _np.array([1.0, 0.0, 0.0])
    bone, seat, deg = h[:3]
    size = float(h[3]) if len(h) > 3 else None
    return [bone, list(seat), float(deg), [round(x, 3) for x in root], [round(x, 5) for x in axis], size]


def _direct():
    """Decisions made by nearest known part (partnet/decide.py), if any. A
    hand-tuned SEATS entry always wins over a looked-up one."""
    import json
    here = _os.path.dirname(_os.path.abspath(__file__))
    p = _os.path.join(here, 'vehicles', f'{VEHICLE}_direct.json')
    return json.load(open(p)) if _os.path.exists(p) else {}


def _wedges():
    """Baked wedge paths for this vehicle (tools/wedge.js), if solved yet."""
    import json
    here = _os.path.dirname(_os.path.abspath(__file__))
    for cand in (_os.path.join(here, f'{VEHICLE}_wedge.json'),
                 _os.path.join(here, 'vehicles', f'{VEHICLE}_wedge.json')):
        if _os.path.exists(cand):
            return json.load(open(cand))
    return {}


def pack_car(G, names):
    """Quantise and gzip the car geometry, IN THE BUILD.

    This used to be a separate script I ran by hand, and the blob and the
    placement table drifted apart every time the split changed -- 67 groups
    of geometry against 168 placements, and the viewer threw on the first
    group that had no placement. Four sync failures of the same shape.

    Now it is derived from the same `G` the placements come from, so they
    cannot disagree.
    """
    import gzip, base64, struct
    allv = [v for g in names for v in G[g]['v']]
    lo = [min(v[a] for v in allv) for a in range(3)]
    hi = [max(v[a] for v in allv) for a in range(3)]
    sc = [(hi[a] - lo[a]) or 1.0 for a in range(3)]
    blob = bytearray()
    blob += struct.pack('<I', len(names))
    blob += struct.pack('<6f', *lo, *sc)
    meta = []
    for g in names:
        d = G[g]
        meta.append({'g': g, 'nv': len(d['v']), 'nt': len(d['f']),
                     'c': [round(x, 2) for x in d['c']],
                     'r': [round(x / 2, 1) for x in d['size']]})
        for v in d['v']:
            for a in range(3):
                blob += struct.pack(
                    '<H', int(round((v[a] - lo[a]) / sc[a] * 65535)))
        for f in d['f']:
            for i in f:
                blob += struct.pack('<I', i)
    gz = gzip.compress(bytes(blob), 9, mtime=0)   # no timestamp: reproducible builds
    return base64.b64encode(gz).decode(), meta


def _jsonable(v):
    if isinstance(v, dict):
        return {k: _jsonable(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [_jsonable(x) for x in v]
    return v


def _axis_frame(size, axis):
    """A frame whose THIN axis is the stated one, with no reference to the
    bone. For a wheel the thin axis is the axle, and where the axle points is
    a fact about the wheel's mounting, not something to derive from whatever
    limb happens to carry it."""
    import math as _m
    order = sorted(range(3), key=lambda i: size[i])
    thin, mid, long_ = order[0], order[1], order[2]
    L = _m.sqrt(sum(x * x for x in axis)) or 1.0
    n = [x / L for x in axis]
    k = min(range(3), key=lambda i: abs(n[i]))
    t = [0.0, 0.0, 0.0]; t[k] = 1.0
    d = sum(t[i] * n[i] for i in range(3))
    a = [t[i] - n[i] * d for i in range(3)]
    L = _m.sqrt(sum(x * x for x in a)) or 1.0
    a = [x / L for x in a]
    b = [n[1]*a[2] - n[2]*a[1], n[2]*a[0] - n[0]*a[2], n[0]*a[1] - n[1]*a[0]]
    col = [None, None, None]
    col[thin] = n; col[long_] = a; col[mid] = b
    M = [[col[0][r], col[1][r], col[2][r]] for r in range(3)]
    det = (M[0][0]*(M[1][1]*M[2][2] - M[1][2]*M[2][1])
           - M[0][1]*(M[1][0]*M[2][2] - M[1][2]*M[2][0])
           + M[0][2]*(M[1][0]*M[2][1] - M[1][1]*M[2][0]))
    if det < 0:
        for r in range(3):
            M[r][mid] = -M[r][mid]
    return M


def car_spec():
    """The car's geometry, packed by group, plus where each group goes.

    130,727 verts and 250,000 triangles: too much to embed as text, so
    positions are quantised to 16 bits over ONE shared bounding box (0.007
    units on a 458-long car, three orders below anything visible) and the
    whole thing is gzipped. 2.0 MB of base64 rather than 11.

    Indices stay exact. Quantising those would be a different kind of wrong.
    """
    import orion_cladding as CL, orion_split as SP
    from orion_balance import RADIUS, DEFAULT_R
    G = _dress().groups()
    # Cut the big contiguous body panels into per-bone sections. A 224-unit
    # cabin panel cannot ride a 44-unit chest bone: as one group it had to be
    # scaled to a third and hung off a single point, which is why the shell
    # read as a lump instead of as bodywork. Sections inherit their parent's
    # fate and layer, and take the bone the cut assigned them.
    G = SP.split_all(G, only=SP.PANELS)
    names = sorted(G)
    place = {}
    import orion_dress as _OD
    fit = _OD.autofit(G, R.solve(R.POSES[0][1]))
    for g in names:
        parent = G[g].get('from', g)
        if '@' in g:
            # the cut chose the bone; everything else comes from the parent
            _b, off, ang, scale, *_x = _seat(parent, CL.FALLBACK)
            bone = G[g]['bone']
        else:
            bone, off, ang, scale, *_x = _seat(g, CL.FALLBACK)
        # DIRECT: a looked-up decision (bone, seat, scale, fate) taken as-is
        direct = (None if parent in getattr(_dress(), 'SEATS', {})
                  else _direct().get(parent))
        if direct:
            bone = direct['b']
        br = RADIUS.get(bone, DEFAULT_R)
        blen = R.BONES[bone][1]
        # fate decides how big it gets and how far out it sits
        fate = CL.fate(parent)
        # STREAM_ALL: only the curated armour stays. A "frame" part travelled
        # to the body and STAYED -- the Falcon's 325-long floor pan and orion's
        # 400-long scattered clips were left hanging on the robot. Everything
        # that is not armour now compacts in place and is absorbed.
        # a vehicle can keep more of its own shell as armour than orion's
        # shared table does: the shell was being eaten away almost entirely
        if parent in getattr(_dress(), 'ARMOUR_EXTRA', ()):
            fate = CL.ARMOUR
        morph = getattr(_dress(), 'ARMOUR_MORPH', {}).get(parent)
        if direct:
            fate = CL.ARMOUR if direct['f'] == 'armour' else CL.STOW
            morph = morph or direct.get('mo')
        if morph:
            fate = CL.ARMOUR
        if getattr(_dress(), 'STREAM_ALL', False) and fate != CL.ARMOUR:
            fate = CL.STOW
        k = fit[g] * scale * CL.SCALE_BY_FATE[fate]
        # a reshaped part stacks over nothing, so it takes the innermost layer:
        # glass is layer 3, which parked orion's shoulder spikes 8 units out
        lay = ((1 if morph else CL.layer(parent)) * max(CL.PLATE * 0.5, br * CL.PLATE_FRAC)
               if fate == CL.ARMOUR else 0.0)
        # SEAT the panel: its inner face touches the bone's surface.
        #
        # The previous rule ADDED a fixed outboard push to an already-authored
        # offset, so the two compounded -- a wheel authored at x=14 on an upper
        # arm of radius 10 was pushed to 23 and floated 15 units clear. Five of
        # 43 groups were doing this, all on the small bones.
        #
        # Computing it from the group's own half-extent means a big panel sits
        # further out than a small one because it IS bigger, not because a
        # constant said so.
        # Push out by the panel's THIN half-extent, because the alignment
        # puts its thin axis outward. Using the pre-rotation x extent meant a
        # wheel was pushed out by half its DIAMETER rather than half its
        # thickness -- 30 units clear of the body on a limb 10 units thick.
        thin = min(G[g]['size'])
        # a RESHAPED part sits at its reshaped thickness: a curved quarter
        # panel's box is 90 thick, and the trim bar it becomes was parked
        # ~45 out from the thigh, floating beside it
        if morph:
            thin *= 0.35 if morph == 'trim' else 0.6
        # and clears the limb's own scale plates, which stand ~4 proud of the
        # surface: the trim bar's inner face sat at +3 and was half buried
        hx = hz = thin * 0.5 * k + (2.5 if morph else 0.0)
        sx = 1 if off[0] > 0 else (-1 if off[0] < 0 else 0)
        sz = 1 if off[2] > 0 else (-1 if off[2] < 0 else 0)
        # ALONG THE BONE, not along local +y.
        #
        # A limb's rest direction is (0,-1,0), so a positive local-y offset
        # runs the OPPOSITE way from the bone -- up past the shoulder and over
        # the head. Measured: arch_r's top reached y 319 against a head at 294,
        # and every group on an upper arm did the same. Offsets are now
        # projected onto the bone's own axis, so "half way down the limb"
        # means that on every bone regardless of which way it points.
        d = R.BONES[bone][2]
        # off[1] between -1 and 1 is a FRACTION of the bone; anything else is
        # an absolute nudge from the 45% default.
        #
        # The formula added `blen * 0.45` unconditionally, so asking for a
        # wheel at the knee by passing 0 still put it halfway down the shin --
        # and at t=0.47, with the machine crouched, that hung it 16 units
        # below its own feet. A position along a bone has to be sayable.
        if -1.0 <= off[1] <= 1.0 and off[1] != 0.0:
            along = blen * abs(off[1])
        elif off[1] == 0.0:
            along = blen * 0.45
        else:
            along = off[1] * 0.5 + blen * 0.45
        # An across-the-limb offset only contributes its SIGN: the distance is
        # recomputed from the bone and the part. So a seat can carry an
        # optional 5th entry (extra_x, extra_z) that is added on top -- the
        # doors met across the spine at every "22/28/34" because none of
        # those numbers were ever used.
        ex, ez = (_x[0] if _x else (0.0, 0.0))
        off2 = (sx * (br * 0.55 + hx * 0.55 + lay + ex) + d[0] * along if sx
                else off[0] + d[0] * along,
                d[1] * along,
                sz * (br * 0.55 + hz * 0.55 + lay + ez) + d[2] * along if sz
                else off[2] + d[2] * along)
        if direct:
            off2 = tuple(direct['o'])
        # the outward direction is whichever side the offset puts it on
        outv = [off2[0], 0.0, off2[2]]
        if abs(outv[0]) + abs(outv[2]) < 1e-6:
            outv = [0.0, 0.0, -1.0]        # centreline parts face forward
        forced = CL.AXLE.get(parent)
        if forced is not None:
            # stated axle: skip the perpendicular-to-bone projection entirely
            M = _axis_frame(G[g]['size'], list(forced))
        else:
            M = _align(G[g]['size'], R.BONES[bone][2], outv)
        # VEHICLE FORM: where the panel sits on the CAR, in the same space the
        # rig is drawn in. Not a placement at all -- it is the original
        # geometry, which is why it is guaranteed to be a car.
        #
        # Every previous attempt tried to make the car out of bone-relative
        # placements and could not: a panel hung off a bone is wherever that
        # bone is, and no set of offsets reassembles a vehicle from a standing
        # skeleton. The car end has to be the identity.
        place[g] = {'b': bone, 'o': [round(v, 2) for v in off2],
                    'car': [round(v, 2) for v in G[g]['c']],
                    'm': [[round(v, 5) for v in row] for row in M],
                    'e': list(ang), 's': round(direct['s'] if direct else k, 4), 'f': fate,
                    # per-vehicle timing overrides the shared table
                    'dis': list(getattr(_dress(), 'DISSOLVE', {}).get(parent)
                                or (direct and direct.get('dis')) or CL.dissolve(parent)),
                    'st': 1 if (fate != CL.ARMOUR and
                                (parent in getattr(_dress(), 'STREAM', ())
                                 or getattr(_dress(), 'STREAM_ALL', False))) else 0,
                    'lay': CL.layer(parent),
                    'spin': CL.SPIN.get(parent, 0.0),
                    'wedge': _wedges().get(g),
                    'hinge': _hinge(g, G[g]),
                    'flip': parent in getattr(_dress(), 'FLIP', ()),
                    # reshape as it attaches: kind, then the panel's own long /
                    # middle / thin axes (as car-axis indices) and half extents
                    'mo': (lambda sz: [1 if morph == 'spike' else 2,
                                       *sorted(range(3), key=lambda a: -sz[a]),
                                       *[round(sz[a] * 0.5, 2) for a in sorted(range(3), key=lambda a: -sz[a])]]
                           )(G[g]['size']) if morph else None,
                    'keep': (list(getattr(_dress(), 'KEEP_CUBES', {})[parent])
                             if parent in getattr(_dress(), 'KEEP_CUBES', {}) else None),
                    'c': [round(v, 2) for v in G[g]['c']]}
    # MIRRORED PAIRS SHARE A FRAME.
    #
    # The short-way search answers each panel on its own, so a left door and a
    # right door -- identical geometry, mirrored -- came out 90 and 120
    # degrees. Deriving the right from the left guarantees they match:
    # reflecting a rotation across the mirror plane is S*M*S with
    # S = diag(1,1,-1), which flips handedness twice and so preserves it.
    for g in list(place):
        if not g.endswith('_l'):
            continue
        twin = g[:-2] + '_r'
        if twin not in place:
            continue
        M = place[g]['m']
        place[twin]['m'] = [[M[0][0], M[0][1], -M[0][2]],
                            [M[1][0], M[1][1], -M[1][2]],
                            [-M[2][0], -M[2][1], M[2][2]]]
    # the wheels are named fl/fr and rl/rr, not _l/_r
    for a, b in (('wheel_fl', 'wheel_fr'), ('wheel_rl', 'wheel_rr')):
        if a in place and b in place:
            M = place[a]['m']
            place[b]['m'] = [[M[0][0], M[0][1], -M[0][2]],
                             [M[1][0], M[1][1], -M[1][2]],
                             [-M[2][0], -M[2][1], M[2][2]]]

    b64, meta = pack_car(G, names)
    return names, place, b64, meta


def spec():
    ms = BD.body_meshes()
    body = {}
    for n, m in ms.items():
        if not m.v:
            continue
        body[n] = {'v': [round(c, 1) for vv in m.v for c in vv],
                   'f': [i for t in m.f for i in t],
                   # per-vertex 0..1, 1 at the joints: drives the shading
                   'w': BD.joint_weight(m, R.BONES[n][1])}
    # The core is parameterised rather than baked: one cube mesh plus a list
    # of shard descriptors, placed in JS. Baking a snapshot per t would be
    # five times the data and would still step rather than flow.
    import math as _m
    cube = BD._y_along(BD.loft([(-1, 0.86, -0.86, 0.86, 0.10),
                                (-0.86, 1, -1, 1, 0.05),
                                (0.86, 1, -1, 1, 0.05),
                                (1, 0.86, -0.86, 0.86, 0.10)], 4, sub=1))
    shards = []
    for i in range(14):
        a = _m.tau * i / 14
        rise = _m.sin(a * 2.3 + i) * 0.5 + 0.5
        shards.append({'a': round(a, 4), 'r': round(rise, 4)})
    core = {'cube': {'v': [round(c, 3) for vv in cube.v for c in vv],
                     'f': [i for t in cube.f for i in t]},
            'shards': shards, 'size': BD.CORE_SIZE, 'bone': BD.CORE_BONE}

    rig = {
        'bones': [{'n': k, 'p': v[0], 'l': v[1], 'd': list(v[2])}
                  for k, v in R.BONES.items()],
        # `list()` on a dict returns its KEYS -- `_bulk` was serialising as
        # ["chest","pelvis","spine"] and the renderer read scale factors off
        # the names. A serialiser that flattens by type has to know the
        # difference between a vector and a mapping.
        # ROOT POSITIONS ARE BAKED, not left for the renderer to derive.
        #
        # `_scaled_pos` applies SCALE and PRONE_SHIFT inside the Python
        # solver, and the JS read `_pos` raw -- so the viewer has been placing
        # 1.40x bones on unscaled roots while every Python render used the
        # scaled ones. Two solvers agreeing on the maths and disagreeing on
        # the inputs is worse than one being wrong, because each looks right
        # on its own.
        'poses': [{'n': n,
                   'a': {**{k: _jsonable(v) for k, v in p.items()},
                         '_pos': list(R._scaled_pos(p))}}
                  for n, p in _ground_poses()],
        'weights': R.WEIGHTS,
        **_air_poses(),
        'weapons': getattr(_dress(), 'WEAPONS', []),
        'contacts': list(R.CONTACTS), 'floor': R.FLOOR,
        'radius': BAL.RADIUS, 'default_r': BAL.DEFAULT_R,
        'contact_h': BAL.CONTACT_H,
        'volumes': [{'n': n, 'x': list(v['x']), 'y': list(v['y']), 'z': list(v['z'])}
                    for n, v in VOL.VOLUMES],
    }
    import orion_muscle as MU
    from orion_balance import RADIUS as _RAD, DEFAULT_R as _DR
    rig['muscles'] = MU.spec(R.BONES, _RAD, _DR)
    # SCALE ARMOUR, sized from the panel area each bone was allocated, so the
    # machine is clad in proportion to the car it came from rather than to a
    # number somebody picked.
    import orion_scale as SC, orion_cladding as _CL
    from orion_balance import RADIUS as _RA, DEFAULT_R as _DRa
    _G = _dress().groups()
    _area = {}
    for _g, _d in _G.items():
        _b = _seat(_g, _CL.FALLBACK)[0]
        _sz = sorted(_d['size'], reverse=True)
        _area[_b] = _area.get(_b, 0.0) + _sz[0] * _sz[1]
    rig['scales'] = SC.build(R.BONES, _RA, _DRa, _area)
    # plate character per bone, measured from the car parts that became it
    rig['plate'] = SC.panel_profile(_G, _CL.CLAD, _CL.FALLBACK)

    rig['core'] = core
    names, place, b64, meta = car_spec()
    # per vehicle: instanced cube particles (the Falcon) or the shader
    # compaction on subdivided panels (orion, which read better that way)
    rig['car'] = {'groups': names, 'place': place,
                  'cubes': bool(getattr(_dress(), 'CUBES', True))}
    return rig, body, b64, meta


HTML = r'''<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>__TITLE__</title>
<style>
:root{--bg:#15171b;--panel:#1c1f25;--line:#2b3039;--ink:#d7dbe2;--dim:#7e8794;
  --accent:#8fb8ff;--ok:#7fc79a;--warn:#e0a458;--mono:ui-monospace,Menlo,Consolas,monospace}
*{box-sizing:border-box}
html,body{margin:0;height:100%;background:var(--bg);color:var(--ink);
  font:12px/1.45 var(--mono);overflow:hidden}

/* Layout: the canvas takes whatever is left, and the CONTROLS ARE PINNED.
 * The previous version put everything in one scrolling side panel, so on a
 * short frame the slider and the play button scrolled out of reach -- the two
 * things you actually need. Controls now sit in a fixed strip that never
 * scrolls; only the readouts do. */
#app{display:flex;flex-direction:column;height:100%}
#main{flex:1;display:flex;min-height:0}
#view{flex:1;min-width:0;min-height:0;position:relative;overflow:hidden}
/* The canvas MUST be sized in CSS. renderer.setSize(w,h,false) deliberately
 * does not set a style, so with devicePixelRatio 2 the element carried
 * width="1600" as an attribute and no CSS -- and an attribute-sized canvas
 * displays at that many CSS pixels. It rendered at twice its container,
 * overflowed, and pushed the control bar off the bottom of the frame. */
#view canvas{display:block;width:100%;height:100%}
#side{width:240px;flex:none;background:var(--panel);border-left:1px solid var(--line);
  overflow-y:auto;padding:10px;font-size:11px}
#bar{flex:none;background:var(--panel);border-top:1px solid var(--line);
  padding:8px 12px;display:flex;align-items:center;gap:10px;flex-wrap:wrap}
#bar .grow{flex:1 1 220px;min-width:160px}
h1{font-size:11px;margin:0 0 2px;letter-spacing:.09em;text-transform:uppercase}
.sub{color:var(--dim);font-size:10px;margin-bottom:8px}
button{background:#242932;color:var(--ink);border:1px solid var(--line);border-radius:4px;
  padding:5px 9px;font:11px var(--mono);cursor:pointer;white-space:nowrap}
button:hover{border-color:var(--accent)}
button.on{border-color:var(--ok);color:var(--ok)}
.poses{display:flex;gap:4px;flex-wrap:wrap}
.poses button{padding:4px 7px;font-size:10px}
label{color:var(--dim);font-size:10px;white-space:nowrap}
input[type=range]{width:100%;margin:0}
table{width:100%;border-collapse:collapse;font-size:10px;margin-top:4px}
th{text-align:left;color:var(--dim);font-weight:400;padding:2px 3px;border-bottom:1px solid var(--line)}
td{padding:1px 3px;border-bottom:1px solid #22262d}
td.n{color:var(--dim)}
.k{color:var(--accent)}.w{color:var(--warn)}.g{color:var(--ok)}
#err{display:none;position:absolute;inset:8px;background:#2a1d1d;border:1px solid #6a3a3a;
  color:#ffb4b4;padding:10px;font-size:11px;white-space:pre-wrap;overflow:auto;z-index:9}
@media(max-width:760px){#side{display:none}}
@media(max-height:520px){#side{display:none}}
</style></head><body>
<div id="app">
  <div id="main">
    <div id="view"><div id="err"></div></div>
    <div id="side">
      <h1>Orion rig</h1>
      <div class="sub">22 bones · 6 poses · 43 car groups</div>
      <table><thead><tr><th>joint</th><th>y</th><th>z</th></tr></thead>
        <tbody id="rows"></tbody></table>
      <div class="sub" id="note" style="margin-top:8px"></div>
    </div>
  </div>
  <div id="bar">
    <div class="poses" id="poses"></div>
    <div class="grow">
      <label>t <span id="tv" class="k">0.00</span> — <span id="pn"></span></label>
      <input type="range" id="t" min="0" max="1" step="0.002" value="0">
    </div>
    <button id="play">play</button><button id="spin" class="on">spin</button><button id="mode" title="air: barrel roll / ground: push-up">air</button><select id="motion" title="robot-form motion (runs after the transform)"><option value="">transform</option><option>idle</option><option value="locomote">walk / run</option><option>walk</option><option>run</option><option>jump</option><option>dash</option><option>punch</option><option>float</option><option>glide</option><option>cast</option><option>weapon</option><option>ranged</option></select><label class="ml">g <input id="grav" type="range" min="0" max="2" step="0.05" value="1"></label><label class="ml">walk&#8596;run <input id="gait" type="range" min="0" max="1" step="0.01" value="0.3"></label><label class="ml">mass <input id="mass" type="range" min="0.5" max="2" step="0.05" value="1"></label><label class="ml">foot thrust <input id="thr" type="range" min="0" max="0.8" step="0.05" value="0"></label><label class="ml">knee damage <input id="limp" type="range" min="0" max="1" step="0.05" value="0"></label><span id="mhud" class="ml"></span>
    <button id="bodyb" class="on">body</button><button id="bones">bones</button>
    <button id="car" class="on">car</button><button id="core" class="on">core</button>
    <button id="axes">volume</button><button id="heft" class="on">heft</button>
    <button id="mus" class="on">muscle</button><button id="scl" class="on">scales</button><button id="fit">fit</button>
  </div>
</div>
<script src="https://cdnjs.cloudflare.com/ajax/libs/three.js/r128/three.min.js"></script>
<script>
const RIG = __RIG__;
const BODY = __BODY__;

/* Anything that throws here used to leave a black rectangle and no clue. */
function fail(e){
  const el=document.getElementById('err');
  el.style.display='block';
  el.textContent='the viewer stopped:\n\n'+(e && e.stack ? e.stack : e);
  console.error(e);
}
window.addEventListener('error', ev => fail(ev.error || ev.message));

try{
/* ── kinematics ───────────────────────────────────────────────────────────
 * ZYX euler -> matrix, matching the Python solver exactly. A pose is a list
 * of ANGLES and forward kinematics gives positions; bone lengths never
 * change, which is what makes the poses hand-authorable. */
function rotMat(rx, ry, rz){
  const d=Math.PI/180;
  const cx=Math.cos(rx*d), sx=Math.sin(rx*d);
  const cy=Math.cos(ry*d), sy=Math.sin(ry*d);
  const cz=Math.cos(rz*d), sz=Math.sin(rz*d);
  return [[cz*cy, cz*sy*sx - sz*cx, cz*sy*cx + sz*sx],
          [sz*cy, sz*sy*sx + cz*cx, sz*sy*cx - cz*sx],
          [-sy,   cy*sx,            cy*cx]];
}
const mul=(a,b)=>[0,1,2].map(i=>[0,1,2].map(j=>a[i][0]*b[0][j]+a[i][1]*b[1][j]+a[i][2]*b[2][j]));
const ap =(m,v)=>[0,1,2].map(i=>m[i][0]*v[0]+m[i][1]*v[1]+m[i][2]*v[2]);

function solve(pose){
  const out={};
  const rpos=pose._pos||[0,0,0];
  let rrot=rotMat(...(pose._rot||[0,0,0]));
  /* `_roll` applied OUTERMOST, about world x, after the lay-down.
   *
   * The root euler is ZYX, so with the lay-down in rz any rx is applied first
   * and rotates the body's front within the horizontal plane -- it can face
   * +x or -x but never DOWN. The machine was lying on its SIDE in every pose
   * and standing up 90 degrees off the car's axis, and no value of the three
   * existing angles could fix it. */
  const roll=pose._roll||0;
  if(roll) rrot=mul(rotMat(roll,0,0), rrot);
  for(const b of RIG.bones){
    let head, pm;
    if(b.p===null){ head=rpos; pm=rrot; }
    else { head=out[b.p].tail; pm=out[b.p].m; }
    const m=mul(pm, rotMat(...(pose[b.n]||[0,0,0])));
    const d=ap(m,b.d);
    out[b.n]={head, tail:[head[0]+d[0]*b.l, head[1]+d[1]*b.l, head[2]+d[2]*b.l], m};
  }
  return out;
}

function blend(a,b,f){
  const keys=new Set([...Object.keys(a),...Object.keys(b)]);
  const o={};
  for(const k of keys){
    if(k==='_bulk'||k==='_tuck') continue;
    if(k==='_roll'){ o[k]=(a[k]||0)+((b[k]||0)-(a[k]||0))*f; continue; }
    const va=a[k]||[0,0,0], vb=b[k]||[0,0,0];
    if(!Array.isArray(va)||!Array.isArray(vb)) continue;
    o[k]=[0,1,2].map(i=>va[i]+(vb[i]-va[i])*f);
  }
  return o;
}

/* Weighted segments: getting off the floor is most of the work, and the
 * settle is a small adjustment that should not eat a sixth of the slider.
 * `_bulk` scales the MESH per bone -- bone lengths never change, so the chest
 * cavity can open for the central pistons without the skeleton knowing. */
/* The body HOLDS folded while the shell cracks. Without it the machine is
 * 122 units tall by 9% -- inside a car 106 tall that has not opened yet. The
 * car break was never delayed; the body was early. */
/* 0.34, not 0.16.
 *
 * Measured: at t=0.24 the skeleton stood 41 units above the car's roof and at
 * 0.28 it was 94 above -- while the panels had cracked only 7 to 12 units and
 * the shell was, to the eye, still shut. The machine was climbing out through
 * its own roof.
 *
 * Two changes together, because either alone does not fix it: the body holds
 * longer, AND the panels start travelling at 0.30 instead of 0.50, so they
 * are already clearing when it rises. */
/* HOLD and the align window solved TOGETHER, by sweeping both against the
 * roof line. At 0.24 the skeleton stood 41 units above the car's roof and at
 * 0.28 it was 94 above, while the panels had cracked 7 to 12 units and the
 * shell was to the eye still shut -- the machine was climbing out through its
 * own roof.
 *
 *     hold  align        worst breach
 *     0.34  0.16..0.68        4
 *     0.38  0.20..0.72        5
 *     0.42  0.16..0.68        0   <-
 *     0.42  0.24..0.76        6
 *
 * Neither number fixes it alone: the body has to wait AND the panels have to
 * be travelling by the time it does not. */
const HOLD=0.42;
function poseAt(t){
  t=(t-HOLD)/(1-HOLD);
  t=Math.max(0,Math.min(0.999999,t));
  const W=RIG.weights, tot=W.reduce((a,b)=>a+b,0);
  let acc=0;
  for(let i=0;i<W.length;i++){
    const frac=W[i]/tot;
    if(t<=acc+frac || i===W.length-1){
      let f=(t-acc)/Math.max(1e-9,frac);
      f=Math.max(0,Math.min(1,f)); f=f*f*(3-2*f);
      const A=RIG.poses[i].a._bulk||{}, B=RIG.poses[i+1].a._bulk||{};
      /* bulk is a 3-VECTOR: radial, along, radial. A scalar could only
       * shrink the ribcage by shortening it too, and a cage that gets
       * shorter has lost its span. */
      const bulk={};
      const v3=(x)=>Array.isArray(x)?x:[x===undefined?1:x,x===undefined?1:x,
                                        x===undefined?1:x];
      for(const k of new Set([...Object.keys(A),...Object.keys(B)])){
        const a3=v3(A[k]), b3=v3(B[k]);
        bulk[k]=[0,1,2].map(i=>a3[i]+(b3[i]-a3[i])*f);
      }
      return {p:blend(RIG.poses[i].a,RIG.poses[i+1].a,f), bulk,
              seg:RIG.poses[i].n+' \u2192 '+RIG.poses[i+1].n};
    }
    acc+=frac;
  }
  return {p:RIG.poses[RIG.poses.length-1].a, bulk:{}, seg:'settle'};
}

/* The torso turns 180 above the hips, after the body is upright and on its
 * feet -- at 0.70 it was spinning while still on all fours, which reads as a
 * stumble. Applied to `spine` alone so the legs are untouched. */
/* Stretched to finish at 1.00: over 0.15 of the timeline it peaked near 18
 * degrees a step, and anything mounted away from the spine whipped round --
 * a shoulder wheel moved 45 units in one step at t=0.88. */
const SPIN={t0:0.66,t1:1.00,deg:180};
/* TWO WAYS TO TRANSFORM, for a vehicle that has both: on the ground the
 * push-up; in the air a barrel roll, which bleeds speed and unfolds at once.
 * Both start from the same fold, so the skinning's fold binding holds for
 * either. Switching swaps the pose list and rebuilds the lift table. */
const GROUND_SEQ={p:RIG.poses, w:RIG.weights};
let AIRMODE=false, MODE='ground';
/* three ways to transform: 'ground' (push-up), 'air' (the vehicle's own: a
 * barrel roll, or a hover car's levitation), 'zerog' (levitate and spin) */
function setMode(m){
  if(m===true) m='air'; if(m===false) m='ground';
  if(m==='air' && !RIG.poses_air) m='ground';
  if(m==='zerog' && !RIG.poses_zerog) m=RIG.poses_air?'air':'ground';
  MODE=m; AIRMODE=(m!=='ground');
  RIG.poses=m==='air'?RIG.poses_air:m==='zerog'?RIG.poses_zerog:GROUND_SEQ.p;
  RIG.weights=m==='air'?RIG.weights_air:m==='zerog'?RIG.weights_zerog:GROUND_SEQ.w;
  /* The torso twist is a FULL turn. At 180 over the hips the chest ended
   * facing +x while the feet pointed -x -- backwards on its own legs, and
   * away from the camera. 360 keeps the flourish and lands it facing the way
   * the feet do. In the air the barrel roll does this job. */
  SPIN.deg=AIRMODE?0:360;
  LIFT=null;
  const b=typeof document!=='undefined'&&document.getElementById('mode');
  if(b){ b.textContent=m==='zerog'?'zero-g':m==='air'?(RIG.air_label||'air roll'):'ground'; b.classList.toggle('on',AIRMODE); b.style.display=(RIG.poses_air||RIG.poses_zerog)?'':'none'; }
}
function spinAt(t){
  const u=Math.max(0,Math.min(1,(t-SPIN.t0)/(SPIN.t1-SPIN.t0)));
  const e=u<0.5?4*u*u*u:1-Math.pow(-2*u+2,3)/2;
  return SPIN.deg*e;
}

/* A machine is stable when its centre of mass projects, in plan, inside the
 * polygon of whatever touches the ground. The sign of the margin says which
 * fix: behind means the legs are too far back, sideways means the stance is
 * too narrow. */
function analyse(sol){
  let tot=0, acc=[0,0,0];
  for(const b of RIG.bones){
    if(b.l<=0) continue;
    const r=RIG.radius[b.n]!==undefined?RIG.radius[b.n]:RIG.default_r;
    const w=b.l*r*r, s=sol[b.n];
    for(let a=0;a<3;a++) acc[a]+=w*(s.head[a]+s.tail[a])*0.5;
    tot+=w;
  }
  const com=acc.map(v=>v/tot);
  /* Contact is measured from the body's OWN lowest point, not from y=0.
   * The body is grounded on its MESH -- a foot's geometry hangs below its
   * ankle -- so every joint sits higher than the floor and a fixed threshold
   * saw nothing touching at all: prone reported 0 contacts while lying flat
   * on 44 of them. */
  let base=Infinity;
  for(const k in sol) base=Math.min(base, sol[k].head[1], sol[k].tail[1]);
  const pts=[];
  for(const k in sol) for(const p of [sol[k].head,sol[k].tail])
    if(p[1]-base<=RIG.contact_h) pts.push([p[0],p[2]]);
  const u=[...new Set(pts.map(p=>p.join(',')))].map(s=>s.split(',').map(Number))
          .sort((a,b)=>a[0]-b[0]||a[1]-b[1]);
  const half=seq=>{const o=[];for(const p of seq){
    while(o.length>=2){const A=o[o.length-2],B=o[o.length-1];
      if((B[0]-A[0])*(p[1]-A[1])-(B[1]-A[1])*(p[0]-A[0])>0)break;o.pop();}
    o.push(p);} return o;};
  const hull=u.length<3?u:half(u).slice(0,-1).concat(half([...u].reverse()).slice(0,-1));
  let margin=NaN;
  if(hull.length>=3){
    let inside=true,best=1e30;
    for(let i=0;i<hull.length;i++){
      const A=hull[i],B=hull[(i+1)%hull.length];
      const ex=B[0]-A[0], ey=B[1]-A[1];
      if(ex*(com[2]-A[1])-ey*(com[0]-A[0])<0) inside=false;
      const L=Math.hypot(ex,ey)||1e-9;
      const tt=Math.max(0,Math.min(1,((com[0]-A[0])*ex+(com[2]-A[1])*ey)/(L*L)));
      best=Math.min(best,Math.hypot(com[0]-(A[0]+ex*tt), com[2]-(A[1]+ey*tt)));
    }
    margin=inside?best:-best;
  }
  return {com,hull,margin,contacts:pts.length};
}

/* HEFT.
 *
 * Every joint has been snapping exactly to its authored angle, which is why
 * the machine reads as rigid: a 300-unit body with a 48%-of-mass leg does not
 * arrive at an angle and stop dead. So each bone's angle FOLLOWS its target
 * through a spring-damper rather than being assigned:
 *
 *     a = k*(target - x) - c*v
 *
 * Stiffness is scaled by the bone's own mass from the same length x radius^2
 * model the balance analysis uses, so a thigh (11% of the machine) lags and
 * overshoots while a hand (1%) tracks almost exactly. That coupling is the
 * point -- the heft is not a global wobble, it is heavy things behaving like
 * heavy things.
 *
 * Damping ratio chosen by SWEEP, measuring overshoot and settle time on the
 * thigh (11% of the machine) after a step from prone to standing:
 *
 *     zeta   overshoot   settles in
 *     0.86      0.02 deg    15 frames    invisible -- what it shipped as
 *     0.60      0.79        11
 *     0.45      1.7         21
 *     0.35      2.74        21           starts to read as loose
 *
 * 0.48: about 1.5 degrees of overshoot on a heavy joint, settling in a third
 * of a second. Enough to see, not enough to look broken. A joint that
 * oscillates reads as a bug; one that passes its mark once and stops reads as
 * mass.
 */
const DYN = {};
(function(){
  let tot=0;
  const w={};
  for(const b of RIG.bones){
    if(b.l<=0) continue;
    const r=RIG.radius[b.n]!==undefined?RIG.radius[b.n]:RIG.default_r;
    w[b.n]=b.l*r*r; tot+=w[b.n];
  }
  for(const b of RIG.bones){
    if(b.l<=0){ DYN[b.n]={x:[0,0,0],v:[0,0,0],k:400,c:26}; continue; }
    const share=w[b.n]/tot;                 // 0.01 .. 0.12
    const k=44/Math.max(0.012,share)*0.55;  // heavy = soft = laggy
    DYN[b.n]={x:null,v:[0,0,0],k:k,c:2*Math.sqrt(k)*0.48};
  }
})();

/* Advance the followers toward the authored pose. Sub-stepped because a stiff
 * spring at 60 Hz with a large dt goes unstable, and a joint that explodes is
 * worse than one that snaps. */
function settle(target, dt){
  const steps=Math.max(1,Math.min(6,Math.ceil(dt/0.006)));
  const h=dt/steps;
  const out={};
  for(const b of RIG.bones){
    const d=DYN[b.n];
    const tgt=target[b.n]||[0,0,0];
    if(d.x===null){ d.x=tgt.slice(); d.v=[0,0,0]; }
    for(let st=0;st<steps;st++){
      for(let i=0;i<3;i++){
        const a=d.k*(tgt[i]-d.x[i]) - d.c*d.v[i];
        d.v[i]+=a*h;
        d.x[i]+=d.v[i]*h;
      }
    }
    out[b.n]=d.x.slice();
  }
  out._pos=target._pos; out._rot=target._rot;
  return out;
}

/* ── scene ─────────────────────────────────────────────────────────────── */
const host=document.getElementById('view');
const renderer=new THREE.WebGLRenderer({antialias:true});
renderer.setPixelRatio(Math.min(devicePixelRatio,2));
host.appendChild(renderer.domElement);
const scene=new THREE.Scene(); scene.background=new THREE.Color(0x15171b);
const camera=new THREE.PerspectiveCamera(40,1,1,4000);
scene.add(new THREE.HemisphereLight(0xdfe6f2,0x20242c,1.0));
const keyL=new THREE.DirectionalLight(0xffffff,1.15); keyL.position.set(-200,400,300);
scene.add(keyL);
// a cool rim from behind: grit only reads if something grazes it
const rimL=new THREE.DirectionalLight(0xbcd2ff,0.85); rimL.position.set(260,180,-320);
scene.add(rimL);
const fillL=new THREE.DirectionalLight(0xffe6c4,0.35); fillL.position.set(160,-60,220);
scene.add(fillL);
scene.add(new THREE.GridHelper(600,12,0x39414d,0x252a32));

/* The two volumes a folded body actually has: the TUNNEL under the floor pan
 * between the frame rails, and the engine BAY. Measured from the car's own
 * parts, not from its bounding box. */
const volBoxes=[];
for(const v of RIG.volumes){
  const bx=new THREE.Box3(new THREE.Vector3(v.x[0],v.y[0],v.z[0]),
                          new THREE.Vector3(v.x[1],v.y[1],v.z[1]));
  const hh=new THREE.Box3Helper(bx, v.n==='bay'?0xb08a4d:0x4d7a62);
  hh.visible=false; scene.add(hh); volBoxes.push(hh);
}

const boneMat=new THREE.MeshStandardMaterial({color:0xe8d9a8,roughness:0.35,metalness:0.7});
const jointMat=new THREE.MeshStandardMaterial({color:0x8fb8ff,roughness:0.3,metalness:0.6});
const bMesh={}, jMesh={};
for(const b of RIG.bones){
  if(b.l<=0) continue;
  bMesh[b.n]=new THREE.Mesh(new THREE.CylinderGeometry(4,3,1,8),boneMat);
  bMesh[b.n].visible=false; scene.add(bMesh[b.n]);
  jMesh[b.n]=new THREE.Mesh(new THREE.SphereGeometry(6,10,8),jointMat);
  jMesh[b.n].visible=false; scene.add(jMesh[b.n]);
}

/* One rigid mesh per bone, in that bone's own frame, oriented along its rest
 * direction. Nothing deforms -- the benefit of fixed-length bones. */
/* METAL SHADED BY WHERE IT IS AND WHAT IT IS DOING.
 *
 * Two inputs, both already known and neither needing a texture:
 *
 *   PROXIMITY  a per-vertex weight, 1 at a bone's joints and 0 at its middle.
 *              Metal near a joint is worked metal -- rubbed bright by
 *              movement, discoloured by the heat of it -- and metal in the
 *              middle of a limb is just panel.
 *   FLEX       how fast that joint is actually moving, taken from the
 *              spring-damper velocity that already exists for the heft. A
 *              joint under load glows at its working surfaces; one at rest
 *              does not.
 *
 * Applied as vertex colour, so it costs one attribute and no texture fetch --
 * which matters on a phone. */
/* metalness 0.72 was the problem, not the vertex colours. A metal in a
 * standard shader has almost no diffuse -- it is lit ENTIRELY by what it
 * reflects, and this scene has two lights and no environment map, so the body
 * went nearly black and the grit and joint shading were invisible underneath.
 * 0.28 keeps the sheen and lets the colour actually show. */
const bodyMat=new THREE.MeshStandardMaterial({color:0xffffff,roughness:0.46,
  metalness:0.28,side:THREE.DoubleSide,vertexColors:true});
/* THE SKELETON CONDENSES OUT OF THE CORE.
 *
 * Running the interior's dissolve backwards. At t=0 the machine is a car: the
 * skeleton does not exist yet as a body, it is lattice inside the core cube,
 * so its geometry is snapped to a coarse grid and it reads as a few floating
 * blocks. Over the first half the cell shrinks and the bones resolve.
 *
 * That is the other half of "matter goes somewhere": the car's interior is
 * dissolving INTO the core across the same window that the skeleton is
 * condensing OUT of it. One is paying for the other.
 */
/* The skeleton condenses DURING the hold and is solid by the time the body
 * starts to move. It was still resolving while already standing, which read
 * as the machine assembling itself in mid-air. */
const bodyVox = (t) => Math.max(0, 1 - t / (HOLD + 0.04));
const bodyMat_hook = (sh, holder) => {
  sh.uniforms.uVox = {value: 0};
  holder.userData.sh = sh;
  sh.vertexShader = 'uniform float uVox;\n' + sh.vertexShader.replace(
    '#include <begin_vertex>',
    '#include <begin_vertex>\n' +
    'if (uVox > 0.001) {\n' +
    '  float cell = mix(1.5, 22.0, uVox*uVox);\n' +
    '  vec3 q = floor(transformed / cell + 0.5) * cell;\n' +
    '  vec3 off = transformed - q;\n' +
    '  transformed = q + mix(off, sign(off) * cell * 0.5, min(1.0, uVox*1.6));\n' +
    '}');
};
const bodyMesh={};
for(const n in BODY){
  const g=new THREE.BufferGeometry();
  g.setAttribute('position',new THREE.Float32BufferAttribute(BODY[n].v,3));
  g.setIndex(BODY[n].f); g.computeVertexNormals();
  bodyMesh[n]=new THREE.Mesh(g,bodyMat);
  bodyMesh[n].matrixAutoUpdate=false;
  scene.add(bodyMesh[n]);
}

/* HYDRAULIC MUSCLES.
 *
 * A McKibben actuator: a braided sleeve that SHORTENS AND FATTENS when
 * pressurised. Each one spans a joint -- one tendon on the parent bone, one
 * on the child -- so the pose gives its length for free, and volume
 * conservation gives its radius:
 *
 *     L = |attach_child - attach_parent|      from the pose
 *     r = r0 * sqrt(L0 / L)                   fluid has to go somewhere
 *
 * That is the whole simulation. No solver, no timestep: a contracting muscle
 * bulges because a fixed volume in a shorter tube is a fatter tube, which is
 * exactly why the real thing bulges. The pistons and coils it replaces were
 * pictures of mechanisms -- a cylinder that never changed length -- and that
 * is why the limbs read as carved.
 *
 * Drawn as a spindle, not a cylinder: thin at the tendons, fat at the belly.
 */
const musMat=new THREE.MeshStandardMaterial({color:0x8d5b52,roughness:0.62,
  metalness:0.12});
const LINK=(()=>{
  /* One actuator link: a barrel with a raised collar at each end, plus the
   * rod it slides on. Built once in unit space (0..1 along +y) and instanced
   * -- a chain of these contracts by closing the gaps between them, which is
   * what reads as hydraulic. A smooth spindle reads as rubber, and this
   * machine is not organic; it borrowed how a muscle behaves, not how one
   * looks. */
  const pos=[], idx=[], ring=8;
  const prof=[[0.00,0.30],[0.06,0.78],[0.14,0.82],[0.20,0.62],
              [0.80,0.62],[0.86,0.82],[0.94,0.78],[1.00,0.30]];
  for(const [u,r] of prof)
    for(let j=0;j<ring;j++){
      const a=Math.PI*2*j/ring;
      pos.push(Math.cos(a)*r, u, Math.sin(a)*r);
    }
  for(let i=0;i<prof.length-1;i++) for(let j=0;j<ring;j++){
    const k=(j+1)%ring, A=i*ring+j, B=i*ring+k, C=(i+1)*ring+k, D=(i+1)*ring+j;
    idx.push(A,B,C, A,C,D);
  }
  const g=new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos,3));
  g.setIndex(idx); g.computeVertexNormals();
  return g;
})();
const ROD=new THREE.CylinderGeometry(0.22,0.22,1,6);
const LINKS_PER=4;
const rodMat=new THREE.MeshStandardMaterial({color:0xb9c2cf,roughness:0.16,
  metalness:0.95});
const musMesh=(RIG.muscles||[]).map(()=>{
  const links=[];
  for(let i=0;i<LINKS_PER;i++){
    const m=new THREE.Mesh(LINK, musMat);
    m.matrixAutoUpdate=false; scene.add(m); links.push(m);
  }
  const rod=new THREE.Mesh(ROD, rodMat);
  rod.matrixAutoUpdate=false; scene.add(rod);
  return {links, rod};
});
const musRest={};
let showMus=true;

/* SCALE ARMOUR.
 *
 * Big car panels do not work as armour: a panel is a large singly-curved
 * sheet shaped to a body it no longer belongs to, no placement makes it
 * follow an arm, and cutting it into per-bone sections leaves a sawtooth seam
 * that reads as spines growing out of flat plates. So the panels reclaim into
 * the core with everything else, and the armour is GENERATED from how much
 * panel each bone was allocated.
 *
 * Scales work where plates do not: small enough to follow a curve, overlapping
 * so there is no seam to tear, and a row of them reads as one surface while
 * behaving like many. Rows are staggered half a step so they interlock rather
 * than lining up into columns with gaps between them.
 *
 * One mesh, instanced by matrix -- 325 scales cost one geometry.
 */
function makePlate(crown, taper){
  /* An armour plate whose SECTION COMES FROM THE CAR.
   *
   * `crown` is how far the donor panel bowed across its width and `taper` how
   * much it narrowed along its length, both measured off the real geometry.
   * A chest plate then inherits the bonnet's low crown and a shin plate the
   * sill's hard section, so the skin has a relationship to the vehicle
   * instead of being one generic shape repeated 508 times.
   *
   * Still a plate: keel, chamfered rim, cut leading edge. What varies is how
   * pronounced each of those is.
   */
  const pos=[], idx=[];
  const K = 0.22 + crown * 1.35;        // keel height
  const T = 0.55 + taper * 0.45;        // trailing-edge width
  const prof=[
    [-0.50, 0.26*T, 0.10],
    [-0.24, 0.46*T, 0.34],
    [ 0.06, 0.50,   0.40],
    [ 0.34, 0.47,   0.30],
    [ 0.44, 0.38,   0.16],
    [ 0.50, 0.20,   0.06],
  ];
  const RIM = 0.78 - crown*0.16;        // a domed plate has a shorter chamfer
  const ring=(y,w,k,outer)=>{
    const z=outer?1:-1;
    return [[-w, y, 0.10*z], [-w*RIM, y, (0.30+k*K*0.75)*z],
            [0, y, (0.30+k*K)*z], [w*RIM, y, (0.30+k*K*0.75)*z], [w, y, 0.10*z]];
  };
  const rows=[];
  for(const [y,w,k] of prof) rows.push(ring(y,w,k,true).map(p=>{pos.push(...p);return pos.length/3-1;}));
  for(const [y,w,k] of prof) rows.push(ring(y,w,k*0.18,false).map(p=>{pos.push(...p);return pos.length/3-1;}));
  const N=prof.length;
  const quad=(a,b,c,d)=>{idx.push(a,b,c, a,c,d);};
  for(let f=0;f<2;f++) for(let i=0;i<N-1;i++) for(let j=0;j<4;j++){
    const A=rows[f*N+i][j], B=rows[f*N+i][j+1];
    const C=rows[f*N+i+1][j+1], D=rows[f*N+i+1][j];
    if(f===0) quad(A,B,C,D); else quad(A,D,C,B);
  }
  for(let i=0;i<N-1;i++){
    quad(rows[i][0], rows[i+1][0], rows[N+i+1][0], rows[N+i][0]);
    quad(rows[N+i][4], rows[N+i+1][4], rows[i+1][4], rows[i][4]);
  }
  for(let j=0;j<4;j++){
    quad(rows[0][j+1], rows[0][j], rows[N][j], rows[N][j+1]);
    quad(rows[N-1][j], rows[N-1][j+1], rows[2*N-1][j+1], rows[2*N-1][j]);
  }
  const g=new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos,3));
  g.setIndex(idx); g.computeVertexNormals();
  return g;
}
// one geometry per bone, so a limb's plates carry its donor panel's section
const PLATE_GEO={};
for(const b in (RIG.scales||{})){
  const pr=(RIG.plate||{})[b] || {crown:0.20, taper:0.55};
  PLATE_GEO[b]=makePlate(pr.crown, pr.taper);
}
const SCALE_GEO=makePlate(0.20, 0.55);
const SCALE_SHELLS = 6;
const scaleMats = [];
for (let d = 0; d < SCALE_SHELLS; d++){
  scaleMats.push(new THREE.MeshStandardMaterial({
    color: 0x8d96a6, roughness: 0.40, metalness: 0.55,
    side: THREE.DoubleSide,
    polygonOffset: true,
    polygonOffsetFactor: -(1 + d),
    polygonOffsetUnits: -(1 + d)}));
}
const scaleMat = scaleMats[0];
const scaleMesh={};
let showScale=true, scaleCount=0;
for(const b in (RIG.scales||{})){
  scaleMesh[b]=RIG.scales[b].map((sc)=>{
    const m=new THREE.Mesh(PLATE_GEO[b]||SCALE_GEO,
                           scaleMats[(sc.d||0) % SCALE_SHELLS]);
    m.matrixAutoUpdate=false; m.visible=false; scene.add(m); scaleCount++;
    return m;
  });
}

/* GROUND ON THE MESH, with bulk as the 3-vector it now is.
 *
 * This read `(...) * k` with k the bulk. Bulk became [radial, along, radial]
 * for the rib compaction, and number times array is NaN in JavaScript -- so
 * the chest, spine and pelvis were silently dropped from grounding. Folded,
 * the chest is the lowest thing on the body, so the machine was grounded on
 * its limbs and sat at the wrong height. The fix for this line had already
 * been written once and never applied: its indentation did not match. */
function groundLift(s,bulk){
  let low=Infinity;
  for(const n in bodyMesh){
    const so=s[n]; if(!so) continue;
    const m=so.m, h=so.head, pos=BODY[n].v;
    const kv=(bulk&&bulk[n])?(Array.isArray(bulk[n])?bulk[n]:[bulk[n],bulk[n],bulk[n]]):[1,1,1];
    for(let i=0;i<pos.length;i+=3){
      const y=m[1][0]*pos[i]*kv[0]+m[1][1]*pos[i+1]*kv[1]+m[1][2]*pos[i+2]*kv[2]+h[1];
      if(y<low) low=y;
    }
  }
  return isFinite(low)?(RIG.floor-low):0;
}

/* GROUNDING OVER THE WHOLE TIMELINE, smoothed, computed once.
 *
 * Grounding frame by frame is exact and jerky: between key poses a different
 * limb takes its turn being lowest, so the lowest point jumps, and the whole
 * machine lurched to follow it -- the chest went 113 -> 204 -> 181 in a
 * tenth of the timeline. The lift is sampled everywhere, then given a ROLLING
 * MAXIMUM and then a ROLLING AVERAGE over the same window. Each averaged value
 * is a mean of maxima that each already clear the floor at this instant, so
 * the result can never let the body sink through it -- at worst it hovers a
 * little during a dip, which is the right side to be wrong on. */
/* FOLDED INSIDE THE CAR, not on the floor. Grounding put the folded robot's
 * lowest point on the floor, but a car's body rides on its wheels: the
 * Falcon's shell starts 28 up, so the robot hung 22 below it, showing between
 * the wheels. While folded it is held just clear of the shell's bottom edge,
 * and let down onto its legs over the first part of the rise. Computed from
 * each vehicle's own shell, so it holds for any car. */
// LAZY: CAR_META is declared further down the page, so reading it here at
// load time threw and took the whole script with it
let _FOLD=null, _HALF=null;
function foldLiftMax(){
  if(_FOLD===null){ let lo=Infinity;
    for(const m of CAR_META){ if(m.g.startsWith('wheel')) continue;
      lo=Math.min(lo, m.c[1]-m.r[1]+RIG.floor); }
    // a vehicle can state its body's floor: a hover car's lowest part (its
    // splitter) is far below the floor of the body the robot folds into
    if(RIG.fold_floor!=null) lo=RIG.fold_floor+RIG.floor;
    _FOLD=Math.max(0, (isFinite(lo)?lo:RIG.floor)+4-RIG.floor); }
  return _FOLD;
}
function foldLift(t){
  const F=foldLiftMax();
  if(t<=HOLD) return F;
  const u=Math.min(1,(t-HOLD)/0.14); return F*(1-u*u*(3-2*u));
}
function halfOf(g){ if(!_HALF) _HALF=Object.fromEntries(CAR_META.map(m=>[m.g,m.r])); return _HALF[g]; }

let LIFT=null;
function smoothLift(t){
  if(!LIFT){
    const N=240, W=14, raw=[];
    for(let i=0;i<=N;i++){ const {p,bulk}=poseAt(i/N); raw.push(groundLift(solve(p),bulk)); }
    const mx=raw.map((_,i)=>{ let m=-Infinity; for(let k=Math.max(0,i-W);k<=Math.min(N,i+W);k++) m=Math.max(m,raw[k]); return m; });
    const A=mx.map((_,i)=>{ let a=0,c=0; for(let k=Math.max(0,i-W);k<=Math.min(N,i+W);k++){a+=mx[k];c++;} return a/c; });
    /* PINNED WHILE FOLDED. The rolling window looks ahead, so on its own it
     * anticipated the rise and floated the still-folded body up from t=0.34
     * -- the skeleton leaving before the car, by a new route. Until HOLD the
     * lift is exactly what the folded pose needs; after it, it eases into the
     * smoothed curve over two windows. Measured: the body leaves the floor at
     * 0.433 and the worst per-step move is 6.6 (it was over 20). */
    const h=Math.round(HOLD*N);
    LIFT=A.map((v,i)=>{ if(i<=h) return raw[h];
      const f=Math.min(1,(i-h)/(2*W)), e=f*f*(3-2*f); return raw[h]+(v-raw[h])*e; });
  }
  const N=LIFT.length-1, u=Math.max(0,Math.min(1,t))*N, i=Math.min(N-1,Math.floor(u)), f=u-i;
  return LIFT[i]+(LIFT[i+1]-LIFT[i])*f+foldLift(t);
}

/* The fold's starting pose, solved once. Stream parts freeze their offset in
 * their bone's frame HERE, so afterwards they move only when the bone does. */
let REF0=null;
function ref0(){
  if(!REF0){
    const {p,bulk}=poseAt(0);
    const s0=solve(p);
    REF0={s:s0, lift:groundLift(s0,bulk)+foldLift(0)};
  }
  return REF0;
}

/* THE CAR, as cladding.
 *
 * 43 groups from the catalogue, each a rigid body that travels from where it
 * sat on the car to a bone. Positions arrive quantised to 16 bits over one
 * shared box and gzipped -- 2.0 MB instead of 11 -- and are unpacked with
 * DecompressionStream.
 *
 * Placement is resolved in the build, not here: scale derived so a group's
 * longest axis matches its bone, then the hand-authored table in
 * orion_cladding.py overrides it per group. The first pass placed every panel
 * at scale 1.0 and `shell` -- 293 units long -- landed on a chest bone 44
 * long, which piled the whole car into one lump.
 */
const CAR_B64 = "__CARB64__";
const CAR_META = __CARMETA__;
let carMeshes = {}, showCar = true;

/* ── CUBE PARTICLES: Mother Box, then Allspark ────────────────────────────
 *
 * A panel cannot be voxelised by moving its own vertices. That was the old
 * effect, and it only read as cubes on finely modelled panels: a big flat
 * panel made of a few large triangles has a few vertices to move, so it stayed
 * one large blocky shard and then collapsed all at once when those few
 * vertices shrank -- the instant disappearance at t=0.66-0.68.
 *
 * So each dissolving panel is sampled into REAL cubes, spaced to tile its
 * surface, drawn as one instanced mesh. Then:
 *
 *   APPEAR     cubes grow in place on the panel's surface; the panel mesh
 *              gives way to them once they cover it.
 *   MOTHER BOX they reorganise into a dense lattice block -- released in
 *              waves, buzzing just before they go, accelerating into their
 *              slots and flipping as they snap, glowing while they move.
 *   ALLSPARK   the block shrinks in four stages, hold then snap to half size,
 *              turning a quarter each time and folding in from its outer layer
 *              first, until it is a sixteenth of its size and winks out.
 */
/* SUBDIVIDE BIG TRIANGLES, for vehicles that use the shader compaction.
 *
 * The shader voxelises a panel by moving its own vertices, so a panel built
 * from a few large triangles has too few points to form cubes: it stayed one
 * blocky shard and popped out at the end. Splitting every triangle would cost
 * 2.3x to 7x the geometry, because most long edges belong to thin slivers,
 * which are not the problem. Only triangles that are both long AND big are
 * split, by bisecting the longest edge (x2 per split, not x4), until each is
 * about a cube face. Orion's streamed panels go from 190k to 198k triangles.
 */
/* Split only triangles that are too big to voxelise, by longest-edge
 * bisection with a shared midpoint cache, so neighbours that split the same
 * edge agree on the new vertex. */
function tessellate(P, I, maxEdge, maxArea){
  const pos=Array.from(P), out=[], mid=new Map();
  const midpoint=(a,b)=>{ const k=a<b?a*4194304+b:b*4194304+a; let m=mid.get(k);
    if(m===undefined){ m=pos.length/3; pos.push((pos[a*3]+pos[b*3])/2,(pos[a*3+1]+pos[b*3+1])/2,(pos[a*3+2]+pos[b*3+2])/2); mid.set(k,m);} return m; };
  const d2=(a,b)=>{const x=pos[a*3]-pos[b*3],y=pos[a*3+1]-pos[b*3+1],z=pos[a*3+2]-pos[b*3+2];return x*x+y*y+z*z;};
  const area=(a,b,c)=>{const ux=pos[b*3]-pos[a*3],uy=pos[b*3+1]-pos[a*3+1],uz=pos[b*3+2]-pos[a*3+2],
    vx=pos[c*3]-pos[a*3],vy=pos[c*3+1]-pos[a*3+1],vz=pos[c*3+2]-pos[a*3+2];
    const cx=uy*vz-uz*vy,cy=uz*vx-ux*vz,cz=ux*vy-uy*vx; return 0.5*Math.sqrt(cx*cx+cy*cy+cz*cz);};
  const E2=maxEdge*maxEdge, st=[];
  for(let k=0;k<I.length;k+=3) st.push(I[k],I[k+1],I[k+2]);
  let guard=0;
  while(st.length){ const c=st.pop(), b=st.pop(), a=st.pop();
    const ab=d2(a,b), bc=d2(b,c), ca=d2(c,a), mx=Math.max(ab,bc,ca);
    // only a triangle that is BOTH long and big enough to read as a slab is
    // split; thin slivers are left alone, they are not the blocky shards
    if(mx<=E2 || area(a,b,c)<=maxArea || ++guard>4e6){ out.push(a,b,c); continue; }
    if(mx===ab){ const m=midpoint(a,b); st.push(a,m,c, m,b,c); }
    else if(mx===bc){ const m=midpoint(b,c); st.push(a,b,m, a,m,c); }
    else { const m=midpoint(c,a); st.push(a,b,m, m,b,c); }
  }
  return {P:new Float32Array(pos), I:new Uint32Array(out)};
}


const carCubes = {};
const CUBE_GEO = new THREE.BoxGeometry(1, 1, 1);
const CUBE_SPACING = 7.0;     // in car units: a 458-long car
const CUBE_CAP = 1400;        // per panel, so a phone can carry it
function sm(a,b,x){ const t=Math.max(0,Math.min(1,(x-a)/(b-a))); return t*t*(3-2*t); }
function rng(seed){ return function(){ seed|=0; seed=seed+0x6D2B79F5|0;
  let t=Math.imul(seed^seed>>>15,1|seed); t=t+Math.imul(t^t>>>7,61|t)^t;
  return ((t^t>>>14)>>>0)/4294967296; }; }

function buildCubes(P, I, idx, color){
  const nt=I.length/3, cum=new Float64Array(nt); let A=0;
  for(let k=0;k<nt;k++){
    const a=I[k*3]*3, b=I[k*3+1]*3, c=I[k*3+2]*3;
    const ux=P[b]-P[a], uy=P[b+1]-P[a+1], uz=P[b+2]-P[a+2];
    const vx=P[c]-P[a], vy=P[c+1]-P[a+1], vz=P[c+2]-P[a+2];
    const cx=uy*vz-uz*vy, cy=uz*vx-ux*vz, cz=ux*vy-uy*vx;
    A+=0.5*Math.sqrt(cx*cx+cy*cy+cz*cz); cum[k]=A;
  }
  if(!(A>0)) return null;
  const N=Math.max(24, Math.min(CUBE_CAP, Math.round(A/(CUBE_SPACING*CUBE_SPACING))));
  const s=Math.sqrt(A/N)*1.02;            // big enough that they tile the surface
  const R=rng(9173+idx*101);
  const pts=new Float32Array(N*3);
  for(let i=0;i<N;i++){
    const r=R()*A; let lo=0, hi=nt-1;
    while(lo<hi){ const mid=(lo+hi)>>1; if(cum[mid]<r) lo=mid+1; else hi=mid; }
    const a=I[lo*3]*3, b=I[lo*3+1]*3, c=I[lo*3+2]*3;
    let u=R(), v=R(); if(u+v>1){ u=1-u; v=1-v; }
    for(let k=0;k<3;k++) pts[i*3+k]=P[a+k]+(P[b+k]-P[a+k])*u+(P[c+k]-P[a+k])*v;
  }
  // Slots in a dense block, assigned by sorting -- x into slabs, y into
  // columns, z into cells -- so neighbours on the panel land in neighbouring
  // slots and the swarm flows instead of crossing over itself.
  const M=Math.max(1, Math.round(Math.cbrt(N)));
  const ord=[...Array(N).keys()].sort((p,q)=>pts[p*3]-pts[q*3]);
  const per=Math.ceil(N/M), cells=[]; let mz=1;
  for(let sx=0;sx<M;sx++){
    const slab=ord.slice(sx*per,(sx+1)*per).sort((p,q)=>pts[p*3+1]-pts[q*3+1]);
    const pc=Math.ceil(slab.length/M);
    for(let sy=0;sy<M;sy++){
      const col=slab.slice(sy*pc,(sy+1)*pc).sort((p,q)=>pts[p*3+2]-pts[q*3+2]);
      mz=Math.max(mz,col.length);
      col.forEach((i,iz)=>cells.push([i,sx,sy,iz]));
    }
  }
  const slot=new Float32Array(N*3);
  for(const [i,sx,sy,iz] of cells){
    slot[i*3]=(sx-(M-1)/2)*s; slot[i*3+1]=(sy-(M-1)/2)*s; slot[i*3+2]=(iz-(mz-1)/2)*s;
  }
  const h=new Float32Array(N), rr=new Float32Array(N), rn=new Float32Array(N);
  let rmax=1e-6, pmax=1e-6;
  for(let i=0;i<N;i++){ h[i]=R(); rr[i]=Math.hypot(slot[i*3],slot[i*3+1],slot[i*3+2]); rmax=Math.max(rmax,rr[i]);
    rn[i]=Math.hypot(pts[i*3],pts[i*3+1],pts[i*3+2]); pmax=Math.max(pmax,rn[i]); }
  for(let i=0;i<N;i++){ rr[i]/=rmax; rn[i]/=pmax; }
  const m=new THREE.MeshStandardMaterial({color, metalness:0.55, roughness:0.35,
    emissive:0x6fd0ff, emissiveIntensity:0});
  const im=new THREE.InstancedMesh(CUBE_GEO, m, N);
  im.matrixAutoUpdate=false; im.visible=false; im.frustumCulled=false;
  scene.add(im);
  let half=0; for(let i=0;i<N*3;i++) half=Math.max(half,Math.abs(slot[i]));
  return {im, N, s, pts, slot, h, rr, rn, half:half+s*0.5};
}

const _m4=new THREE.Matrix4(), _q=new THREE.Quaternion(), _qy=new THREE.Quaternion(),
      _v=new THREE.Vector3(), _sv=new THREE.Vector3(), _ax=new THREE.Vector3(),
      _Y=new THREE.Vector3(0,1,0), _one=new THREE.Vector3(1,1,1);
function updateCubes(C, u, pos, qt, now, anchor, seat, kscale){
  /* A KEPT panel compacts into cube armour and stays: every cube, on its own
   * staggered clock, flies from the panel into a dense block seated on the
   * body -- no decay, no fade. The shell's matter is conserved as armour
   * instead of being eaten away. */
  if(seat){
    C.im.visible = u>0.03 && showCar;
    if(!C.im.visible) return;
    C.im.matrix.compose(pos, qt, _one);
    _v.copy(seat).sub(pos).applyQuaternion(_qy.copy(qt).invert());
    const sx=_v.x, sy=_v.y, sz=_v.z;
    C.im.material.emissiveIntensity=0.22*sm(0.12,0.20,u)*(1-sm(0.70,0.82,u));
    for(let i=0;i<C.N;i++){
      const h=C.h[i], i3=i*3;
      const a=sm(0.04+h*0.06, 0.08+h*0.06, u);
      const ord=Math.max(0,Math.min(1,(1-C.rn[i])*0.72+h*0.28));
      const rel=0.10+ord*0.62;                     // edges first, long stagger
      const pA=Math.max(0,Math.min(1,(u-rel)/0.10)), eA=pA*pA*pA;
      let x=C.pts[i3]+(sx+C.slot[i3]*kscale-C.pts[i3])*eA;
      let y=C.pts[i3+1]+(sy+C.slot[i3+1]*kscale-C.pts[i3+1])*eA;
      let z=C.pts[i3+2]+(sz+C.slot[i3+2]*kscale-C.pts[i3+2])*eA;
      if(u<rel && u>rel-0.035){
        const w=Math.sin(now*70+h*50)*C.s*0.3*((u-(rel-0.035))/0.035);
        x+=w*(h-0.5)*2; y+=w*(((h*7.3)%1)-0.5)*2; z+=w*(((h*3.7)%1)-0.5)*2;
      }
      _v.set(x,y,z);
      _ax.set(h-0.5, ((h*7.3)%1)-0.5, ((h*3.7)%1)-0.5).normalize();
      _q.setFromAxisAngle(_ax, eA*Math.PI/2);
      // denser as it lands: the block is the panel's matter, packed
      const k=C.s*a*(1-(1-kscale)*eA)*0.96;
      _m4.compose(_v, _q, _sv.set(k,k,k));
      C.im.setMatrixAt(i,_m4);
    }
    C.im.instanceMatrix.needsUpdate=true;
    return;
  }
  const on = u>0.03 && u<0.999;
  C.im.visible = on && showCar;
  if(!C.im.visible) return;
  C.im.matrix.compose(pos, qt, _one);
  /* the torso point this panel has been following, in the panel's own space:
   * the block drifts into it as it decays, so it ends inside the body. It used
   * to shrink where the panel sat -- the roof's block finished ~80 units above
   * the chest, in mid-air. */
  let ax=0, ay=0, az=0;
  if(anchor){ _v.copy(anchor).sub(pos).applyQuaternion(_qy.copy(qt).invert());
    ax=_v.x; ay=_v.y; az=_v.z; }
  // faint, only while the swarm is moving: it was far too bright
  C.im.material.emissiveIntensity=0.22*sm(0.12,0.20,u)*(1-sm(0.48,0.58,u));
  const fade=1-sm(0.94,1.0,u);
  const KD=3.7, DEND=1-Math.exp(-KD);
  for(let i=0;i<C.N;i++){
    const h=C.h[i], i3=i*3;
    const a=sm(0.04+h*0.06, 0.08+h*0.06, u);
    /* EVERY CUBE ON ITS OWN CLOCK. Cubes went in six waves and then the
     * whole block shrank on one shared clock -- a global slide. Now each cube
     * peels off at its own moment, from the panel's edges inward with some
     * noise, across most of the window; snaps into the block; and starts its
     * own decay and drift into the body the moment it lands. At any instant
     * some cubes are still on the panel, some in flight, some already
     * shrinking into the body. */
    const ord=Math.max(0,Math.min(1,(1-C.rn[i])*0.72+h*0.28));
    const rel=0.10+ord*0.52;                       // release, edges first
    const pA=Math.max(0,Math.min(1,(u-rel)/0.07)), eA=pA*pA*pA;
    let x=C.pts[i3]+(C.slot[i3]-C.pts[i3])*eA;
    let y=C.pts[i3+1]+(C.slot[i3+1]-C.pts[i3+1])*eA;
    let z=C.pts[i3+2]+(C.slot[i3+2]-C.pts[i3+2])*eA;
    if(u<rel && u>rel-0.035){
      const w=Math.sin(now*70+h*50)*C.s*0.3*((u-(rel-0.035))/0.035);
      x+=w*(h-0.5)*2; y+=w*(((h*7.3)%1)-0.5)*2; z+=w*(((h*3.7)%1)-0.5)*2;
    }
    // its own decay, starting when it lands: fast in, log tail, tiny out
    const ci=Math.max(0,Math.min(1,(u-(rel+0.07))/0.28));
    const S=Math.exp(-KD*ci);
    const ang=(Math.PI/4)*(KD*ci/Math.LN2);
    const d=(1-S)/DEND;
    const ca=Math.cos(ang), sa=Math.sin(ang);
    _v.set((x*ca+z*sa)*S+ax*d, y*S+ay*d, (-x*sa+z*ca)*S+az*d);
    _ax.set(h-0.5, ((h*7.3)%1)-0.5, ((h*3.7)%1)-0.5).normalize();
    _q.setFromAxisAngle(_ax, eA*Math.PI/2);
    _qy.setFromAxisAngle(_Y, ang); _q.premultiply(_qy);
    const k=C.s*a*S*fade*0.96;
    _m4.compose(_v, _q, _sv.set(k,k,k));
    C.im.setMatrixAt(i,_m4);
  }
  C.im.instanceMatrix.needsUpdate=true;
}
const IDENTQ = new THREE.Quaternion();
/* The car's wheels sit at y=0 in its own coordinates; the rig is drawn with
 * its floor at RIG.floor. One constant reconciles the two spaces. */
const CARLIFT = RIG.floor;

async function loadCar(){
  const bin = Uint8Array.from(atob(CAR_B64), c => c.charCodeAt(0));
  if (typeof DecompressionStream !== 'function')
    throw new Error('this browser has no DecompressionStream');
  const raw = new Uint8Array(await new Response(
    new Blob([bin]).stream().pipeThrough(new DecompressionStream('gzip'))
  ).arrayBuffer());
  const dv = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
  const n = dv.getUint32(0, true);
  const lo = [dv.getFloat32(4,true), dv.getFloat32(8,true), dv.getFloat32(12,true)];
  const sc = [dv.getFloat32(16,true), dv.getFloat32(20,true), dv.getFloat32(24,true)];
  let off = 28;
  /* The car material carries the DISSOLVE. Interior parts snap to a lattice
   * whose cell grows over the transform: smooth panel, then blocky, then a
   * handful of cubes, then gone. Matter being reclaimed by the core rather
   * than a panel quietly scaling to zero, which reads as a bug.
   *
   * Done in the vertex shader on geometry that already exists, so it costs
   * one uniform and nothing else. */
  /* polygonOffset per layer. 67 panels stacked four deep put coplanar faces
   * at the same depth, and the depth test then picks a different winner from
   * frame to frame -- which reads as parts flipping between two states. A
   * bias per layer breaks the tie deterministically. */
  const mat = new THREE.MeshStandardMaterial({color:0x6f7684,roughness:0.34,
    metalness:0.62,side:THREE.DoubleSide,
    polygonOffset:true, polygonOffsetFactor:0, polygonOffsetUnits:0});
  const voxHook = (sh, holder) => {
    sh.uniforms.uVox = {value: 0};
    sh.uniforms.uTime = {value: 0};
    holder.userData.sh = sh;
    /* MAGNETIC COMPACTION.
     *
     * The first version snapped vertices to a lattice whose cell grew, which
     * turned a panel blocky and then coarse -- correct in outline and inert to
     * look at: cubes that only got bigger. What it was missing is that the
     * matter has to GO somewhere and be seen going.
     *
     * Four things, all in the vertex shader on geometry that already exists:
     *
     *   RESOLUTION  the cell starts fine (2 units, not 1) and ends at 26
     *               rather than 46 -- more, smaller cubes for longer, which is
     *               what makes it read as granular rather than as a panel
     *               turning into four boxes.
     *   DEPTH       each cube is pushed out along its own face normal, so it
     *               has thickness instead of being a snapped sheet. A flat
     *               panel quantised is still flat; these are solid.
     *   JITTER      per-cube noise on position and a small rotation, rising
     *               then falling. Cubes vibrate as they are pulled in and go
     *               still as they arrive.
     *   MAGNETISM   every cube is drawn toward the panel's own centre by an
     *               amount that grows through the dissolve, so they close the
     *               gaps and PHASE INTO each other -- the volume is eaten
     *               rather than faded.
     */
    sh.uniforms.uStream = {value: 0};
    sh.uniforms.uMorph = {value: 0}; sh.uniforms.uMorphT = {value: 0};
    sh.uniforms.uAxL = {value: new THREE.Vector3(1,0,0)}; sh.uniforms.uAxM = {value: new THREE.Vector3(0,1,0)};
    sh.uniforms.uAxT = {value: new THREE.Vector3(0,0,1)}; sh.uniforms.uHalf = {value: new THREE.Vector3(1,1,1)};
    sh.uniforms.uBQ = {value: Array.from({length:22}, ()=>new THREE.Vector4(0,0,0,1))};
    sh.uniforms.uBT = {value: Array.from({length:22}, ()=>new THREE.Vector4(0,0,0,0))};
    sh.uniforms.uPull = {value: new THREE.Vector3()};
    /* STREAM MODE -- the body comes apart in place, like a supermagnet.
     *
     * The first version faded: bits shrank as they travelled and moved on a
     * smooth ease-in-out, so the panel thinned away. Magnets are the opposite
     * -- stick, stick, SNAP -- so this is two violent acts instead of one
     * gentle one. See the shader for the details; the earlier notes below
     * still hold for why the grid is fixed.
     *
     * The compaction above grows its cell as it runs, which is right for a
     * part collapsing on itself and wrong for this: a cube cannot be SEEN
     * travelling if the grid under it keeps changing, because it is a
     * different cube every frame. Here the grid is fixed and small, so each
     * bit keeps its identity from the moment it trembles loose to the moment
     * it lands.
     *
     *   TREMBLE   every bit shakes before it goes, harder the closer it is to
     *             leaving -- the magnetic pull taking hold.
     *   STAGGER   each bit leaves at its own moment (hashed from where it
     *             sat), so the panel erodes rather than lifting off whole.
     *   STREAM    it arcs to the core and shrinks as it travels, so bits
     *             arriving together overlap and phase into each other instead
     *             of piling up.
     */
    sh.vertexShader =
      'uniform float uVox;\nuniform float uTime;\nuniform float uStream;\nuniform vec3 uPull;\nattribute vec3 aCell;\n' +
      'attribute float aBone;\nattribute vec3 aAnchor;\nuniform vec4 uBQ[22];\nuniform vec4 uBT[22];\n' +
      'uniform float uMorph;\nuniform float uMorphT;\nuniform vec3 uAxL;\nuniform vec3 uAxM;\nuniform vec3 uAxT;\nuniform vec3 uHalf;\n' +
      'float vhash(vec3 p){ return fract(sin(dot(p, vec3(127.1,311.7,74.7)))*43758.5453); }\n' +
      sh.vertexShader.replace(
      '#include <begin_vertex>',
      '#include <begin_vertex>\n' +
      /* RESHAPE AS IT ATTACHES. A shell part coming through as armour is
       * morphed in its own axes (long L, middle M, thin T) as it rides to
       * its seat -- a spike tapers to a point along L and stretches; a trim
       * piece narrows to a bar -- so it transforms instead of arriving as a
       * car panel. uMorphT is the attach progress. */
      'if (uMorph > 0.5) {\n' +
      '  float mL = dot(transformed, uAxL), mM = dot(transformed, uAxM), mT = dot(transformed, uAxT);\n' +
      '  float ms = clamp(mL / max(uHalf.x, 0.001) * 0.5 + 0.5, 0.0, 1.0);\n' +
      '  float mk = uMorphT * uMorphT * (3.0 - 2.0 * uMorphT);\n' +
      '  if (uMorph < 1.5) {\n' +
      '    float taper = pow(1.0 - ms, 1.25);\n' +
      '    mM *= mix(1.0, 0.12 + 0.88 * taper, mk);\n' +
      '    mT *= mix(1.0, 0.40 + 0.60 * taper, mk);\n' +
      '    mL *= mix(1.0, 1.20, mk);\n' +
      '  } else {\n' +
      '    mM *= mix(1.0, 0.22, mk);\n' +
      '    mT *= mix(1.0, 0.35, mk);\n' +
      '  }\n' +
      '  transformed = uAxL * mL + uAxM * mM + uAxT * mT;\n' +
      '}\n' +
      'if (uStream > 0.5) {\n' +
      ' if (uVox > 0.001) {\n' +
      /* ONE CUBE PER TRIANGLE. Each corner used to follow the cube it was
       * in, but a triangle's corners often sit in different cubes: as those
       * shrank apart, every straddling triangle stretched across the gap as a
       * flat sheet. The shell ballooned to 3x its surface mid-dissolve and
       * still had 92% of it when it was hidden -- the sharp cut. aCell is the
       * triangle's own centre, so all three corners move with one cube. */
      '  float cell = 5.0;\n' +
      '  vec3 q = floor(aCell / cell + 0.5) * cell;\n' +
      '  vec3 off = transformed - q;\n' +
      '  float h = vhash(q);\n' +
      '  float CS = 16.0;\n' +
      '  vec3 qc = floor(q / CS + 0.5) * CS;\n' +
      '  float hc = vhash(qc + 5.3);\n' +
      '  float dA = floor(h * 5.0) / 5.0 * 0.26;\n' +
      '  float pA = clamp((uVox - dA) / 0.07, 0.0, 1.0);\n' +
      '  float eA = pA * pA * pA;\n' +
      // gentler crazing: seams, not holes, so the panel stays solid longer
      '  vec3 chunk = qc + (q - qc) * 0.82;\n' +
      /* ACT TWO: THE BODY IS THE MAGNET. Each chunk snaps onto the bone it
       * is bound to, accelerating as it closes, and ends at 28% of its
       * offset -- on or inside the limb. It used to pack toward fixed points
       * in CAR space, so the skin carried a halo of fragments hovering 30-80
       * units off the standing robot. Released earlier, too, so fragments
       * land as the body starts to rise rather than hovering while it stands. */
      '  float dB = 0.18 + hc * 0.22;\n' +
      '  float pB = clamp((uVox - dB) / 0.09, 0.0, 1.0);\n' +
      '  float eB = pB * pB * pB;\n' +
      '  vec3 brick = aAnchor + (q - aAnchor) * 0.28;\n' +
      '  vec3 posA = mix(q, chunk, eA);\n' +
      '  vec3 posB = mix(posA, brick, eB);\n' +
      '  float buzzA = smoothstep(dA - 0.05, dA, uVox) * (1.0 - step(dA, uVox));\n' +
      '  float buzzB = smoothstep(dB - 0.05, dB, uVox) * (1.0 - step(dB, uVox));\n' +
      '  vec3 n = vec3(vhash(q + 3.1), vhash(q + 7.7), vhash(q + 13.3)) - 0.5;\n' +
      '  vec3 jit = n * cell * 0.7 * max(buzzA, buzzB) * sin(uTime * 70.0 + h * 50.0);\n' +
      '  float tA = uVox - (dA + 0.07);\n' +
      '  float tB = uVox - (dB + 0.10);\n' +
      '  vec3 travA = normalize(chunk - q + vec3(0.0001));\n' +
      '  vec3 travB = normalize(brick - posA + vec3(0.0001));\n' +
      '  float kA = step(0.0, tA) * exp(-max(tA, 0.0) * 40.0) * sin(max(tA, 0.0) * 90.0);\n' +
      '  float kB = step(0.0, tB) * exp(-max(tB, 0.0) * 40.0) * sin(max(tB, 0.0) * 90.0);\n' +
      '  vec3 kick = travA * cell * 0.5 * kA + travB * cell * 0.8 * kB;\n' +
      // the decay clock comes first now: the corner snap waits for it
      '  float sC = 0.46 + h * 0.34;\n' +
      '  float cD = clamp((uVox - sC) / 0.20, 0.0, 1.0);\n' +
      /* SNAP LATE. With its own cube, a triangle snapped onto that cube's
       * corners at once covers only a sliver of it, and the shell looked
       * eaten away by u=0.25. It keeps its real shape while crazing and
       * compacting, moving rigidly with its cube, and only snaps to corners
       * as it starts to shrink. */
      '  vec3 cube = mix(off, sign(off) * cell * 0.48, min(1.0, cD * 3.0));\n' +
      '  float lv = floor(min(cD * 4.0, 3.999));\n' +
      '  float fl = cD * 4.0 - lv;\n' +
      '  float halvings = lv + smoothstep(0.62, 1.0, fl);\n' +
      '  float shrink = pow(0.5, halvings) * (1.0 - smoothstep(0.93, 1.0, cD));\n' +
      '  float scl = cell * pow(0.5, lv);\n' +
      '  vec3 sub = (floor(off / scl) + 0.5) * scl;\n' +
      '  vec3 subCorner = sub + sign(off - sub + vec3(0.0001)) * scl * 0.48;\n' +
      '  vec3 piece = mix(cube, subCorner, min(1.0, lv));\n' +
      '  vec3 ax = normalize(n + vec3(0.0001, 0.0, 0.0));\n' +
      /* rotate only as the triangle becomes a small cube. Whole triangles
       * flipped about their cube centre: a long sliver spun half a turn and
       * stuck out as a spike -- the hairy look on a real GPU. */
      '  float snapAmt = min(1.0, cD * 3.0);\n' +
      '  float ang = snapAmt * (1.5708 * (eA + eB) + 0.8 * halvings);\n' +
      '  vec3 rc = piece * cos(ang) + cross(ax, piece) * sin(ang)\n' +
      '          + ax * dot(ax, piece) * (1.0 - cos(ang));\n' +
      '  transformed = posB + jit + kick + rc * shrink;\n' +
      ' }\n' +
      /* SKIN. Each fragment rides the bone nearest to it in the folded pose.
       * One torso point per PANEL left the shell as a car-shaped cloud hanging
       * off the standing robot, still at the car's orientation. Per fragment,
       * nothing is far from its bone, so nothing whips; and as it compacts it
       * is drawn onto that bone, so the shell settles onto the body. */

      ' int bi = int(aBone + 0.5);\n' +
      ' vec4 bq = uBQ[bi];\n' +
      ' transformed = transformed + 2.0 * cross(bq.xyz, cross(bq.xyz, transformed) + bq.w * transformed) + uBT[bi].xyz;\n' +
      '} else if (uVox > 0.001) {\n' +
      '  float cell = mix(2.0, 26.0, uVox*uVox);\n' +
      '  vec3 q = floor(transformed / cell + 0.5) * cell;\n' +
      '  vec3 off = transformed - q;\n' +
      '  vec3 solid = sign(off) * cell * 0.5;\n' +
      '  vec3 base = q + mix(off, solid, min(1.0, uVox*1.6));\n' +
      '  float shake = uVox * (1.0 - uVox) * 4.0;\n' +
      '  vec3 n = vec3(vhash(q), vhash(q+11.3), vhash(q+27.7)) - 0.5;\n' +
      '  base += n * cell * 0.45 * shake * (0.6 + 0.4*sin(uTime*11.0 + n.x*20.0));\n' +
      '  base = mix(base, base * (1.0 - 0.55*uVox*uVox), 1.0);\n' +
      '  transformed = base;\n' +
      '}');
  };
  for (const m of CAR_META){
    const P = new Float32Array(m.nv*3);
    for (let k=0;k<m.nv*3;k++){
      P[k] = dv.getUint16(off + k*2, true)/65535*sc[k%3] + lo[k%3];
    }
    off += m.nv*3*2;
    const I = new Uint32Array(m.nt*3);
    for (let k=0;k<m.nt*3;k++) I[k] = dv.getUint32(off + k*4, true);
    off += m.nt*3*4;
    // centre the group on its own middle so the placement offset means
    // something -- otherwise every panel is positioned by its car coordinates
    const c = m.c;
    for (let k=0;k<P.length;k+=3){ P[k]-=c[0]; P[k+1]-=c[1]; P[k+2]-=c[2]; }
    let g;
    if (RIG.car.cubes===false && RIG.car.place[m.g] && RIG.car.place[m.g].st){
      /* subdivide, then UNSHARE: every triangle gets its own three corners,
       * each carrying the triangle's centre, so the shader moves the whole
       * triangle with one cube. Normals are taken from the shared mesh first
       * so the car still looks smooth before it starts to dissolve. */
      // split ANY triangle with an edge over 10, whatever its area: long thin
      // slivers moving as one piece were the spikes (the worst was 223 long)
      const T=tessellate(P, I, 10.0, 0.0);
      const gi=new THREE.BufferGeometry();
      gi.setAttribute('position', new THREE.Float32BufferAttribute(T.P,3));
      gi.setIndex(Array.from(T.I)); gi.computeVertexNormals();
      const N=gi.attributes.normal.array, nt=T.I.length/3;
      const fp=new Float32Array(nt*9), fn=new Float32Array(nt*9), fc=new Float32Array(nt*9);
      const fb=new Float32Array(nt*3), fa=new Float32Array(nt*9);
      const R0=ref0(), plc=RIG.car.place[m.g];
      const vx=plc.car[0], vy=plc.car[1]+RIG.floor, vz=plc.car[2];
      const segs=RIG.bones.map((B,bi)=>{ const J=R0.s[B.n];
        return J ? [bi, J.head[0], J.head[1]+R0.lift, J.head[2], J.tail[0], J.tail[1]+R0.lift, J.tail[2]] : null; })
        .filter(Boolean);
      for(let t=0;t<nt;t++){
        const ia=T.I[t*3]*3, ib=T.I[t*3+1]*3, ic=T.I[t*3+2]*3;
        const cx=(T.P[ia]+T.P[ib]+T.P[ic])/3, cy=(T.P[ia+1]+T.P[ib+1]+T.P[ic+1])/3,
              cz=(T.P[ia+2]+T.P[ib+2]+T.P[ic+2])/3;
        // nearest bone to this triangle, in the folded pose, and the point on it
        const wx=cx+vx, wy=cy+vy, wz=cz+vz;
        let bb=0, bd=Infinity, ax=0, ay=0, az=0;
        for(const [bi,hx,hy,hz,tx,ty,tz] of segs){
          const dx=tx-hx, dy=ty-hy, dz=tz-hz, L2=dx*dx+dy*dy+dz*dz;
          const f=L2>1e-9?Math.max(0,Math.min(1,((wx-hx)*dx+(wy-hy)*dy+(wz-hz)*dz)/L2)):0;
          const px=hx+dx*f, py=hy+dy*f, pz=hz+dz*f;
          const d=(wx-px)**2+(wy-py)**2+(wz-pz)**2;
          if(d<bd){ bd=d; bb=bi; ax=px-vx; ay=py-vy; az=pz-vz; }
        }
        [ia,ib,ic].forEach((v,j)=>{ const o=t*9+j*3;
          fp[o]=T.P[v]; fp[o+1]=T.P[v+1]; fp[o+2]=T.P[v+2];
          fn[o]=N[v]; fn[o+1]=N[v+1]; fn[o+2]=N[v+2];
          fc[o]=cx; fc[o+1]=cy; fc[o+2]=cz;
          fb[t*3+j]=bb; fa[o]=ax; fa[o+1]=ay; fa[o+2]=az; });
      }
      gi.dispose();
      g=new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.Float32BufferAttribute(fp,3));
      g.setAttribute('normal', new THREE.Float32BufferAttribute(fn,3));
      g.setAttribute('aCell', new THREE.Float32BufferAttribute(fc,3));
      g.setAttribute('aBone', new THREE.Float32BufferAttribute(fb,1));
      g.setAttribute('aAnchor', new THREE.Float32BufferAttribute(fa,3));
    } else {
      g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.Float32BufferAttribute(P,3));
      g.setIndex(Array.from(I));
      g.computeVertexNormals();
    }
    // each group needs its own material instance, or one dissolve uniform
    // would drive every panel in the car
    // its own material instance, or one dissolve uniform would drive the
    // whole car
    const gm = mat.clone();
    const _lay = (RIG.car.place[m.g] && RIG.car.place[m.g].lay) || 0;
    gm.polygonOffsetFactor = -1 - _lay;
    gm.polygonOffsetUnits = -1 - _lay;
    gm.onBeforeCompile = (sh) => voxHook(sh, gm);
    const mesh = new THREE.Mesh(g, gm);
    mesh.matrixAutoUpdate = false;
    scene.add(mesh);
    carMeshes[m.g] = mesh;
    if (RIG.car.cubes!==false && RIG.car.place[m.g] && RIG.car.place[m.g].st){
      const C = buildCubes(P, I, CAR_META.indexOf(m), 0x7d8794);
      if (C) carCubes[m.g] = C;
    }
  }
}

/* THE CORE. `compact.rs`: "matter goes somewhere on the way, and the
 * somewhere is a lattice." The cube ABSORBS -- as the machine folds down it
 * swallows shards off the skeleton, and that is what frees the volume for the
 * car's panels. At t=0 (prone, inside the car) it is full and large; at t=1
 * it has given everything back. The shards orbit inward rather than the cube
 * simply scaling, because a cube that only changes size reads as a bug. */
const coreMat=new THREE.MeshStandardMaterial({color:0xffe9a8,roughness:0.18,
  metalness:0.45,emissive:0x6a5410});
const shardMat=new THREE.MeshStandardMaterial({color:0xf0d488,roughness:0.25,
  metalness:0.5,emissive:0x3a2c08});
const coreGeo=new THREE.BufferGeometry();
coreGeo.setAttribute('position',new THREE.Float32BufferAttribute(RIG.core.cube.v,3));
coreGeo.setIndex(RIG.core.cube.f); coreGeo.computeVertexNormals();
const coreMesh=new THREE.Mesh(coreGeo,coreMat); scene.add(coreMesh);
const shardMeshes=RIG.core.shards.map(()=>{
  const mm=new THREE.Mesh(coreGeo,shardMat); scene.add(mm); return mm;});
let showCore=true;

const comMesh=new THREE.Mesh(new THREE.SphereGeometry(10,12,10),
  new THREE.MeshStandardMaterial({color:0x7fc79a,emissive:0x143020}));
scene.add(comMesh);
const hullLine=new THREE.Line(new THREE.BufferGeometry(),
  new THREE.LineBasicMaterial({color:0x7fc79a}));
scene.add(hullLine);

const UP=new THREE.Vector3(0,1,0), A=new THREE.Vector3(), B=new THREE.Vector3(),
      M=new THREE.Vector3(), Q=new THREE.Quaternion();
function place(mesh,h,t){
  A.set(h[0],h[1],h[2]); B.set(t[0],t[1],t[2]);
  const L=A.distanceTo(B);
  if(L<1e-6){mesh.visible=false;return;}
  M.addVectors(A,B).multiplyScalar(0.5); mesh.position.copy(M);
  Q.setFromUnitVectors(UP,B.clone().sub(A).normalize()); mesh.quaternion.copy(Q);
  mesh.scale.set(1,L,1);
}

let yaw=0.7,pitch=0.22,spinning=true,playing=false;
let dynOn=true, lastDt=1/60, lastT=performance.now();
/* FRAMING. The camera used a fixed distance and a fixed look-at height, which
 * was tuned when the machine was a bare skeleton ~300 tall. With the car on it
 * the content is half again as big and sat below the bottom of the frame.
 *
 * So the camera frames what is ACTUALLY VISIBLE: bounds are accumulated each
 * refresh from whichever layers are switched on, and the distance comes from
 * the vertical extent and the field of view. Toggling the car off now pulls
 * in rather than leaving a small figure in a big empty frame. */
let focus=new THREE.Vector3(0,110,0), frameR=320, userZoom=1.0;
const BB=new THREE.Box3();
let showBody=true, showBones=false, showVol=false;
function resize(){
  const w=host.clientWidth||600, h=host.clientHeight||400;
  renderer.setSize(w,h,false);
  camera.aspect=w/Math.max(1,h);
  camera.updateProjectionMatrix();
}
addEventListener('resize',resize);
// The container can still be measuring at first paint inside an iframe, so
// re-measure on the next frame and whenever the box changes.
if (typeof ResizeObserver === 'function') new ResizeObserver(resize).observe(host);
requestAnimationFrame(resize);
let drag=null;
renderer.domElement.addEventListener('pointerdown',e=>{drag={x:e.clientX,y:e.clientY};
  spinning=false;document.getElementById('spin').classList.remove('on');});
renderer.domElement.addEventListener('pointermove',e=>{if(!drag)return;
  // Drag right, the model turns right. The camera orbits the OPPOSITE way
  // from the hand, so yaw subtracts -- with a plus the model ran away from
  // the cursor, which is the inversion.
  yaw-=(e.clientX-drag.x)*0.008;pitch+=(e.clientY-drag.y)*0.006;
  pitch=Math.max(-0.2,Math.min(1.2,pitch));drag={x:e.clientX,y:e.clientY};});
addEventListener('pointerup',()=>drag=null);
renderer.domElement.addEventListener('wheel',e=>{e.preventDefault();
  userZoom*=(1+Math.sign(e.deltaY)*0.1);
  userZoom=Math.max(0.3,Math.min(4.0,userZoom));},{passive:false});

const tIn=document.getElementById('t');
/* ══ MOTION CORE ══════════════════════════════════════════════════════════
 * Headless: (clip, time, physical parameters, standing pose) -> pose offsets,
 * airborne height, travel and effects. Knows nothing about the scene, so it
 * runs the same in node. Every number is DERIVED -- from gravity, leg length,
 * mass and thrust -- and reported back, so the page can show the loop.
 *
 * Units: 1 unit ~ 1 cm (a 0.65 m wheel is ~64 units), so g = 981 u/s^2.
 * Conventions, measured on the rig (not assumed):
 *   the robot faces -x;  thigh +z swings the knee BACK;  shin +z bends the
 *   knee;  foot +z points the toes down;  spine +z leans forward, +y twists;
 *   upper arm +z swings the arm back/up;  forearm +z bends the elbow.
 *   The right side mirrors the left as (-x, -y, +z).
 */
const MOTION=(()=>{
  const TAU=Math.PI*2, sm=x=>x<=0?0:x>=1?1:x*x*(3-2*x);
  const P={g:981, L:185, mass:1.0, thrust:0.0, legF:2.6, gait:0.0,
           // damage deltas, for blackstar-rs's damage system to set
           limp:{side:'l', amount:0}};
  const add=(p,k,d)=>{ const a=(p[k]||[0,0,0]).slice(); p[k]=[a[0]+d[0],a[1]+d[1],a[2]+d[2]]; };
  const both=(p,k,d)=>{ add(p,k+'_l',d); add(p,k+'_r',[-d[0],-d[1],d[2]]); };
  const cp=o=>JSON.parse(JSON.stringify(o));

  /* GAIT from the Froude number Fr = v^2/(gL): walking tops out near 0.5,
   * running sits around 1-3 for any legged animal, so speed scales with
   * sqrt(gL). NO-SLIP STANCE: while a foot is planted the body travels
   * v * duty * cycle = 2 * duty * step, and the foot sweeps 2 L sin(hipA)
   * under it, so hipA = asin(duty * step / L). (Matching the sweep to ONE
   * step, as first written, left the planted foot sliding at 75% of walking
   * speed -- the body outran its own feet.) */
  function gaitInfo(Fr, duty){
    const v=Math.sqrt(Fr*P.g*P.L), step=P.L*(Fr<0.5?0.75:1.25), cadence=v/step;
    const hipA=Math.asin(Math.min(0.9,duty*step/P.L))*180/Math.PI;
    const cycle=2/cadence, flight=Math.max(0,cycle*(0.5-duty));
    return {v, step, cadence, hipA, cycle, duty, flight, hop:P.g*flight*flight/8};
  }
  /* FOOT IK. Driving the legs in angle space and letting grounding pick the
   * LOWEST foot as the planted one dragged the "planted" ankle forward at 78%
   * of walking speed: nothing tied that choice to the stance phase. Now each
   * ankle has a TARGET, held still in the world while planted and carried on
   * an arc to its next landing, and the hip and knee are solved to reach it.
   * The solver is the rig's own, handed in (the core stays headless): a
   * two-variable Newton solve on thigh z and shin z by finite differences. */
  /* ANALYTIC two-bone IK in the leg's side-on (x,y) plane. A Newton solve by
   * finite differences flipped to the wrong branch mid-stance (the foot jumped
   * from 50 ahead of the pelvis to 153 behind); the law of cosines, always
   * taking the knee-FORWARD solution, cannot. Applied as CHANGES from the
   * standing leg's own angles, measured with the same formula, so the rig's
   * base pose is respected. Angles: from straight down, +x (backward) positive
   * -- the rig's thigh +z and shin +z both rotate that way (measured). */
  const DEG=180/Math.PI;
  function legAngles(tx, ty, a, b){
    const d=Math.min(Math.hypot(tx,ty), (a+b)*0.999);
    const gam=Math.acos(Math.max(-1,Math.min(1,(a*a+b*b-d*d)/(2*a*b))));
    const alp=Math.acos(Math.max(-1,Math.min(1,(a*a+d*d-b*b)/(2*a*d))));
    const thT=Math.atan2(tx,-ty);
    return {thigh:(thT-alp)*DEG, bend:(Math.PI-gam)*DEG};
  }
  function ikLeg(p, side, dx, dy, s0){
    const H=s0['thigh_'+side].head, K=s0['shin_'+side].head, A=s0['foot_'+side].head;
    const a=Math.hypot(K[0]-H[0],K[1]-H[1],K[2]-H[2]), b=Math.hypot(A[0]-K[0],A[1]-K[1],A[2]-K[2]);
    // side-on, in the robot's own frame: +s is BACKWARD (the way thigh +z and
    // shin +z rotate), i.e. against its facing
    const sx=-((A[0]-H[0])*FWD[0]+(A[2]-H[2])*FWD[2]), sy=A[1]-H[1];
    let tx=sx+dx, ty=sy+dy;
    // out of reach: raise the target toward the hip until the leg can reach it
    // -- with the planted foot held on the floor, that is the pelvis dropping
    // the reach limit is the STANDING leg's side-on length: its knee is at
    // straight (shin z = 0) and kinked sideways, so seen side-on it is shorter
    // than thigh + shin. Capping at thigh + shin let the IK ask for more than
    // a straight knee gives -- which it got by bending the knee backwards.
    const R=Math.hypot(sx,sy)*0.995; if(Math.hypot(tx,ty)>R) ty=-Math.sqrt(Math.max(1,R*R-tx*tx));
    const s=legAngles(sx,sy,a,b), g=legAngles(tx,ty,a,b);
    add(p,'thigh_'+side,[0,0,g.thigh-s.thigh]);
    add(p,'shin_'+side,[0,0,g.bend-s.bend]);
    /* A KNEE NEVER BENDS PAST STRAIGHT. The standing knee's bend is sideways
     * (on x, not the knee's z), so seen side-on the standing leg looks bent;
     * asking for a straight leg subtracted that phantom bend, drove shin z
     * negative, and hyperextended the knee -- 12.8 behind the leg line on the
     * left in the run: the limp. Straight is shin z = 0 on this rig. */
    const k=p['shin_'+side]; if(k[2]<0) p['shin_'+side]=[k[0],k[1],0];
  }
  /* ARMS ARE AIMED, not rotated about an axis. At the standing pose the
   * upper arms point 81% BACKWARDS (58 degrees behind vertical), and upper
   * arm z lifts them mostly UP -- so the old "swing" made the arms rise and
   * flail: the zombie run. Now each motion says where the ELBOW should be (a
   * direction from the shoulder: hanging, a little forward and out, swinging
   * about the sideways axis), and a damped least-squares solve on the three
   * upper-arm angles gets it there with the rig's own solver. Twisting the
   * arm about its own length barely moves the elbow, so the damping matters. */
  /* FACING. Everything here assumed the robot faces -x; a robot that spins
   * during its transform ends facing elsewhere. Its facing is read off its
   * own feet (the toes point forward -- measured), and aiming, leg IK and
   * travel all work in that frame. */
  let FWD=[-1,0,0];
  // BOTH feet: each splays ~21 degrees outward, so one foot alone skewed the
  // facing -- and with it the aim, the leg IK and the travel direction
  function facingOf(sol){ let x=0,z=0; for(const n of ['foot_l','foot_r']){ const f=sol[n]; x+=f.tail[0]-f.head[0]; z+=f.tail[2]-f.head[2]; }
    const L=Math.hypot(x,z)||1; return [x/L,0,z/L]; }
  function aimArm(p, side, fwdDeg, outw, solveFn){
    const k='uparm_'+side, th=fwdDeg*Math.PI/180;
    // right is up x forward; left is its opposite
    const rt=[-FWD[2],0,FWD[0]], sg=side==='l'?-1:1;
    const want=[FWD[0]*Math.sin(th)+rt[0]*sg*outw, -Math.cos(th), FWD[2]*Math.sin(th)+rt[2]*sg*outw]; const wl=Math.hypot(...want);
    for(let it=0; it<6; it++){
      const s0=solveFn(p), sh=s0[k].head, el=s0['forearm_'+side].head;
      const L=Math.hypot(el[0]-sh[0],el[1]-sh[1],el[2]-sh[2]);
      const e=[0,1,2].map(i=>sh[i]+want[i]/wl*L-el[i]);
      if(Math.hypot(...e)<0.8) break;
      const J=[0,1,2].map(ax=>{ const q=cp(p); const a=(q[k]||[0,0,0]).slice(); a[ax]+=1; q[k]=a;
        const ee=solveFn(q)['forearm_'+side].head; return [ee[0]-el[0],ee[1]-el[1],ee[2]-el[2]]; });
      // d = A^T (A A^T + lambda I)^-1 e, with A's columns the per-degree moves
      const A=[0,1,2].map(r=>[J[0][r],J[1][r],J[2][r]]), lam=0.08;
      const M=[0,1,2].map(r=>[0,1,2].map(c=>A[r][0]*A[c][0]+A[r][1]*A[c][1]+A[r][2]*A[c][2]+(r===c?lam:0)));
      const inv=inv3(M); if(!inv) break;
      const y=[0,1,2].map(r=>inv[r][0]*e[0]+inv[r][1]*e[1]+inv[r][2]*e[2]);
      const d=[0,1,2].map(ax=>Math.max(-20,Math.min(20, A[0][ax]*y[0]+A[1][ax]*y[1]+A[2][ax]*y[2])));
      add(p,k,d);
    }
  }
  function inv3(m){
    const [a,b,c]=m[0],[d,e,f]=m[1],[g,h,i]=m[2];
    const A=e*i-f*h, B=-(d*i-f*g), C=d*h-e*g, det=a*A+b*B+c*C; if(Math.abs(det)<1e-12) return null;
    return [[A/det,-(b*i-c*h)/det,(b*f-c*e)/det],[B/det,(a*i-c*g)/det,-(a*f-c*d)/det],[C/det,-(a*h-b*g)/det,(a*e-b*d)/det]];
  }
  function arms(p, fwdL, fwdR, bendL, bendR, solveFn, outw=0.14){
    aimArm(p,'l',fwdL,outw,solveFn); aimArm(p,'r',fwdR,outw,solveFn);
    add(p,'forearm_l',[0,0,bendL]); add(p,'forearm_r',[0,0,bendR]);
  }
  function legs(p, ph, G, lift, comp, solveFn){
    const s0=solveFn(p), D=G.duty, reach=D*G.step;
    for(const [s,off] of [['l',0],['r',0.5]]){
      const c=(ph+off)%1;
      let dx, dy;
      if(c<D){ const u=c/D;               // planted: ahead (-x) at landing, behind at lift-off,
        dx=-reach*(1-2*u);                 // sliding back at exactly the body's speed
        dy=comp*Math.sin(Math.PI*u); }     // the stance knee gives a little
      else { const u=(c-D)/(1-D);          // swing: eased forward on a lifted arc
        dx=reach-2*reach*sm(u); dy=lift*Math.sin(Math.PI*u); }
      ikLeg(p, s, dx, dy, s0);
      /* DAMAGE: a hyperextending knee on its stance -- the limp, kept as a
       * deliberate delta for the damage system to drive (P.limp.amount 0..1) */
      if(P.limp.amount>0 && P.limp.side===s && c<D)
        add(p,'shin_'+s,[0,0,-22*P.limp.amount*Math.sin(Math.PI*c/D)]);
    }
  }
  /* The ranged schedule: first shot once deployed, then alternating part /
   * hand every `every` seconds; a shot lives `life` seconds. */
  const RANGED={first:0.6, every:0.4, life:0.6, slugV:3500, boltV:4500};
  function rangedShots(t){
    const out=[]; if(t<RANGED.first) return out;
    const n=Math.floor((t-RANGED.first)/RANGED.every);
    for(let k=Math.max(0,n-3); k<=n; k++){ const age=t-(RANGED.first+k*RANGED.every);
      if(age>=0 && age<RANGED.life) out.push({src:k%2?'palm':'part', age}); }
    return out;
  }
  const clips={
    idle(t,p,fx,solveFn){ const b=Math.sin(TAU*t/3.2);
      add(p,'spine',[0,0,1.2*b]); arms(p,6+2*b,6+2*b,18+3*b,18+3*b,solveFn);
      return {air:0, travel:0, info:'breathing, 3.2 s'}; },
    walk(t,p,fx,solveFn){ const G=gaitInfo(0.25,0.6), ph=t/G.cycle;
      add(p,'spine',[0,0,3]);
      // each arm swings against its own leg: forward when that leg is back
      const sw=Math.cos(TAU*ph);
      arms(p, 6-16*sw, 6+16*sw, 22, 22, solveFn);
      legs(p,ph,G,0.12*P.L,0.03*P.L,solveFn);
      return {air:0, travel:G.v*t, info:`walk  Fr 0.25  ${(G.v/100).toFixed(2)} m/s  ${G.cadence.toFixed(2)} steps/s  hip +-${G.hipA.toFixed(0)} deg`}; },
    /* WALK <-> RUN, continuously. The slider is the Froude number (0.25 walk
     * .. 1.6 run, log-spaced) -- a gait's SHAPE -- and gravity sets its
     * SPEED, v = sqrt(Fr g L). On a low-gravity world the same setting is a
     * slower, floatier lope: cadence falls, the flight phase lengthens and the
     * hop grows as g * flight^2 / 8, which is how legs behave on the Moon. */
    locomote(t,p,fx,solveFn){ const u=Math.max(0,Math.min(1,P.gait)), e=sm(u);
      const Fr=0.25*Math.pow(1.6/0.25,u), duty=0.6+(0.34-0.6)*e;
      const G=gaitInfo(Fr,duty), ph=t/G.cycle, sw=Math.cos(TAU*ph);
      add(p,'spine',[0,0,3+7*e]);
      const amp=16+22*e, base=6+6*e;
      arms(p, base-amp*sw, base+amp*sw, 22+63*e, 22+63*e, solveFn, 0.14-0.04*e);
      legs(p,ph,G,(0.12+0.16*e)*P.L,(0.03+0.07*e)*P.L,solveFn);
      const inStep=(ph*2)%1, fStart=G.duty*2, fr=(inStep-fStart)/(1-fStart);
      const air=(G.flight>0 && fr>0) ? 0.5*P.g*(fr*G.flight)*(G.flight-fr*G.flight) : 0;
      return {air, travel:G.v*t, info:`${u<0.35?'walk':u<0.65?'jog':'run'}  Fr ${Fr.toFixed(2)}  ${(G.v/100).toFixed(2)} m/s at ${(P.g/981).toFixed(2)} g  ${G.cadence.toFixed(2)} steps/s`+(G.flight>0?`  flight ${(G.flight*1000).toFixed(0)} ms, hop ${(P.g*G.flight*G.flight/8).toFixed(0)} u`:'')}; },
    run(t,p,fx,solveFn){ const G=gaitInfo(1.6,0.34);
      const ph=t/G.cycle; add(p,'spine',[0,0,10]);
      // a runner's arms: elbows at ~90, pumping forward and back
      const sw=Math.cos(TAU*ph);
      arms(p, 12-38*sw, 12+38*sw, 85, 85, solveFn, 0.10);
      legs(p,ph,G,0.28*P.L,0.10*P.L,solveFn);
      // the flight phase: both feet off, a ballistic hop per step
      const inStep=(ph*2)%1, fStart=G.duty*2, fr=(inStep-fStart)/(1-fStart);
      const air= fr>0 ? 0.5*P.g*(fr*G.flight)*(G.flight-fr*G.flight) : 0;
      return {air, travel:G.v*t, info:`run  Fr 1.6  ${(G.v/100).toFixed(2)} m/s  flight ${(G.flight*1000).toFixed(0)} ms/step  hop ${G.hop.toFixed(0)} u`}; },
    jump(t,p,fx,solveFn){
      /* apex from the legs: pushing through a crouch of depth d with force
       * F gives h = d (F/mg - 1). Foot thrusters (thrust/weight) lower the
       * effective gravity on the way UP only; the fall is plain g. */
      // leg force and thrust are ABSOLUTE, quoted as multiples of the robot's
      // weight at 1 g: halve gravity and the same legs jump higher (they did
      // not, while force was a multiple of the CURRENT weight)
      const d=0.35*P.L, h0=d*Math.max(0.05,P.legF*(981/P.g)/P.mass-1);
      // the LEGS set take-off speed, under plain gravity; the thrusters then
      // soften the deceleration on the way up, so the apex is h0/(1-thrust).
      // (Computing v0 under the reduced gravity cancelled the thrust out.)
      const gUp=Math.max(0.05*P.g, P.g-P.thrust*981/P.mass), v0=Math.sqrt(2*P.g*h0), h=v0*v0/(2*gUp);
      const tUp=v0/gUp, tDn=Math.sqrt(2*h/P.g), T=tUp+tDn;
      const tc=0.35, tp=0.12, ta=0.28, tr=0.45, period=tc+tp+T+ta+tr+0.6, u=t%period;
      let air=0, crouch=0, tuck=0, seg='rest';
      if(u<tc){ crouch=sm(u/tc); seg='crouch'; }
      else if(u<tc+tp){ crouch=1-sm((u-tc)/tp); seg='push'; }
      else if(u<tc+tp+T){ const a=u-tc-tp; seg=a<tUp?'rise':'fall';
        air = a<tUp ? v0*a-0.5*gUp*a*a : h-0.5*P.g*(a-tUp)*(a-tUp); tuck=Math.sin(Math.PI*a/T);
        fx.feet = a<tUp ? P.thrust : 0; }
      else if(u<tc+tp+T+ta){ crouch=Math.sin(Math.PI*(u-tc-tp-T)/ta)*Math.min(1,v0/400); seg='land'; }
      else seg='recover';
      both(p,'thigh',[0,0,-38*crouch-30*tuck]); both(p,'shin',[0,0,72*crouch+55*tuck]); both(p,'foot',[0,0,-30*crouch]);
      // arms swing BACK in the crouch and drive forward through the jump
      add(p,'spine',[0,0,14*crouch]);
      const aF=6-40*crouch+35*tuck; arms(p,aF,aF,25+20*crouch,25+20*crouch,solveFn);
      return {air, travel:0, info:`jump  ${seg}  apex ${(h/100).toFixed(2)} m  airtime ${T.toFixed(2)} s  take-off ${(v0/100).toFixed(1)} m/s`}; },
    dash(t,p,fx,solveFn){
      /* back thrusters: thrust/weight T accelerates at T g / m while firing */
      const Tw=0.9, acc=Tw*981/P.mass, tl=0.25, tf=0.6, tb=0.5, period=tl+tf+tb+0.9, u=t%period, n=Math.floor(t/period);
      const vmax=acc*tf, per=0.5*acc*tf*tf+vmax*tb*0.5;
      let lean=0, x=0, seg='rest';
      if(u<tl){ lean=sm(u/tl); seg='lean in'; }
      else if(u<tl+tf){ const a=u-tl; lean=1; x=0.5*acc*a*a; fx.back=1; seg='thrust'; }
      else if(u<tl+tf+tb){ const a=u-tl-tf; lean=1-2*sm(a/tb); x=0.5*acc*tf*tf+vmax*a-0.5*(vmax/tb)*a*a; seg='brake'; }
      else { x=per; lean=-1+sm((u-tl-tf-tb)/0.9); }
      add(p,'spine',[0,0,22*lean]);
      const aD=6-50*Math.max(0,lean); arms(p,aD,aD,25,25,solveFn);
      both(p,'thigh',[0,0,14*Math.max(0,lean)]); both(p,'shin',[0,0,20*Math.max(0,lean)]);
      return {air:fx.back?10:0, travel:n*per+x, info:`dash  ${seg}  thrust/weight ${Tw}  top ${(vmax/100).toFixed(1)} m/s`}; },
    /* WEAPON HAND. The weapon arm rises to aim straight ahead while the hand
     * retracts and the part mounted there deploys (the page moves the part;
     * the core reports how far: fx.weapon 0..1, and the attack: fx.attack).
     * The attack depends on the weapon: a saw sweeps side to side, spinning
     * hard; a cannon fires with a recoil kick; a staff thrusts. */
    weapon(t,p,fx,solveFn){
      const dep=sm(t/0.6), mode=P.weaponMode||'saw';
      const k= t<0.6 ? 0 : ((t-0.6)%1.2)/1.2;
      let fwdR=6+(90-6)*dep, bendR=5+60*(1-dep), out=0.14, atk=0, seg=dep<1?'deploy':'';
      if(t>=0.6){
        if(mode==='saw'){ const sw=Math.sin(TAU*k); add(p,'spine',[0,28*sw,4]); out=0.14+0.35*Math.max(0,sw); atk=1; seg='sweep'; }
        else if(mode==='cannon'){ atk=Math.exp(-Math.pow((k-0.1)/0.04,2)); fwdR=90-9*atk; bendR=5+14*atk; add(p,'spine',[0,0,-4*atk]); seg=atk>0.3?'fire':'aim'; }
        else { const r=Math.sin(Math.PI*Math.min(1,k/0.5)); fwdR=90+25*r; atk=Math.max(0,Math.sin(TAU*k)); seg=k<0.5?'raise':'thrust'; }
      }
      arms(p, 45, fwdR, 105, bendR, solveFn, out);
      fx.weapon=dep; fx.attack=atk;
      return {air:0, travel:0, info:`weapon  ${mode}  ${seg||'ready'}  deployed ${(dep*100).toFixed(0)}%`}; },
    /* RANGED. The weapon arm aims straight ahead; a barrel pops out of the
     * mounted part (from a wheel's HUB, or the tip of a cannon or staff) and
     * a blaster from the HAND. They fire alternately. The schedule is a pure
     * function of time -- which shots are in flight and how old -- so any
     * frame stands alone; the page flies each shot from its muzzle. */
    ranged(t,p,fx,solveFn){
      const dep=sm(t/0.5), shots=rangedShots(t);
      let kPart=0, kPalm=0;
      for(const sh of shots){ const k=Math.exp(-sh.age/0.06); if(sh.src==='part') kPart=Math.max(kPart,k); else kPalm=Math.max(kPalm,k); }
      const kick=Math.max(kPart, 0.5*kPalm);
      arms(p, 45, 6+84*dep-7*kick, 105, 5+50*(1-dep)+10*kick, solveFn);
      add(p,'spine',[0,0,-3*kick]);
      fx.ranged=dep; fx.shots=shots; fx.kickPart=kPart; fx.kickPalm=kPalm;
      return {air:0, travel:0, info:`ranged  slugs ${(RANGED.slugV/100).toFixed(0)} m/s ballistic, bolts ${(RANGED.boltV/100).toFixed(0)} m/s straight, a shot every ${RANGED.every}s`}; },
    /* THE FLOATING MAGE. No floor to plant on, so the legs are posed, not
     * solved: together, knees soft, toes down. The bob is a hover thrust
     * controller settling: a lightly damped spring, period 2.4 s. */
    float(t,p,fx,solveFn){ const b=Math.sin(TAU*t/2.4), w=Math.sin(TAU*t/3.7);
      add(p,'spine',[0,2*w,-2+1.5*b]);
      arms(p, 52+6*w, 52-6*w, 38+8*b, 38-8*b, solveFn, 0.55);
      fx.feet=0.28+0.06*b;
      // the bob stays ABOVE its base: on a robot with no hover height a +-7
      // bob went 7 units into the floor
      return {air:3.5*(1+b), travel:0, info:`float  hover bob 7 u, period 2.4 s`}; },
    glide(t,p,fx,solveFn){
      /* tilting the thrust by theta accelerates at g tan(theta) against drag
       * k v: terminal speed g tan(theta) / k */
      const th=14*Math.PI/180, k=4.5, v=P.g*Math.tan(th)/k;
      add(p,'spine',[0,0,14]); both(p,'thigh',[0,0,16]); both(p,'shin',[0,0,10]);
      arms(p,-28,-28,30,30,solveFn,0.35);
      fx.feet=0.35; fx.back=0.5;
      return {air:1.5*(1+Math.sin(TAU*t/1.9)), travel:v*t, info:`glide  thrust tilted 14 deg  ${(v/100).toFixed(1)} m/s terminal`}; },
    cast(t,p,fx,solveFn){
      const tg=0.45, tt=0.14, th=0.35, tr=0.5, period=tg+tt+th+tr, u=t%period;
      let k=0, seg='ready';
      if(u<tg){ k=-sm(u/tg); seg='gather'; } else if(u<tg+tt){ k=-1+2*sm((u-tg)/tt); seg='cast'; }
      else if(u<tg+tt+th){ k=1; seg='hold'; } else { k=1-sm((u-tg-tt-th)/tr); seg='return'; }
      const fwd= k<0 ? 40-15*(-k) : 40+48*k, bend= k<0 ? 40+55*(-k) : 40*(1-k);
      arms(p, fwd, fwd, bend, bend, solveFn, k>0?0.12:0.35);
      add(p,'spine',[0,0, k>0? -6*k : 4*(-k)]);
      fx.feet=0.3+0.4*Math.max(0,k);
      return {air:0, travel:-8*Math.max(0,k), info:`cast  ${seg}`}; },
    punch(t,p,fx,solveFn){
      const tw=0.30, ts=0.12, tr=0.40, period=tw+ts+tr+0.35, u=t%period;
      let k=0, seg='guard';
      if(u<tw){ k=-sm(u/tw); seg='wind-up'; } else if(u<tw+ts){ k=-1+2*sm((u-tw)/ts); seg='strike'; }
      else if(u<tw+ts+tr){ k=1-sm((u-tw-ts)/tr); seg='recover'; }
      // right: a straight punch driven by the torso's twist; left: guard up
      // the striking elbow is AIMED: pulled back in the wind-up, straight
      // ahead (90 degrees forward) at full extension -- an angle offset had
      // carried it past horizontal to overhead
      add(p,'spine',[0,-22*k,5]);
      const fwdR= k<0 ? 6-30*(-k) : 6+84*k;
      arms(p, 45, fwdR, 105, k<0? 20+70*(-k) : 20*(1-k), solveFn);
      both(p,'thigh',[0,0,-8]); both(p,'shin',[0,0,14]);
      return {air:0, travel:0, info:`punch  ${seg}  fist wheel leads`}; },
  };
  return {
    P, clips: Object.keys(clips), gaitInfo, RANGED, rangedShots,
    pose(clip, t, stand, solveFn){
      const p=JSON.parse(JSON.stringify(stand)), fx={feet:0, back:0, weapon:0, attack:0, ranged:0, shots:[], kickPart:0, kickPalm:0};
      FWD=facingOf(solveFn(p));
      const r=(clips[clip]||clips.idle)(t,p,fx,solveFn);
      // travel along its OWN facing (it was always -x), wrapping at 900
      const pos=(p._pos||[0,0,0]).slice(), tr=r.travel%900; pos[0]+=FWD[0]*tr; pos[2]+=FWD[2]*tr;
      p._pos=pos;
      return {p, air:r.air, fx, info:r.info};
    },
  };
})();

let MOTION_ON='', MOTION_T0=0, MOTION_FX={feet:0,back:0,weapon:0,attack:0,ranged:0,shots:[],kickPart:0,kickPalm:0};
if(RIG.weapons && RIG.weapons.length) MOTION.P.weaponMode=RIG.weapons[0].mode;
const WEAPON_OF=Object.fromEntries((RIG.weapons||[]).map(w=>[w.part,w]));
/* thruster flames: cones, additive, shown only while the core says so */
const flameMat=new THREE.MeshBasicMaterial({color:0xffa040,transparent:true,opacity:0.8,blending:THREE.AdditiveBlending,depthWrite:false});
const flameGeo=new THREE.ConeGeometry(7,1,12,1,true); flameGeo.translate(0,-0.5,0);
const flames=[0,1,2,3,4].map(()=>{ const m=new THREE.Mesh(flameGeo,flameMat); m.visible=false; scene.add(m); return m; });

/* RANGED hardware: a barrel popping out of the part's hub, one out of the
 * hand, a pool of shots, and a flash per source. */
const gunMat=new THREE.MeshStandardMaterial({color:0x8a939e,metalness:0.9,roughness:0.3});
const barrelGeo=new THREE.CylinderGeometry(1,1,1,12); barrelGeo.translate(0,0.5,0);
const hubBarrel=new THREE.Mesh(barrelGeo,gunMat), palmBarrel=new THREE.Mesh(barrelGeo,gunMat);
[hubBarrel,palmBarrel].forEach(m=>{ m.visible=false; scene.add(m); });
const slugMat=new THREE.MeshBasicMaterial({color:0xffc060}), boltMat=new THREE.MeshBasicMaterial({color:0x7fd8ff,transparent:true,opacity:0.9,blending:THREE.AdditiveBlending,depthWrite:false});
const shotGeo=new THREE.CylinderGeometry(1,1,1,8); shotGeo.translate(0,-0.5,0);
const shotPool=[...Array(8)].map(()=>{ const m=new THREE.Mesh(shotGeo,slugMat); m.visible=false; scene.add(m); return m; });
const flashPart=new THREE.Mesh(flameGeo,flameMat), flashPalm=new THREE.Mesh(flameGeo,flameMat);
[flashPart,flashPalm].forEach(m=>{ m.visible=false; scene.add(m); });

function refresh(){
  const t=+tIn.value;
  document.getElementById('tv').textContent=t.toFixed(2);
  let {p,seg,bulk}=poseAt(t);
  /* ROBOT-FORM MOTION: once transformed, the pose comes from the headless
   * core, grounding adds its airborne height, and the torso turn is off. */
  let air=0; MOTION_FX={feet:0,back:0,weapon:0,attack:0,ranged:0,shots:[],kickPart:0,kickPalm:0};
  /* The robot-form motion BLENDS IN over t 0.9..1 instead of switching on at
   * 0.999: the mage's arms snapped 90 degrees in the last hundredth. */
  const mw=MOTION_ON? Math.max(0,Math.min(1,(t-0.9)/0.1)) : 0, mwe=mw*mw*(3-2*mw);
  const moving=MOTION_ON && mw>=1;
  if(MOTION_ON && mw>0){
    const m=MOTION.pose(MOTION_ON,(performance.now()-MOTION_T0)/1000,p,solve);
    if(mwe>=0.999) { p=m.p; bulk={}; }
    else { p=blend(p,m.p,mwe); }
    air=m.air*mwe; MOTION_FX=m.fx; seg=MOTION_ON;
    for(const k of ['feet','back','weapon','ranged','attack']) MOTION_FX[k]=(MOTION_FX[k]||0)*mwe;
    const hud=document.getElementById('mhud'); if(hud) hud.textContent=m.info;
  }
  document.getElementById('pn').textContent=seg;
  const p2=Object.assign({},dynOn&&!moving?settle(p,lastDt):p);
  const sp=moving?0:spinAt(t);
  if(Math.abs(sp)>0.01){
    const a=(p2['spine']||[0,0,0]).slice();
    p2['spine']=[a[0],a[1]+sp,a[2]];
  }
  const s=solve(p2);

  /* Ground on the MESH, not the joints: a foot's geometry hangs below its
   * ankle, so a correctly grounded rig still renders sunk once it has a body. */
  // the smoothed curve, but never below what THIS frame needs: the heft and
  // the spine spin move the pose slightly off the sampled one
  /* FLOATING: a hover robot's height is added AFTER grounding (grounding
   * pulls the lowest point to the floor). It ramps in through the levitation
   * so it rises rather than popping up at the end. */
  const floatH=(MODE==='air'&&RIG.hover)?RIG.hover:(MODE==='zerog'?(RIG.hover||70):0);
  const HOV=floatH? floatH*Math.max(0,Math.min(1,(t-0.5)/0.4))**2*(3-2*Math.max(0,Math.min(1,(t-0.5)/0.4))):0;
  const lTr=Math.max(smoothLift(t), groundLift(s,bulk)), lMo=groundLift(s,bulk)+air;
  const lift=(moving ? lMo : mw>0 ? lTr+(lMo-lTr)*mwe : lTr)+HOV;
  // flames: two under the feet, two on the back (behind the pelvis)
  { const put=(m,at,dir,len)=>{ m.visible=len>1; if(!m.visible) return;
      m.position.set(at[0],at[1]+lift,at[2]); m.scale.set(1,len,1);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(0,-1,0), new THREE.Vector3(...dir).normalize()); };
    const ft=MOTION_FX.feet, bk=MOTION_FX.back;
    put(flames[0], s.foot_l.head, [0,-1,0], 60*ft); put(flames[1], s.foot_r.head, [0,-1,0], 60*ft);
    const pv=s.pelvis.head, bm=s.pelvis.m, back=[bm[0][0],bm[1][0]-0.4,bm[2][0]];
    put(flames[2], [pv[0]+bm[0][0]*22, pv[1]+14, pv[2]-12], back, 80*bk);
    put(flames[3], [pv[0]+bm[0][0]*22, pv[1]+14, pv[2]+12], back, 80*bk);
    // MUZZLE FLASH: ahead of the retracted fist, along the forearm, on each shot
    const Wc=(RIG.weapons||[]).find(w=>w.mode==='cannon');
    if(Wc && MOTION_FX.weapon>0.9 && MOTION_FX.attack>0.15){
      /* at the BARRELS' tip: the far end of the cannon's own long axis, from
       * its transform (a frame old here, which is negligible). A fixed 48
       * past the wrist left the flash floating ahead of and below the guns. */
      const fa=s['forearm_'+Wc.side], d=[fa.tail[0]-fa.head[0],fa.tail[1]-fa.head[1],fa.tail[2]-fa.head[2]];
      const gm=carMeshes[Wc.part]; gm.geometry.computeBoundingBox(); const bb=gm.geometry.boundingBox;
      const ext=[bb.max.x-bb.min.x,bb.max.y-bb.min.y,bb.max.z-bb.min.z], ax=ext.indexOf(Math.max(...ext));
      const ends=[0,1].map(e=>{ const v=bb.getCenter(new THREE.Vector3()); v.setComponent(ax, e?bb.max.getComponent(ax):bb.min.getComponent(ax)); return v.applyMatrix4(gm.matrixWorld); });
      // the muzzle is the end further along the arm
      const dl=Math.hypot(...d)||1, along=v=>v.x*d[0]/dl+v.y*d[1]/dl+v.z*d[2]/dl;
      const tip=along(ends[0])>along(ends[1])?ends[0]:ends[1];
      put(flames[4], [tip.x,tip.y-lift,tip.z], d, 70*MOTION_FX.attack);
    } else flames[4].visible=false; }

  const lifted={};
  for(const k in s) lifted[k]={head:[s[k].head[0],s[k].head[1]+lift,s[k].head[2]],
                               tail:[s[k].tail[0],s[k].tail[1]+lift,s[k].tail[2]]};
  const lifted0=lifted;
  for(const b of RIG.bones){
    if(!bMesh[b.n]) continue;
    place(bMesh[b.n],[s[b.n].head[0],s[b.n].head[1]+lift,s[b.n].head[2]],
                     [s[b.n].tail[0],s[b.n].tail[1]+lift,s[b.n].tail[2]]);
    bMesh[b.n].visible=showBones;
    jMesh[b.n].visible=showBones;
    jMesh[b.n].position.set(s[b.n].head[0],s[b.n].head[1]+lift,s[b.n].head[2]);
  }
  for(const n in bodyMesh){
    const so=s[n];
    if(!so){bodyMesh[n].visible=false;continue;}
    // flex: the follower's angular speed at this joint, normalised
    const dyn=DYN[n];
    let flex=0;
    if(dyn&&dyn.v) flex=Math.min(1,(Math.abs(dyn.v[0])+Math.abs(dyn.v[1])
                                   +Math.abs(dyn.v[2]))/160);
    // bones nearer the core resolve first, so the machine builds outward
    const depth=Math.min(1,Math.abs(so.head[1]-(s.pelvis?s.pelvis.head[1]:0))/220);
    const bm2=bodyMesh[n].userData.mat;
    if(bm2&&bm2.userData.sh)
      bm2.userData.sh.uniforms.uVox.value=
        Math.max(0,Math.min(1,bodyVox(t)*(0.55+0.75*depth)));
    const ca=bodyMesh[n].geometry.getAttribute('color');
    const w=bodyMesh[n].userData.w;
    if(ca&&w&&w.length){
      for(let vi=0;vi<w.length;vi++){
        const j=w[vi];
        // base steel, darkened and warmed toward the joints, brightened
        // further where the joint is working
        const heat=j*(0.22+0.78*flex);
        // wider spread between panel and joint, and brighter overall: the
        // old range was 0.48..0.69 which is all mid-grey once lit
        ca.array[vi*3  ]=0.72+0.34*heat-0.30*j;
        ca.array[vi*3+1]=0.76+0.16*heat-0.36*j;
        ca.array[vi*3+2]=0.84-0.16*heat-0.44*j;
      }
      ca.needsUpdate=true;
    }
    bodyMesh[n].visible=showBody;
    /* bulk is a 3-VECTOR (radial, along, radial) since the poses changed to
     * it, but this line still multiplied by it as a scalar: number * array is
     * NaN, so the pelvis, spine and chest -- the bones the fold squeezes --
     * had NaN matrices, and were invisible, through the whole fold and early
     * rise on every vehicle. Scale each axis of the bone frame by its own
     * factor, exactly as the grounding code applies it to vertices. */
    const m=so.m, b=(bulk&&bulk[n])?bulk[n]:1, k0=Array.isArray(b)?b:[b,b,b];
    // a weapon hand RETRACTS into its forearm as the weapon deploys
    const wr=(MOTION_FX.weapon>0 && (RIG.weapons||[]).some(w=>n==='hand_'+w.side && w.mode!=='staff'))?1-0.92*MOTION_FX.weapon:1;
    const k=[k0[0]*wr,k0[1]*wr,k0[2]*wr];
    bodyMesh[n].matrix.set(
      m[0][0]*k[0],m[0][1]*k[1],m[0][2]*k[2],so.head[0],
      m[1][0]*k[0],m[1][1]*k[1],m[1][2]*k[2],so.head[1]+lift,
      m[2][0]*k[0],m[2][1]*k[1],m[2][2]*k[2],so.head[2],
      0,0,0,1);
  }

  // accumulate what is on screen, for the camera to frame
  BB.makeEmpty();
  const addPt=(x,y,z)=>BB.expandByPoint(new THREE.Vector3(x,y,z));
  for(const k in lifted0){
    addPt(lifted0[k].head[0],lifted0[k].head[1],lifted0[k].head[2]);
    addPt(lifted0[k].tail[0],lifted0[k].tail[1],lifted0[k].tail[2]);
  }
  if(showCar) for(const g in carMeshes){
    if(!carMeshes[g].visible) continue;
    const pl=RIG.car.place[g], so=s[pl.b];
    if(!so) continue;
    // the panel's own reach, approximated from its group radius
    const m2=CAR_META.find(m=>m.g===g);
    const rr=m2?Math.max(...(m2.r||[60,60,60]))*pl.s:60*pl.s;
    addPt(so.head[0]+rr, so.head[1]+lift+rr, so.head[2]+rr);
    addPt(so.head[0]-rr, so.head[1]+lift-rr, so.head[2]-rr);
  }
  if(!BB.isEmpty()){
    const c=BB.getCenter(new THREE.Vector3());
    const sz=BB.getSize(new THREE.Vector3());
    focus.lerp(c, 0.25);
    const need=Math.max(sz.y, sz.x*0.7, sz.z*0.7);
    // 2.1 rather than 1.5: at 1.5 the machine filled the frame edge to
    // edge with nothing around it, which reads as too close even when
    // it technically fits.
    const want=need/(2*Math.tan(camera.fov*Math.PI/360))*2.1;
    frameR += (want-frameR)*0.25;
  }

  for(const k in s) lifted[k]={head:[s[k].head[0],s[k].head[1]+lift,s[k].head[2]],
                               tail:[s[k].tail[0],s[k].tail[1]+lift,s[k].tail[2]]};
  // core rides the pelvis and reports the fold: full when folded, a nub when
  // stood, with the shards travelling in as it closes
  const cb=lifted[RIG.core.bone];
  if(cb){
    const cs=RIG.core.size*(1-0.55*t)*0.5;
    const cx=(cb.head[0]+cb.tail[0])*0.5, cy=(cb.head[1]+cb.tail[1])*0.5,
          cz=(cb.head[2]+cb.tail[2])*0.5;
    coreMesh.position.set(cx,cy,cz);
    coreMesh.scale.setScalar(cs);
    coreMesh.rotation.y=t*3.0;
    coreMesh.visible=showCore;
    RIG.core.shards.forEach((sh,i)=>{
      const a2=sh.a+t*2.2;
      const dist=cs*(0.62+1.9*(1-t)*(0.35+0.65*sh.r));
      const e=cs*(0.20+0.12*sh.r);
      shardMeshes[i].position.set(cx+dist*Math.cos(a2),
                                  cy+(sh.r-0.5)*cs*2.2,
                                  cz+dist*Math.sin(a2));
      shardMeshes[i].scale.setScalar(e);
      shardMeshes[i].rotation.set(a2*1.7,a2*1.1,a2*0.6);
      shardMeshes[i].visible=showCore;
    });
  }
  // car panels ride their bone: local euler, then the offset, then the
  // bone's own transform
  /* every bone's rigid motion since the fold: world p' = dq*p + dt. Shared by
   * all skinned panels this frame. */
  const BD=[];
  if(RIG.car.cubes===false){
    const R0=ref0();
    const qOf=m=>new THREE.Quaternion().setFromRotationMatrix(new THREE.Matrix4().set(
      m[0][0],m[0][1],m[0][2],0, m[1][0],m[1][1],m[1][2],0, m[2][0],m[2][1],m[2][2],0, 0,0,0,1));
    for(const B of RIG.bones){
      const J=s[B.n], J0=R0.s[B.n];
      if(!J||!J0){ BD.push([new THREE.Quaternion(), new THREE.Vector3()]); continue; }
      const dq=qOf(J.m).multiply(qOf(J0.m).invert());
      const h0=new THREE.Vector3(J0.head[0], J0.head[1]+R0.lift, J0.head[2]);
      const ht=new THREE.Vector3(J.head[0], J.head[1]+lift, J.head[2]);
      BD.push([dq, ht.sub(h0.applyQuaternion(dq))]);
    }
  }
  for(const g in carMeshes){
    const pl=RIG.car.place[g];
    const so=s[pl.b];
    if(!so){ carMeshes[g].visible=false; continue; }
    carMeshes[g].visible=showCar;
    // derived alignment, not the authored euler: thin axis outward, long axis
    // along the limb
    const lm=pl.m;
    /* STOW: the car's interior has nowhere to go on a robot, so it shrinks
     * into the core as the body opens. That is what the core cube is for --
     * matter going somewhere -- and it is the volume the armour spreads into.
     * Gone by 70%, which is before the panels finish settling. */
    let fk=1;
    if(pl.f==='stow'){
      /* DISSOLVE, not shrink. An interior part that merely gets smaller reads
       * as a scaling bug; one that breaks into cubes and falls toward the core
       * reads as matter being reclaimed -- which is what compact.rs describes
       * and what the core exists to receive.
       *
       * Done by SNAPPING the panel's vertices to a lattice whose cell grows as
       * it goes: the geometry turns blocky first, then coarse, then it is a
       * handful of cubes, then nothing. One uniform, no extra geometry. */
      /* Hold the car together through the FIRST HALF. Interiors used to start
       * dissolving immediately, so by 20% the vehicle had already stopped
       * looking like one. Nothing goes until 0.34, and it is gone by 0.82 --
       * which is also when the ribcage has opened far enough to take what the
       * panels leave behind. */
      /* PER-GROUP window. One global (t-0.18)/0.44 had every reclaiming
       * part diminishing at the same rate toward the same point -- a global
       * shrink, which is why it stopped reading as a transformation. The
       * cockpit goes at 0.16 and the shell not until 0.72. */
      /* a streamed part needs room for three acts. The interiors were given
       * windows as short as 0.14, which compressed crazing, compaction and
       * decay into a blink -- another cutoff, just a smaller one. */
      const dw0 = pl.dis || [0.40, 0.22];
      const dw = pl.st ? [dw0[0], Math.max(dw0[1], 0.36)] : dw0;
      const u=Math.max(0, Math.min(1, (t-dw[0])/dw[1]));
      if(pl.st){
        carMeshes[g].userData.u=u;
        if(u>=0.999 && !pl.keep){ carMeshes[g].visible=false;
          if(carCubes[g]) carCubes[g].im.visible=false; continue; }
        fk=1;
      } else {
        fk=Math.max(0,1-u); fk=fk*fk;
        if(fk<0.02){ carMeshes[g].visible=false; continue; }
      }
      // a panel with cubes takes no vertex effect: its cubes do the work
      carMeshes[g].userData.vox = (pl.st && carCubes[g]) ? 0 : u;
    } else carMeshes[g].userData.vox=0;
    const msh=carMeshes[g].material;
    if(msh.userData.sh){
      msh.userData.sh.uniforms.uVox.value=carMeshes[g].userData.vox;
      msh.userData.sh.uniforms.uTime.value=performance.now()*0.001;
      msh.userData.sh.uniforms.uStream.value=(pl.st && !carCubes[g])?1:0;
    }
    const _bk=(bulk&&bulk[pl.b])?bulk[pl.b]:[1,1,1];
    const bm=so.m, kb=pl.s*fk*((_bk[0]+_bk[1]+_bk[2])/3);
    // ROBOT end: bone * local alignment, scaled, offset along the bone
    const M=[0,1,2].map(i=>[0,1,2].map(j=>
      bm[i][0]*lm[0][j]+bm[i][1]*lm[1][j]+bm[i][2]*lm[2][j]));
    const ox=bm[0][0]*pl.o[0]+bm[0][1]*pl.o[1]+bm[0][2]*pl.o[2];
    const oy=bm[1][0]*pl.o[0]+bm[1][1]*pl.o[1]+bm[1][2]*pl.o[2];
    const oz=bm[2][0]*pl.o[0]+bm[2][1]*pl.o[1]+bm[2][2]*pl.o[2];
    const rPos=new THREE.Vector3(so.head[0]+ox, so.head[1]+oy+lift, so.head[2]+oz);
    const rM=new THREE.Matrix4().set(
      M[0][0],M[0][1],M[0][2],0, M[1][0],M[1][1],M[1][2],0,
      M[2][0],M[2][1],M[2][2],0, 0,0,0,1);
    const rQ=new THREE.Quaternion().setFromRotationMatrix(rM);

    /* CAR end: the identity. The geometry is already centred on its own
     * middle, so putting that middle back where it was on the car -- with no
     * rotation and no scale -- reproduces the vehicle exactly. */
    const vPos=new THREE.Vector3(pl.car[0], pl.car[1]+CARLIFT, pl.car[2]);
    const vQ=IDENTQ;

    /* THREE PHASES, not one blend.
     *
     * A single lerp from car to robot makes every panel leave its neighbours
     * the moment it starts moving, which is why the middle of the transform
     * looked like debris. A shell comes apart in stages:
     *
     *   CRACK   0.00-0.24  the panel lifts a little along its OWN outward
     *                      normal and turns a few degrees. The car is still
     *                      a car -- it has just split along its seams.
     *   ATTACH  0.24-0.52  it takes its bone's orientation while staying near
     *                      where it was. This is the machine getting hold of
     *                      it; nothing travels yet.
     *   ALIGN   0.52-0.95  now it moves to its seat, carried by a bone that
     *                      is already holding it.
     *
     * Splitting rotation from translation is what makes the difference: a
     * panel that turns before it travels reads as being gripped, and one that
     * does both at once reads as being thrown.
     */
    const ph = (a,b) => { let u=Math.max(0,Math.min(1,(t-a)/(b-a))); return u*u*(3-2*u); };
    const lay3 = (pl.lay||0)/3;
    // Retimed against HOLD: the crack runs and FINISHES inside the window the
    // body spends folded, so the shell is open before anything pushes on it.
    /* Rotation happens DURING the travel, not before it.
     *
     * Attach ran 0.12-0.28 and align 0.16-0.68, so a panel had turned 71
     * degrees by t=0.22 and 108 by 0.26 -- spinning in place while the body
     * was still folded and the car was still supposed to look like a car.
     * That is the axial rotation at global scale in the opening.
     *
     * Attach now trails align: the panel starts moving, and turns as it goes.
     * A part being carried somewhere rotates on the way; it does not pirouette
     * first and then set off. */
    const cr = ph(0.00+0.02*lay3, 0.14+0.03*lay3);        // crack
    const al = ph(0.16+0.05*lay3, 0.68+0.07*lay3);        // align
    const at = ph(0.30+0.05*lay3, 0.74+0.06*lay3);        // attach

    // crack: out along the panel's own normal, which the alignment already
    // gives us as the third column of its local frame
    const nx=lm[0][1], ny=lm[1][1], nz=lm[2][1];
    // and less travel: the panels were moving far enough out during the
    // crack that the shell stopped being adapted to the car
    const crackD = 3.5 + 3.0*lay3;
    const cPos = vPos.clone().add(
      new THREE.Vector3(nx,ny,nz).multiplyScalar(crackD*cr));
    // a few degrees of tilt as it pops, so the seam opens rather than the
    // panel sliding straight out
    // 0.035, not 0.12. At 0.12 a panel turned 22 degrees while the shell was
    // supposed to be merely splitting along its seams -- enough to swing it
    // clear of the body and through the skeleton underneath. A crack opens a
    // gap; it does not reorient anything.
    const cQ = vQ.clone().slerp(rQ, 0.035*cr);

    /* CARRIED, not lerped.
     *
     * The two windows that read as floating were 10-40% and 65-80%, and they
     * are the same fault seen twice: the panel's position was in WORLD space
     * the whole way. During `attach` it rotated in place while the body moved
     * out from under it, and during `align` it slid through the air on a
     * straight line between two world points. Nothing was holding it.
     *
     * So once gripped, the panel's position is expressed in its BONE's frame
     * and only the LOCAL offset interpolates. The bone is standing up, so the
     * panel goes with it -- motion comes from the machine rather than from a
     * tween, which is the whole difference between carried and floating.
     *
     * Bone matrices are orthonormal, so the inverse is the transpose.
     */
    /* GRIP FOLLOWS THE CRACK, not the travel.
     *
     * A panel only became bone-relative in proportion to how far it had
     * TRAVELLED, so at t=0.45 it was 53% held by the bone and 47% still
     * anchored to the car's coordinates -- and the bone was by then pushing
     * the machine up off the floor. The shell and the wheels lagged the
     * push-up because half of each panel was still parked where the car used
     * to be.
     *
     * Attached means attached. Once the seam has opened the bone has hold of
     * the piece, and `al` then moves it from where it was caught to its seat
     * WITHIN the bone's frame -- so it rides the push-up from the start
     * instead of catching up afterwards.
     */
    const grip = cr;
    const rel = new THREE.Vector3(
      cPos.x - so.head[0], cPos.y - (so.head[1]+lift), cPos.z - so.head[2]);
    const lCar = new THREE.Vector3(
      bm[0][0]*rel.x + bm[1][0]*rel.y + bm[2][0]*rel.z,
      bm[0][1]*rel.x + bm[1][1]*rel.y + bm[2][1]*rel.z,
      bm[0][2]*rel.x + bm[1][2]*rel.y + bm[2][2]*rel.z);
    const lRob = new THREE.Vector3(pl.o[0], pl.o[1], pl.o[2]);
    const lNow = lCar.lerp(lRob, al);
    const carried = new THREE.Vector3(
      bm[0][0]*lNow.x + bm[0][1]*lNow.y + bm[0][2]*lNow.z + so.head[0],
      bm[1][0]*lNow.x + bm[1][1]*lNow.y + bm[1][2]*lNow.z + so.head[1] + lift,
      bm[2][0]*lNow.x + bm[2][1]*lNow.y + bm[2][2]*lNow.z + so.head[2]);
    let pos = cPos.clone().lerp(carried, grip);
    /* clone FIRST. Quaternion.slerp mutates in place, so this line used to
     * rotate cQ itself toward the panel's robot seat -- and everything that
     * read cQ afterwards as "the car orientation" got a panel turned up to
     * 109 degrees. That was every dissolving panel spinning about its own
     * centre while it floated: the globally rotated elements. */
    let qt  = cQ.clone().slerp(rQ, at);
    let kk  = 1+(kb-1)*al;
    if(pl.st){
      /* RIDES ITS BONE, frozen in the bone's frame.
       *
       * The carried path re-derives a panel's offset from the bone EVERY
       * frame, so without travel it is simply pinned to the car -- and with
       * travel it heads for a seat that, while folded, is down inside the
       * car: the roof sank from y=110 to 56 before the body had moved. That
       * was the "sucked in". A shell part now takes its offset from the fold's
       * starting pose once, and afterwards moves only because its bone moves:
       * still while the body is still, rising and turning with it after. */
      /* FOLLOWS THE CORE'S POSITION, NOT ITS ROTATION.
       *
       * Two rules failed first. The cladding table's bone was chosen for
       * where ARMOUR ends up; a dissolving part never ends up anywhere, and
       * it put the Falcon's front bumper on the left shin, 316 units away --
       * a 316-unit arc every time the shin moved. The nearest bone was
       * better but landed rear panels on the shins and side windows on the
       * hands, the bones that flail most.
       *
       * Rotation is what turns a lever arm into a whip. So a dissolving part
       * picks the nearest point on the TORSO in the folded pose and follows
       * that point's travel, keeping its own orientation: it rises with the
       * body and compacts, and cannot swing, whatever the distance. */
      const R0=ref0();
      if(pl._core===undefined){
        let best=null, bd=Infinity, bf=0;
        for(const bn of ['pelvis','spine','chest','neck','head']){
          const J=R0.s[bn]; if(!J) continue;
          const a=[J.head[0],J.head[1]+R0.lift,J.head[2]], c=[J.tail[0],J.tail[1]+R0.lift,J.tail[2]];
          const ab=[c[0]-a[0],c[1]-a[1],c[2]-a[2]];
          const L2=ab[0]*ab[0]+ab[1]*ab[1]+ab[2]*ab[2];
          const ap=[vPos.x-a[0],vPos.y-a[1],vPos.z-a[2]];
          const f=L2>1e-9?Math.max(0,Math.min(1,(ap[0]*ab[0]+ap[1]*ab[1]+ap[2]*ab[2])/L2)):0;
          const d=Math.hypot(ap[0]-ab[0]*f, ap[1]-ab[1]*f, ap[2]-ab[2]*f);
          if(d<bd){bd=d;best=bn;bf=f;}
        }
        pl._core=best; pl._cf=bf;
        const J=R0.s[best];
        pl._p0=[J.head[0]+(J.tail[0]-J.head[0])*bf,
                J.head[1]+R0.lift+(J.tail[1]-J.head[1])*bf,
                J.head[2]+(J.tail[2]-J.head[2])*bf];
      }
      if(pl._core && s[pl._core]){
        const J=s[pl._core], f=pl._cf;
        const pt=[J.head[0]+(J.tail[0]-J.head[0])*f,
                  J.head[1]+lift+(J.tail[1]-J.head[1])*f,
                  J.head[2]+(J.tail[2]-J.head[2])*f];
        pos=cPos.clone().add(new THREE.Vector3(pt[0]-pl._p0[0],pt[1]-pl._p0[1],pt[2]-pl._p0[2]));
        qt=cQ.clone();
        pl._anchorW=pt;
      }
      kk=1;
      if(RIG.car.cubes===false){
        /* SKINNED: the panel mesh stays at its car pose and every fragment is
         * moved by its own bone in the shader. Hand each bone's motion over in
         * this panel's local space: q_l = qi*dq*q, t_l = qi*(dq*p + dt - p). */
        // exactly the pose the fragments were bound in: no crack, no tilt
        pos=vPos.clone(); qt=vQ.clone();
        const qi=qt.clone().invert(), ud=carMeshes[g].userData;
        ud.bq=ud.bq||[]; ud.bt=ud.bt||[];
        for(let i=0;i<BD.length;i++){
          const [dq,dt]=BD[i];
          const ql=qi.clone().multiply(dq).multiply(qt);
          const tl=pos.clone().applyQuaternion(dq).add(dt).sub(pos).applyQuaternion(qi);
          ud.bq[i]=[ql.x,ql.y,ql.z,ql.w]; ud.bt[i]=[tl.x,tl.y,tl.z,0];
          if(msh.userData.sh){
            msh.userData.sh.uniforms.uBQ.value[i].set(ql.x,ql.y,ql.z,ql.w);
            msh.userData.sh.uniforms.uBT.value[i].set(tl.x,tl.y,tl.z,0);
          }
        }
      }
    }
    /* HINGED: a folding part never detaches. Its root line is bound to its
     * bone at the fold; the hinge point eases from where it sat on the vehicle
     * to its seat on the body, the part swings about that line by up to the
     * fold angle, and everything rides the bone's own motion since the fold.
     * Scaled about the root, so the hinge edge stays where it is. */
    if(pl.hinge){
      const [hb, seat, deg, root, axis, hsize]=pl.hinge, R0=ref0(), J0=R0.s[hb], J=s[hb];
      // a folding part keeps its own size rather than being fitted to a bone
      if(hsize) kk=1+(hsize-1)*at;
      if(J0 && J){
        const qOf=m=>new THREE.Quaternion().setFromRotationMatrix(new THREE.Matrix4().set(
          m[0][0],m[0][1],m[0][2],0, m[1][0],m[1][1],m[1][2],0, m[2][0],m[2][1],m[2][2],0, 0,0,0,1));
        const r0=new THREE.Vector3(root[0], root[1]+RIG.floor, root[2]);
        if(!pl._hl){ const d=[r0.x-J0.head[0], r0.y-(J0.head[1]+R0.lift), r0.z-J0.head[2]];
          pl._hl=[0,1,2].map(j=>J0.m[0][j]*d[0]+J0.m[1][j]*d[1]+J0.m[2][j]*d[2]); }
        const Bn=RIG.bones.find(x=>x.n===hb);
        const ls=[seat[0]+Bn.d[0]*seat[1]*Bn.l, Bn.d[1]*seat[1]*Bn.l, seat[2]+Bn.d[2]*seat[1]*Bn.l];
        const l=[0,1,2].map(k=>pl._hl[k]+(ls[k]-pl._hl[k])*at);
        const rw=new THREE.Vector3(
          J.m[0][0]*l[0]+J.m[0][1]*l[1]+J.m[0][2]*l[2]+J.head[0],
          J.m[1][0]*l[0]+J.m[1][1]*l[1]+J.m[1][2]*l[2]+J.head[1]+lift,
          J.m[2][0]*l[0]+J.m[2][1]*l[1]+J.m[2][2]*l[2]+J.head[2]);
        const Q=qOf(J.m).multiply(qOf(J0.m).invert())
          .multiply(new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(axis[0],axis[1],axis[2]), deg*at*Math.PI/180));
        pos=rw.clone().add(vPos.clone().sub(r0).multiplyScalar(kk).applyQuaternion(Q));
        qt=Q;
      }
    }
    if(pl.f==='armour'){
      // END FOR END, ramping in as it attaches (unchanged on the vehicle)
      if(pl.flip) qt.multiply(new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0,0,1), Math.PI*at));
      // SPINNING WEAPON: about the wheel's own axle (car-lateral z in its
      // geometry), ramping in as it attaches
      /* SPIN as an ACCUMULATED angle: the rate changes (a saw spins up to 4x
       * when it attacks), and rate x clock jumps whenever the rate does */
      const W=WEAPON_OF[g], wdep=W?MOTION_FX.weapon:0, watk=W?MOTION_FX.attack:0;
      if(pl.spin){
        const rate=pl.spin*at*(1+3*(W&&W.mode==='saw'?watk*wdep:0));
        pl._ang=(pl._ang||0)+rate*(lastDt||0.016);
        qt.multiply(new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0,0,1), pl._ang));
      }
      /* WEAPON DEPLOY: the part slides along its bone toward the hand (over
       * the retracting fist, or past it); a cannon kicks back as it fires; a
       * staff grows along its length. */
      if(W && wdep>0){
        const J=s[pl.b], d=[J.tail[0]-J.head[0],J.tail[1]-J.head[1],J.tail[2]-J.head[2]], L=Math.hypot(...d)||1;
        const Bn=RIG.bones.find(x=>x.n===pl.b);
        const slide=(W.slide||0.3)*Bn.l*wdep - (W.mode==='cannon'?14*watk:0);
        pos.add(new THREE.Vector3(d[0]/L*slide, d[1]/L*slide, d[2]/L*slide));
        if(W.mode==='staff') kk*=1+0.45*wdep;
      }
      /* WEDGE: where this piece's path runs into another piece's material,
       * a baked nudge (in its own frame) seats it in that piece's gaps --
       * orion's nose tips drove 35-91% of the way through the front wheels. */
      if(pl.wedge && pl.wedge.length){
        const c=pl.wedge; let o=null;
        if(t>=c[0][0] && t<=c[c.length-1][0]){
          for(let i=0;i<c.length-1;i++) if(t>=c[i][0]&&t<=c[i+1][0]){
            const f=(t-c[i][0])/((c[i+1][0]-c[i][0])||1); o=[1,2,3].map(k=>c[i][k]+(c[i+1][k]-c[i][k])*f); break; }
          if(!o) o=c[c.length-1].slice(1);
        }
        if(o) pos.add(new THREE.Vector3(o[0],o[1],o[2]).multiplyScalar(kk).applyQuaternion(qt));
      }
      /* NEVER BELOW THE FLOOR on its way to its seat: the rear wheels dipped
       * to y=-12 as the body started to rise. The panel's lowest point is its
       * half-extents projected onto world up through its rotation. */
      const r=halfOf(g);
      if(r){
        const e=new THREE.Matrix4().makeRotationFromQuaternion(qt).elements;
        const hy=(Math.abs(e[1])*r[0]+Math.abs(e[5])*r[1]+Math.abs(e[9])*r[2])*kk;
        if(pos.y-hy<RIG.floor) pos.y=RIG.floor+hy;
      }
    }
    if(pl.mo){
      const ax=i=>new THREE.Vector3(i===0?1:0, i===1?1:0, i===2?1:0);
      const ud=carMeshes[g].userData;
      ud.mo=[pl.mo[0], at, pl.mo[1], pl.mo[2], pl.mo[3], pl.mo[4], pl.mo[5], pl.mo[6]];
      if(msh.userData.sh){ const U=msh.userData.sh.uniforms;
        U.uMorph.value=pl.mo[0]; U.uMorphT.value=at;
        U.uAxL.value.copy(ax(pl.mo[1])); U.uAxM.value.copy(ax(pl.mo[2])); U.uAxT.value.copy(ax(pl.mo[3]));
        U.uHalf.value.set(pl.mo[4], pl.mo[5], pl.mo[6]); }
    }
    carMeshes[g].matrix.compose(pos, qt, new THREE.Vector3(kk,kk,kk));
    if(carCubes[g]){
      const uu=carMeshes[g].userData.u||0;
      const aw=pl._anchorW ? new THREE.Vector3(pl._anchorW[0],pl._anchorW[1],pl._anchorW[2]) : null;
      let seat=null, ks=1;
      if(pl.keep && s[pl.keep[0]]){
        const [bn, frac, dx, dz, kscale]=pl.keep, J=s[bn], M=J.m;
        const B=RIG.bones.find(x=>x.n===bn);
        const bk=(bulk&&bulk[bn])?(Array.isArray(bulk[bn])?bulk[bn][0]:bulk[bn]):1;
        const r=(RIG.radius[bn]!==undefined?RIG.radius[bn]:RIG.default_r)*bk;
        const dl=Math.hypot(dx,dz)||1;
        const out=r+carCubes[g].half*kscale+2;       // on the surface, not in it
        const loc=[B.d[0]*frac*B.l+dx/dl*out, B.d[1]*frac*B.l, B.d[2]*frac*B.l+dz/dl*out];
        seat=new THREE.Vector3(
          M[0][0]*loc[0]+M[0][1]*loc[1]+M[0][2]*loc[2]+J.head[0],
          M[1][0]*loc[0]+M[1][1]*loc[1]+M[1][2]*loc[2]+J.head[1]+lift,
          M[2][0]*loc[0]+M[2][1]*loc[1]+M[2][2]*loc[2]+J.head[2]);
        ks=kscale;
      }
      updateCubes(carCubes[g], uu, pos, qt, performance.now()*0.001, aw, seat, ks);
      // the mesh gives way once its cubes cover it
      if(uu>0.15) carMeshes[g].visible=false;
    }
  }
  /* RANGED -- placed AFTER the car parts, because it reads where the weapon
   * part is THIS frame (before them, it read last frame's: 436 units off on a
   * jump in time). Barrels pop out (overshooting a little, like a spring), shots fly
   * from their muzzles along the aim, slugs dropping under gravity. The aim is
   * the forearm's direction; with the arm held on aim, muzzle_now + v*age is
   * where a shot fired `age` ago is now. */
  { const W=(RIG.weapons||[])[0], dep=MOTION_FX.ranged||0;
    [hubBarrel,palmBarrel,flashPart,flashPalm,...shotPool].forEach(m=>m.visible=false);
    if(W && dep>0.001){
      const L3=(a,b)=>Math.hypot(b[0]-a[0],b[1]-a[1],b[2]-a[2]);
      const fa=s['forearm_'+W.side], hd=s['hand_'+W.side];
      const d0=[fa.tail[0]-fa.head[0],fa.tail[1]-fa.head[1],fa.tail[2]-fa.head[2]], dl=Math.hypot(...d0)||1;
      const dir=new THREE.Vector3(d0[0]/dl,d0[1]/dl,d0[2]/dl);
      const pop=x=>{ x=Math.max(0,Math.min(1,x)); return 1-Math.pow(1-x,3)+0.18*Math.sin(Math.PI*x); };
      const up=new THREE.Vector3(0,1,0);
      const place=(m,base,len,rad)=>{ m.visible=len>1; if(!m.visible) return;
        m.position.copy(base); m.quaternion.setFromUnitVectors(up,dir); m.scale.set(rad,len,rad); };
      // the PART's muzzle: out of a wheel's hub along the aim, or a cannon's / staff's own tip
      const pm=carMeshes[W.part]; pm.geometry.computeBoundingBox(); const bb=pm.geometry.boundingBox;
      const ctr=bb.getCenter(new THREE.Vector3()).applyMatrix4(pm.matrix);
      let partMuzzle;
      if(W.mode==='saw'){ const len=34*pop(dep), kick=6*MOTION_FX.kickPart;
        place(hubBarrel, ctr.clone().addScaledVector(dir,-kick), len, 3.2);
        partMuzzle=ctr.clone().addScaledVector(dir,len-kick); }
      else { const ext=[bb.max.x-bb.min.x,bb.max.y-bb.min.y,bb.max.z-bb.min.z], ax=ext.indexOf(Math.max(...ext));
        const ends=[0,1].map(e=>{ const v=bb.getCenter(new THREE.Vector3()); v.setComponent(ax,e?bb.max.getComponent(ax):bb.min.getComponent(ax)); return v.applyMatrix4(pm.matrix); });
        partMuzzle=ends[0].dot(dir)>ends[1].dot(dir)?ends[0]:ends[1]; }
      // the HAND's blaster: out of the knuckles, along the aim
      const hbase=new THREE.Vector3(hd.tail[0],hd.tail[1]+lift,hd.tail[2]);
      const plen=22*pop((dep-0.2)/0.8), pk=4*MOTION_FX.kickPalm;
      place(palmBarrel, hbase.clone().addScaledVector(dir,-pk), plen, 2.2);
      const palmMuzzle=hbase.clone().addScaledVector(dir,plen-pk);
      // flashes
      const fl=(m,at,k)=>{ m.visible=k>0.15; if(!m.visible) return; m.position.copy(at); m.scale.set(0.8,45*k,0.8);
        m.quaternion.setFromUnitVectors(new THREE.Vector3(0,-1,0),dir); };
      fl(flashPart,partMuzzle,MOTION_FX.kickPart); fl(flashPalm,palmMuzzle,MOTION_FX.kickPalm);
      // shots
      const R=MOTION.RANGED, g=MOTION.P.g;
      (MOTION_FX.shots||[]).slice(0,shotPool.length).forEach((sh,i)=>{
        const m=shotPool[i], slug=sh.src==='part', v=slug?R.slugV:R.boltV, from=slug?partMuzzle:palmMuzzle;
        const at=from.clone().addScaledVector(dir,v*sh.age); if(slug) at.y-=0.5*g*sh.age*sh.age;
        const vel=dir.clone().multiplyScalar(v); if(slug) vel.y-=g*sh.age;
        m.material=slug?slugMat:boltMat; m.visible=true; m.position.copy(at);
        m.quaternion.setFromUnitVectors(new THREE.Vector3(0,-1,0), vel.clone().normalize().negate());
        m.scale.set(slug?2.4:1.8, slug?16:30, slug?2.4:1.8);
      });
    } }

  // muscles: length from the pose, radius from volume conservation
  (RIG.muscles||[]).forEach((mu,i)=>{
    const A=s[mu.a], B=s[mu.b];
    if(!A||!B){ musMesh[i].visible=false; return; }
    const wp=(so,lp)=>[0,1,2].map(r=>
      so.m[r][0]*lp[0]+so.m[r][1]*lp[1]+so.m[r][2]*lp[2]+so.head[r]);
    const p0=wp(A,mu.ap), p1=wp(B,mu.bp);
    const d=[p1[0]-p0[0],p1[1]-p0[1],p1[2]-p0[2]];
    const L=Math.hypot(d[0],d[1],d[2]);
    if(L<1e-4){ musMesh[i].visible=false; return; }
    if(musRest[mu.n]===undefined) musRest[mu.n]=L;
    const rr=mu.r*Math.sqrt(Math.max(0.25,Math.min(4,musRest[mu.n]/L)));
    const up=new THREE.Vector3(0,1,0);
    const dir=new THREE.Vector3(d[0]/L,d[1]/L,d[2]/L);
    const q=new THREE.Quaternion().setFromUnitVectors(up,dir);
    const base=new THREE.Vector3(p0[0],p0[1]+lift,p0[2]);
    // the rod spans the whole actuator; the links ride it and BUNCH as it
    // contracts, because their count is fixed and the spacing is not
    musMesh[i].rod.visible=showMus;
    musMesh[i].rod.matrix.compose(
      base.clone().addScaledVector(dir,L*0.5), q,
      new THREE.Vector3(mu.r*0.55,L,mu.r*0.55));
    const seg=L/LINKS_PER;
    for(let j=0;j<LINKS_PER;j++){
      const m=musMesh[i].links[j];
      m.visible=showMus;
      m.matrix.compose(base.clone().addScaledVector(dir, seg*j+seg*0.06), q,
                       new THREE.Vector3(rr, seg*0.88, rr));
    }
  });
  // scales arrive as the panels reclaim: each has its own phase so a limb
  // clads in a wave from the joint outward rather than all at once
  for(const b in scaleMesh){
    const so=s[b];
    const list=RIG.scales[b];
    for(let i=0;i<list.length;i++){
      const sc=list[i], m=scaleMesh[b][i];
      if(!so){ m.visible=false; continue; }
      // start at 0.24, not 0.30: the panels are 39% gone by 0.40 and the
      // scales were not arriving until 0.50, leaving the machine at its
      // barest exactly when it is most visible
      const on=Math.max(0,Math.min(1,(t-0.24-sc.t*0.30)/0.20));
      if(on<=0.001||!showScale){ m.visible=false; continue; }
      m.visible=true;
      const e=on*on*(3-2*on);
      // lift off the limb as it seats, so it lands rather than fades in
      const g=1.0+(1.0-e)*0.55;
      const p=sc.p, n=sc.n, z=sc.s;
      const lx=p[0]*g, ly=p[1], lz=p[2]*g;
      const wx=so.m[0][0]*lx+so.m[0][1]*ly+so.m[0][2]*lz+so.head[0];
      /* never below the floor: a plate seats from 1.55x its radius, and on
       * the underside of a limb folded prone that pointed straight down --
       * to y=-14 at t=0.35 */
      // and while folded, never below the car's shell: the robot is held up
      // inside it then, so its floor is the shell's bottom edge
      const wy=Math.max(RIG.floor+1.5+foldLift(t), so.m[1][0]*lx+so.m[1][1]*ly+so.m[1][2]*lz+so.head[1]+lift);
      const wz=so.m[2][0]*lx+so.m[2][1]*ly+so.m[2][2]*lz+so.head[2];
      const nx=so.m[0][0]*n[0]+so.m[0][1]*n[1]+so.m[0][2]*n[2];
      const ny=so.m[1][0]*n[0]+so.m[1][1]*n[1]+so.m[1][2]*n[2];
      const nz=so.m[2][0]*n[0]+so.m[2][1]*n[1]+so.m[2][2]*n[2];
      /* A FULL FRAME, not just a normal.
       *
       * setFromUnitVectors constrains one axis and leaves the roll free, so
       * every scale was rotated arbitrarily about its own normal -- the
       * armour read as a pinecone instead of a shell, and symmetric pairs
       * rolled opposite ways because the arbitrary choice differed per
       * scale.
       *
       * A scale needs all three: its normal points out, its LENGTH runs along
       * the bone, and its width goes around. Building the basis explicitly is
       * the only way to say that.
       */
      const nV=new THREE.Vector3(nx,ny,nz).normalize();
      // the bone's own axis, in world
      const bd=RIG.bones.find(x=>x.n===b).d;
      const aV=new THREE.Vector3(
        so.m[0][0]*bd[0]+so.m[0][1]*bd[1]+so.m[0][2]*bd[2],
        so.m[1][0]*bd[0]+so.m[1][1]*bd[1]+so.m[1][2]*bd[2],
        so.m[2][0]*bd[0]+so.m[2][1]*bd[1]+so.m[2][2]*bd[2]).normalize();
      // make it perpendicular to the normal so the basis is orthonormal
      aV.addScaledVector(nV, -aV.dot(nV));
      if (aV.lengthSq() < 1e-8) aV.set(0,1,0).addScaledVector(nV,-nV.y);
      aV.normalize();
      const xV=new THREE.Vector3().crossVectors(aV, nV).normalize();
      const basis=new THREE.Matrix4().makeBasis(xV, aV, nV);
      const q=new THREE.Quaternion().setFromRotationMatrix(basis);
      m.matrix.compose(new THREE.Vector3(wx,wy,wz), q,
                       new THREE.Vector3(z[0]*e, z[1]*e, Math.max(0.6,z[2])));
    }
  }
  const an=analyse(lifted);
  comMesh.position.set(an.com[0],an.com[1],an.com[2]);
  const stable=an.margin>0;
  comMesh.material.color.setHex(stable?0x7fc79a:0xe0a458);
  hullLine.geometry.dispose();
  const hp=an.hull.length>=3?an.hull.concat([an.hull[0]]):[];
  hullLine.geometry=new THREE.BufferGeometry().setFromPoints(
    hp.map(q=>new THREE.Vector3(q[0],1.5,q[1])));
  hullLine.material.color.setHex(stable?0x7fc79a:0xe0a458);
  for(const b of volBoxes) b.visible=showVol;

  const rows=document.getElementById('rows'); rows.innerHTML='';
  for(const n of ['head','chest','pelvis','hand_l','foot_l','foot_r']){
    const v=lifted[n].tail;
    const tr=document.createElement('tr');
    tr.innerHTML='<td>'+n+'</td><td class="'+(v[1]<16?'g':'n')+'">'+v[1].toFixed(0)+
                 '</td><td class="n">'+v[2].toFixed(0)+'</td>';
    rows.appendChild(tr);
  }
  let out=0, lo=[1e9,1e9,1e9], hi=[-1e9,-1e9,-1e9];
  for(const k in lifted) for(const q of [lifted[k].head,lifted[k].tail]){
    for(let a=0;a<3;a++){lo[a]=Math.min(lo[a],q[a]);hi[a]=Math.max(hi[a],q[a]);}
    let ok=false;
    for(const v of RIG.volumes)
      if(q[0]>=v.x[0]&&q[0]<=v.x[1]&&q[1]>=v.y[0]&&q[1]<=v.y[1]&&
         q[2]>=v.z[0]&&q[2]<=v.z[1]){ok=true;break;}
    if(!ok) out++;
  }
  const mtxt=isNaN(an.margin)
    ? '<span class="w">'+an.hull.length+' contact pt(s)</span>'
    : (stable?'<span class="g">margin '+an.margin.toFixed(0)+'</span>'
             :'<span class="w">tips by '+(-an.margin).toFixed(0)+'</span>');
  document.getElementById('note').innerHTML=
    'extent '+(hi[0]-lo[0]).toFixed(0)+' \u00d7 '+(hi[1]-lo[1]).toFixed(0)+
    ' \u00d7 '+(hi[2]-lo[2]).toFixed(0)+
    (t<0.02?(out===0?' <span class="g">fits the tunnel</span>'
                    :' <span class="w">'+out+' joints outside</span>'):'')+
    '<br>CoM '+an.com[0].toFixed(0)+', '+an.com[1].toFixed(0)+' \u00b7 '+mtxt+
    (Math.abs(sp)>0.5?'<br><span class="k">torso spun '+sp.toFixed(0)+'\u00b0</span>':'');
}
tIn.oninput=refresh;

const pb=document.getElementById('poses');
RIG.poses.forEach((p,i)=>{
  const b=document.createElement('button'); b.textContent=p.n;
  b.onclick=()=>{
    // jump to where this pose actually sits, given weighted segments
    const W=RIG.weights, tot=W.reduce((a,b)=>a+b,0);
    let acc=0; for(let k=0;k<i;k++) acc+=W[k]/tot;
    tIn.value=Math.min(1,acc); refresh(); playing=false;
    document.getElementById('play').classList.remove('on');
  };
  pb.appendChild(b);
});
/* A toggle whose button is missing used to throw and take the whole viewer
 * down -- one control added to the script and not to the markup, and nothing
 * rendered at all. A missing control should cost you that control. */
const tog=(id,get,set)=>{
  const el=document.getElementById(id);
  if(!el){ console.warn('no button #'+id+'; toggle disabled'); return; }
  el.onclick=()=>{set(!get());el.classList.toggle('on',get());refresh();};
};
document.getElementById('play').onclick=e=>{playing=!playing;
  e.target.classList.toggle('on',playing);};
document.getElementById('mode').onclick=()=>{
  const order=['ground', ...(RIG.poses_air?['air']:[]), ...(RIG.poses_zerog?['zerog']:[])];
  setMode(order[(order.indexOf(MODE)+1)%order.length]); refresh(); };
{ const sel=document.getElementById('motion');
  const phys=()=>{ MOTION.P.g=981*(+document.getElementById('grav').value);
    MOTION.P.mass=+document.getElementById('mass').value; MOTION.P.thrust=+document.getElementById('thr').value;
    const lp=document.getElementById('limp'); if(lp) MOTION.P.limp.amount=+lp.value;
    const gt=document.getElementById('gait'); if(gt) MOTION.P.gait=+gt.value;
    // ZERO GRAVITY: a robot that does not hover reuses the levitate-and-spin
    // transform, and floats once transformed
    const zg=MOTION.P.g<0.05*981;
    // the physics never sees g = 0 (jump height divides by it); zero-g is a MODE
    MOTION.P.g=Math.max(MOTION.P.g, 0.02*981);
    if(zg && MODE!=='zerog'){ setMode('zerog'); const sel=document.getElementById('motion'); if(sel&&(!MOTION_ON||['walk','run','locomote','jump','dash'].includes(MOTION_ON))){ sel.value='float'; MOTION_ON='float'; } refresh(); }
    else if(!zg && MODE==='zerog' && !RIG.hover){ setMode(RIG.poses_air?'air':'ground'); refresh(); } };
  ['grav','mass','thr','limp','gait'].forEach(id=>{ const el=document.getElementById(id); if(el) el.oninput=phys; });
  if(sel) sel.onchange=()=>{ MOTION_ON=sel.value; MOTION_T0=performance.now(); phys();
    if(MOTION_ON){ tIn.value=1; playing=false; document.getElementById('play').classList.remove('on'); }
    const hud=document.getElementById('mhud'); if(hud&&!MOTION_ON) hud.textContent='';
    refresh(); }; }
setMode(RIG.poses_air?'air':'ground');
// a floating robot floats once transformed: its motion defaults to 'float'
if(RIG.hover){ const sel=document.getElementById('motion'); if(sel){ sel.value='float'; } MOTION_ON='float'; MOTION_T0=performance.now(); }
document.getElementById('spin').onclick=e=>{spinning=!spinning;
  e.target.classList.toggle('on',spinning);};
tog('bodyb',()=>showBody,v=>showBody=v);
tog('bones',()=>showBones,v=>showBones=v);
tog('axes',()=>showVol,v=>showVol=v);
tog('core',()=>showCore,v=>showCore=v);
tog('car',()=>showCar,v=>showCar=v);
document.getElementById('fit').onclick=()=>{userZoom=1.0;};
tog('heft',()=>dynOn,v=>dynOn=v);
tog('mus',()=>showMus,v=>showMus=v);
tog('scl',()=>showScale,v=>showScale=v);

let dir=1;
function frame(){
  requestAnimationFrame(frame);
  const now=performance.now();
  lastDt=Math.min(0.05,(now-lastT)/1000); lastT=now;
  // the followers keep moving after the slider stops, which is the settle
  if(dynOn && !playing) refresh();
  else if(MOTION_ON && +tIn.value>=0.9) refresh();
  if(spinning) yaw+=0.004;
  if(playing){
    let v=+tIn.value+0.0035*dir;
    if(v>=1){v=1;dir=-1;} else if(v<=0){v=0;dir=1;}
    tIn.value=v; refresh();
  }
  const d=frameR*userZoom;
  // No extra height term. The old +d*0.10 tilted the camera up and pushed the
  // subject toward the bottom of the frame, which is most of why it read as
  // "too low" -- the framing was right and the aim was not.
  camera.position.set(focus.x+Math.sin(yaw)*Math.cos(pitch)*d,
                      focus.y+Math.sin(pitch)*d,
                      focus.z+Math.cos(yaw)*Math.cos(pitch)*d);
  camera.lookAt(focus);
  renderer.render(scene,camera);
}
resize(); refresh(); frame();
loadCar().then(refresh).catch(e=>{
  document.getElementById('note').innerHTML +=
    '<br><span class="w">car geometry failed: '+e.message+'</span>';
  console.error(e);
});
}catch(e){ fail(e); }
</script></body></html>
'''


if __name__ == '__main__':
    rig, body, car_b64, car_meta = spec()
    html = (HTML.replace('__RIG__', json.dumps(rig, separators=(',', ':')))
                .replace('__BODY__', json.dumps(body, separators=(',', ':')))
                .replace('__CARB64__', car_b64)
                .replace('__CARMETA__', json.dumps(car_meta, separators=(',', ':'))))
    # Every getElementById must resolve against the markup. A control added
    # to the script and not to the bar threw on load and took the whole
    # viewer down -- a five-character omission for a blank page. Checked at
    # build time now rather than discovered in the browser.
    import re as _re

    # ---- reference check -------------------------------------------------
    # Three separate breakages have shipped from editing this generated page:
    # a deleted scene block, a toggle whose button did not exist, and a const
    # whose declaration was never inserted. All three were a string replace
    # that silently did not match.
    #
    # So: pull the script out, strip comments and string literals (the base64
    # blob is megabytes of capital letters and confuses any naive scan), and
    # check every identifier the code USES against the ones it DECLARES.
    _js = html[html.index('const RIG = '):html.rindex('</script>')]
    _clean = _re.sub(r'/\*.*?\*/', ' ', _js, flags=_re.S)
    _clean = _re.sub(r'//[^\n]*', ' ', _clean)
    _clean = _re.sub(r'"(?:[^"\\]|\\.)*"', '""', _clean)
    _clean = _re.sub(r"'(?:[^'\\]|\\.)*'", "''", _clean)
    _clean = _re.sub(r'`(?:[^`\\]|\\.)*`', '``', _clean)
    # comma-separated declarations too: `const bMesh={}, jMesh={}` declares
    # both, and matching only the first name produced false positives, which
    # is worse than no check -- a warning you learn to ignore is noise.
    _declared = set()
    for _decl in _re.findall(
            r'\b(?:const|let|var)\s+([^;\n]+)', _clean):
        depth = 0; cur = ''
        for ch in _decl:
            if ch in '([{': depth += 1
            elif ch in ')]}': depth -= 1
            if ch == ',' and depth == 0:
                _declared.add(cur.strip().split('=')[0].strip()); cur = ''
            else:
                cur += ch
        _declared.add(cur.strip().split('=')[0].strip())
    _declared |= set(_re.findall(
        r'\b(?:function|class)\s+([A-Za-z_$][\w$]*)', _clean))
    _declared = {d for d in _declared
                 if _re.fullmatch(r'[A-Za-z_$][\w$]*', d or '')}
    # parameters, destructuring and loop variables
    for pat in (r'\(([^()]*)\)\s*=>', r'([A-Za-z_$][\w$]*)\s*=>',
                r'function[^(]*\(([^)]*)\)',
                r'for\s*\(\s*(?:const|let|var)\s+([\w$]+)',
                r'catch\s*\(\s*([\w$]+)', r'\{([^{}]*)\}\s*=' ):
        for m in _re.findall(pat, _clean):
            for part in _re.split(r'[,:\s]+', m):
                part = part.strip()
                if part and _re.fullmatch(r'[A-Za-z_$][\w$]*', part):
                    _declared.add(part)
    _BUILTIN = {
        'THREE','Math','JSON','Object','Array','Number','String','Boolean',
        'Set','Map','Float32Array','Uint8Array','Uint16Array','Uint32Array',
        'Int8Array','DataView','ArrayBuffer','Blob','Response','Promise',
        'DecompressionStream','ResizeObserver','console','document','window',
        'performance','requestAnimationFrame','addEventListener','devicePixelRatio',
        'atob','btoa','Infinity','NaN','undefined','null','true','false',
        'this','arguments','isFinite','isNaN','parseFloat','parseInt','Error',
        'setTimeout','fetch','navigator',
    }
    _used = set(_re.findall(r'(?<![.\w$])([A-Za-z_$][\w$]*)\s*(?![\w$(]*\s*:)',
                            _clean))
    _kw = {'const','let','var','function','return','if','else','for','while',
           'new','of','in','typeof','instanceof','continue','break','try',
           'catch','throw','class','extends','delete','void','do','switch',
           'case','default','await','async','yield','get','set','static'}
    _missing = sorted(i for i in _used
                      if i not in _declared and i not in _BUILTIN
                      and i not in _kw and not i.isupper() or
                      (i.isupper() and i not in _declared and i not in _BUILTIN
                       and len(i) > 2 and i not in _kw and
                       _re.search(r'(?<![.\w$])' + _re.escape(i) + r'\s*[.\[)]', _clean)))
    _missing = [i for i in _missing if _re.search(
        r'(?<![.\w$])' + _re.escape(i) + r'\s*(?:[.\[]|\()', _clean)]
    if _missing:
        print(f"  warning: possibly undeclared: {', '.join(_missing[:12])}")

    # ARM CLEARANCE. Solved angles go stale whenever the skeleton changes --
    # the last solve predated SCALE=1.40 and the thicker radii, and four of
    # six poses had drifted back into contact, one to 0.9 units, which is a
    # hand inside a leg. Checked every build rather than remembered.
    import orion_fixpose as _FP
    _ms = BD.body_meshes(rough=False, studs=False)
    _tight = []
    for _n, _p in R.POSES:
        _sol = R.solve(_p)
        _d = min(_FP.torso_clearance(_sol, _x, _ms)
                 for _x in ('hand_l', 'hand_r', 'forearm_l', 'forearm_r'))
        if _d < 3.0:
            _tight.append(f"{_n} {_d:.1f}")
    if _tight:
        print(f"  warning: arms within 3 units of the body in {', '.join(_tight)}"
              f" -- re-run the clearance solve")

    # geometry and placements must cover the same set, or the viewer throws
    # on the first group it cannot place
    _gm = {m['g'] for m in car_meta}
    _gp = set(rig['car']['place'])
    if _gm != _gp:
        raise SystemExit(
            f"build aborted: {len(_gm - _gp)} geometry groups have no placement "
            f"and {len(_gp - _gm)} placements have no geometry")

    ids = set(_re.findall(r'id="([A-Za-z0-9_-]+)"', html))
    used = set(_re.findall(r"getElementById\('([A-Za-z0-9_-]+)'\)", html))
    missing = sorted(used - ids)
    if missing:
        raise SystemExit(f"build aborted: script references #{', #'.join(missing)} "
                         f"but the markup has no such element")

    html = html.replace('__TITLE__', f"{VEHICLE.capitalize()} rig")
    out = __import__('rigpaths').out('rig-viewer.html' if VEHICLE == 'orion'
                                     else f'rig-viewer-{VEHICLE}.html')
    open(out, 'w').write(html)
    print(f"{len(html)} bytes -> {out}")
    print(f"  {len(used)} DOM ids referenced, all present")
    print(f"  {len(rig['bones'])} bones, {len(rig['poses'])} poses, "
          f"{sum(len(b['f'])//3 for b in body.values())} body triangles")
