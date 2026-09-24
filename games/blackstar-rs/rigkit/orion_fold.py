#!/usr/bin/env python3
"""orion_fold.py — author a transformation for orion, from its own geometry.

## Why this is a script and not a hand-written table

orion has 387 islands, one node, one material. Nothing about which part is
which is authored, so every assignment here is derived from position and
shape and then STATED, so it can be argued with. Change a rule, re-run, see
the whole fold change.

## The design

The anatomy, measured (x runs nose at -229 to tail at +229, y up, z is the
mirror plane):

    #16      front splitter      x -193
    #10-14   FRONT AXLE          x -138   wheels 60x60x27
    #1 #2    front cross members x -100..-76
    #0       cabin / canopy      x  -39   224x82x155
    #17      floor / chassis     x    1   379x50x186   spans the whole car
    #3/#4    side rockers        z +-83   319 long
    #27/#28  upper side panels   z +-67   128x74x20
    #8-15    REAR AXLE           x +120   wheels 60x60x27
    #5 #6    rear deck           x +113..132
    #7       rear wing           x +178

Which gives the classic car fold, and it is the only one this geometry
supports: the REAR becomes the legs (the rear axle is already where the heels
need to be), the FRONT folds under to become the arms (the front axle lands at
the elbows), the FLOOR becomes the spine, the CABIN becomes the chest, and the
SIDE PANELS rotate out into shoulder armour.

## The timing is the point

The reference is live-action film transformation choreography: nothing moves at once, every part
has its own window, and the windows OVERLAP so the machine is always doing
several things. A single eased 0..1 applied to everything is what makes a fold
read as a rig test.

Each part gets (t0, t1) and eases inside its own window. Phases:

    0.00-0.22   UNLATCH   panels lift a few degrees; wheels spin in their
                          wells. Still unmistakably a car.
    0.18-0.52   OPEN      rear deck splits, thighs swing down, the car rears
                          up on the rear axle
    0.40-0.78   RISE      floor rotates to vertical, front section folds under
    0.62-0.92   DEPLOY    arms unfold, shoulder panels settle outboard
    0.84-1.00   LOCK      head rises, small overshoot and settle

## Hinges are EDGES, not centroids

A panel that rotates up hinges on its inboard-lower edge. Pivoting a panel
about its own centre is what makes parts sweep through each other -- which is
exactly what the first workbench did.
"""
import json, math
from collections import defaultdict
import orion_anat as A

SRC = __import__('rigpaths').inp('orion.glb')


def anatomy():
    V, I = A.load(SRC)
    gs = A.islands(V, I)
    B = [A.bb(g) for g in gs]
    W = A.bb(V)
    return B, W


def classify(B, W):
    """Assign a role to every island from position and shape.

    Stated as rules so a wrong one is findable. Thresholds are fractions of
    the model, never absolute, so this survives a rescale."""
    L = W['size'][0]
    x0 = W['lo'][0]
    role = []
    for b in B:
        cx = (b['ctr'][0] - W['ctr'][0]) / L        # -0.5 nose .. +0.5 tail
        cz = b['ctr'][2]
        s = sorted(b['size'], reverse=True)
        roundish = s[0] > 1e-6 and abs(s[0]-s[1])/s[0] < 0.12 and s[2] < s[0]*0.55
        side = 'l' if cz < -W['size'][2]*0.06 else ('r' if cz > W['size'][2]*0.06 else 'c')

        if roundish and b['n'] > 300 and abs(cz) > W['size'][2]*0.25:
            # a wheel: round, big, outboard. Which axle decides which limb.
            role.append(('wheel_front_' + side) if cx < 0 else ('wheel_rear_' + side))
        elif b['n'] > 1500 and abs(cx) < 0.12 and abs(cz) < W['size'][2]*0.06 \
                and b['size'][0] > L*0.6:
            role.append('floor')                    # the spine
        elif b['n'] > 2000 and -0.25 < cx < 0.02 and side == 'c':
            role.append('cabin')                    # becomes the chest
        elif cx > 0.15 and side == 'c':
            role.append('rear_deck')                # becomes the pelvis/thighs
        elif cx < -0.28 and side == 'c':
            role.append('nose')                     # folds under, becomes chest plate
        elif abs(cz) > W['size'][2]*0.28 and b['size'][0] > L*0.25:
            role.append('rocker_' + side)           # long side beams -> shins
        elif abs(cz) > W['size'][2]*0.2:
            role.append('panel_' + side)            # shoulder armour
        elif cx > 0.05:
            role.append('rear_detail')
        elif cx < -0.15:
            role.append('front_detail')
        else:
            role.append('core')
    return role


# (t0, t1, axis, degrees, hinge, translate) per role.
# `hinge` picks a CORNER of the part's own bounding box: a tuple of
# -1/0/+1 per axis, so (-1, -1, 0) means "the nose-side, lowest edge, centred
# across the mirror". That is how a panel gets a real hinge line instead of
# pivoting about its middle.
#
# `translate` is in fractions of the whole model's extent.
PLAN = {
    # ---- UNLATCH -------------------------------------------------------
    'panel_l':        (0.00, 0.30, (1,0,0),  -34, (0,-1,+1), (0,  0.02, -0.03)),
    'panel_r':        (0.00, 0.30, (1,0,0),   34, (0,-1,-1), (0,  0.02,  0.03)),
    'front_detail':   (0.04, 0.34, (0,0,1),   22, (+1,0,0),  (0.01, 0.0, 0)),
    'rear_detail':    (0.04, 0.34, (0,0,1),  -18, (-1,0,0),  (-0.01,0.0, 0)),
    # ---- OPEN: the rear becomes the legs --------------------------------
    'rear_deck':      (0.18, 0.56, (0,0,1),  -96, (-1,-1,0), (0.04,-0.02, 0)),
    'wheel_rear_l':   (0.22, 0.60, (0,0,1),  -96, (-1,-1,0), (0.05,-0.10,-0.02)),
    'wheel_rear_r':   (0.22, 0.60, (0,0,1),  -96, (-1,-1,0), (0.05,-0.10, 0.02)),
    'rocker_l':       (0.26, 0.64, (0,0,1),  -78, (-1,-1,+1),(0.03,-0.06,-0.01)),
    'rocker_r':       (0.26, 0.64, (0,0,1),  -78, (-1,-1,-1),(0.03,-0.06, 0.01)),
    # ---- RISE: the floor stands up --------------------------------------
    'floor':          (0.40, 0.80, (0,0,1),  -84, (+1,-1,0), (0.02, 0.18, 0)),
    'core':           (0.40, 0.80, (0,0,1),  -84, (+1,-1,0), (0.02, 0.16, 0)),
    'cabin':          (0.46, 0.84, (0,0,1),  -72, (+1,-1,0), (0.06, 0.22, 0)),
    # ---- DEPLOY: the front becomes the arms ------------------------------
    'nose':           (0.58, 0.90, (0,0,1),  128, (+1,+1,0), (0.10, 0.20, 0)),
    'wheel_front_l':  (0.62, 0.94, (0,0,1),  128, (+1,+1,0), (0.12, 0.14,-0.06)),
    'wheel_front_r':  (0.62, 0.94, (0,0,1),  128, (+1,+1,0), (0.12, 0.14, 0.06)),
    # ---- LOCK ------------------------------------------------------------
    'panel_c':        (0.84, 1.00, (1,0,0),    0, (0,0,0),   (0, 0.06, 0)),
}
DEFAULT = (0.30, 0.70, (0,0,1), -40, (0,-1,0), (0.0, 0.05, 0))


def hinge_point(b, corner):
    """A corner/edge/face-centre of the part's own bounding box."""
    out = []
    for a in range(3):
        c = corner[a]
        out.append(b['lo'][a] if c < 0 else (b['hi'][a] if c > 0 else b['ctr'][a]))
    return out


def parent_small(B, W, min_verts=900):
    """Attach every small island to the nearest large one, and make it inherit
    that part's motion exactly.

    This is most of what separates a believable fold from parts drifting
    through each other. A bolt does not have its own hinge; it rides the panel
    it is bolted to. Without this, 227 of orion's 387 islands got their own
    axis and swept through their neighbours -- which is exactly what the first
    workbench looked like.

    Nearest is measured to the BOX, not the centre: a long rocker's centroid
    is far from a clip at its end, but the box is touching it."""
    big = [i for i, b in enumerate(B) if b['n'] >= min_verts]
    parent = {}
    for i, b in enumerate(B):
        if b['n'] >= min_verts:
            continue
        best, bd = None, 1e30
        for j in big:
            c = B[j]
            d = 0.0
            for a in range(3):
                lo, hi = c['lo'][a], c['hi'][a]
                v = b['ctr'][a]
                if v < lo: d += (lo - v) ** 2
                elif v > hi: d += (v - hi) ** 2
            # prefer a parent on the same side of the mirror plane: a clip
            # does not belong to the panel on the other side of the car
            if (b['ctr'][2] < 0) != (c['ctr'][2] < 0) and abs(c['ctr'][2]) > W['size'][2]*0.06:
                d += (W['size'][2] * 0.5) ** 2
            if d < bd:
                bd, best = d, j
        parent[i] = best
    return parent


def build(min_verts=900):
    B, W = anatomy()
    roles = classify(B, W)
    parent = parent_small(B, W, min_verts)
    # a child takes its parent's ROLE, so it takes its window and its hinge
    for i, pj in parent.items():
        if pj is not None:
            roles[i] = roles[pj]
    L = max(W['size'])
    parts = []
    for i, b in enumerate(B):
        r = roles[i]
        t0, t1, axis, deg, corner, tr = PLAN.get(r, DEFAULT)
        # mirror the swing for left-side parts so the two halves move together
        if r.endswith('_l') and axis == (0, 0, 1):
            pass
        pj = parent.get(i)
        hb = B[pj] if pj is not None else b
        parts.append({
            'i': i, 'role': r, 'verts': b['n'],
            'parent': pj,
            'centre': [round(v, 3) for v in b['ctr']],
            'size': [round(v, 3) for v in b['size']],
            't0': t0, 't1': t1,
            'axis': list(axis), 'deg': deg,
            # the hinge comes from the PARENT's box when this is a child, so
            # a clip swings on the panel's hinge line rather than its own
            'hinge': [round(v, 3) for v in hinge_point(hb, corner)],
            'move': [round(v * L, 3) for v in tr],
        })
    return {
        'model': 'orion',
        'extent': [round(v, 3) for v in W['size']],
        'centre': [round(v, 3) for v in W['ctr']],
        'mirror_axis': 2,
        'parts': parts,
    }


if __name__ == '__main__':
    spec = build()
    n_child = sum(1 for p in spec['parts'] if p['parent'] is not None)
    print(f"{n_child} of {len(spec['parts'])} islands ride a parent "
          f"({len(spec['parts'])-n_child} move on their own hinge)\n")
    counts = defaultdict(int)
    vc = defaultdict(int)
    for p in spec['parts']:
        counts[p['role']] += 1
        vc[p['role']] += p['verts']
    print(f"orion: {len(spec['parts'])} islands, extent {spec['extent']}\n")
    print(f"{'role':<16}{'islands':>8}{'verts':>9}  window")
    for r in sorted(counts, key=lambda r: -vc[r]):
        t0, t1, *_ = PLAN.get(r, DEFAULT)
        bar = ' ' * int(t0*20) + '=' * max(1, int((t1-t0)*20))
        print(f"  {r:<14}{counts[r]:>8}{vc[r]:>9}  |{bar:<20}|")
    json.dump(spec, open(__import__('rigpaths').out('orion-fold.json'), 'w'), indent=1)
    print(f"\nwrote orion-fold.json")
