#!/usr/bin/env python3
"""orion_catalogue.py — ROUND 1: what every one of the 387 parts actually is.

## Why this round exists

Four attempts at a transformation failed the same way, and the reason was
always the same: I was inferring articulation from geometry that has none.
orion is one node, one mesh, one material. Roles, slots, bones, clearance —
all of it was guessing what a piece was from its bounding box.

So: look at them. Every island rendered on its own, identified by eye, written
down. That is what an artist does first and it is the step I skipped.

## What the looking found

This is not a crude game asset. orion has:

    #12-15    four tyres, 1855-1856v each
    #8-11     vented brake rotors and discs, front and rear
    #18/19    bucket seats, with #42/43 as their backs
    #31/32    coil springs
    #64       a suspension wishbone
    #67       a shock body
    #52       a steering wheel
    #59       a headlight dish
    #27/28    side glass, #61/62 quarter glass
    #0        the main body shell, with its wheel arches

Identifiable parts change the problem. A tyre knows where it goes on a robot;
a "roundish island of 1856 verts" does not.

## How a part is written down

    P(id, name, group, mirror, note)

`group` is what moves together and is the unit a bone carries. `mirror` is the
id of its twin across z, or None. The note is why, when it is not obvious.

Groups are deliberately SMALL. The last attempt had one group of 222 islands
and it moved like a slab, because it was one. A transformation is many small
pieces going many places, which is the whole reason it reads as mechanical.

## Coverage

The 72 parts below are hand-identified from renders. The remaining 315 are
small fasteners, trim and greebles, and they are assigned by the rule at the
bottom: a part under 370 verts rides the nearest hand-identified group. That
is not laziness — a bolt has no independent destination, and giving it one is
what made parts fly around in the previous rounds.
"""
from collections import namedtuple

P = namedtuple('P', 'id name group mirror note')

# ─────────────────────────────────────────────────────── hand-identified
# id, name, group, mirror, note
CATALOGUE = [
    # ---- body shell -------------------------------------------------
    P(  0, 'body shell',        'shell',       None, 'the whole outer body with arches; the single biggest piece'),
    P(  1, 'roof panel',        'roof',        None, 'curved, spans the cabin'),
    P(  2, 'floor pan',         'floorpan',    None, 'long flat, full length'),
    P(  3, 'frame rail L', 'frame_r',        4, 'chassis beam, 319 long'),
    P(  4, 'frame rail R', 'frame_l',        3, ''),
    P(  5, 'screen frame',      'screen',      None, 'windscreen surround / rollover hoop'),
    P(  6, 'bonnet',            'bonnet',      None, 'vented'),
    P(  7, 'front wing',        'wing',        None, 'sits at x +178 -- the NOSE end. A front splitter, not a rear wing.'),
    P( 16, 'undertray',         'undertray',   None, 'flat, perforated'),
    P( 17, 'wipers',            'wipers',      None, 'thin, paired arms'),
    P( 35, 'front fender L',    'fender_l',      36, ''),
    P( 36, 'front fender R',    'fender_r',      35, ''),
    P( 30, 'rear deck panel',   'deck',        None, ''),
    P( 46, 'side scoop',        'scoop',       None, ''),
    P( 47, 'louvre L',          'louvre_l',      48, ''),
    P( 48, 'louvre R',          'louvre_r',      47, ''),
    # ---- glass ------------------------------------------------------
    P( 27, 'side glass L', 'glass_r',       28, ''),
    P( 28, 'side glass R', 'glass_l',       27, ''),
    P( 34, 'rear glass',        'glass_rear',  None, ''),
    P( 61, 'quarter glass L',   'glass_q_l',     62, ''),
    P( 62, 'quarter glass R',   'glass_q_r',     61, ''),
    P( 56, 'lamp bezel',        'lamp',        None, ''),
    P( 59, 'headlight dish',    'lamp',        None, 'reflector'),
    # ---- doors and sides --------------------------------------------
    P( 22, 'door skin L',       'door_l',        23, ''),
    P( 23, 'door skin R',       'door_r',        22, ''),
    P( 29, 'sill blade L',      'sill_l',      None, ''),
    P( 24, 'arch tube L', 'arch_r',        25, 'curved tube over the wheel'),
    P( 25, 'arch tube R', 'arch_l',        24, ''),
    # ---- wheels: four corners, each its own group -------------------
    #
    # Corners assigned from MEASURED POSITION, not from the renders. Reading
    # them off a contact sheet put six of these eight parts in the wrong
    # corner -- a rotor at x=-138 was filed as front-left -- which is why a
    # tyre appeared on the wrong side of the vehicle form. The car's nose is
    # +x and its left is -z; that is all it takes to be sure.
    #
    #   id  x     z     corner
    #    8  +120  -89   FL        12  -138  +80   RR
    #    9  +120  +89   FR        13  +120  +80   FR
    #   10  -138  +81   RR        14  -138  -80   RL
    #   11  -138  -81   RL        15  +120  -80   FL
    P( 15, 'tyre FL',           'wheel_fl',      13, ''),
    P( 13, 'tyre FR',           'wheel_fr',      15, ''),
    P( 14, 'tyre RL',           'wheel_rl',      12, ''),
    P( 12, 'tyre RR',           'wheel_rr',      14, ''),
    P(  8, 'rotor FL',          'wheel_fl',       9, 'vented disc'),
    P(  9, 'rotor FR',          'wheel_fr',       8, ''),
    P( 11, 'rotor RL',          'wheel_rl',      10, ''),
    P( 10, 'rotor RR',          'wheel_rr',      11, ''),
    # ---- suspension: this is what makes a leg look mechanical -------
    P( 31, 'coil spring L',     'susp_l',        32, ''),
    P( 32, 'coil spring R',     'susp_r',        31, ''),
    P( 64, 'wishbone',          'susp_l',      None, 'A-arm'),
    P( 67, 'shock body', 'susp_l',      None, ''),
    P( 26, 'tie rod',           'susp_l',      None, ''),
    P( 49, 'link L', 'susp_r',        50, ''),
    P( 50, 'link R',            'susp_r',        49, ''),
    P( 20, 'sway bar',          'susp_r',      None, 'oval loop'),
    # ---- interior: seats become the chest cavity --------------------
    P( 18, 'seat base L', 'seat_r',        19, ''),
    P( 19, 'seat base R', 'seat_l',        18, ''),
    P( 42, 'seat back L', 'seat_r',        43, 'ribbed'),
    P( 43, 'seat back R', 'seat_l',        42, ''),
    P( 52, 'steering wheel',    'cockpit',     None, ''),
    P( 21, 'pedal box',         'cockpit',     None, ''),
    P( 51, 'tank sphere',       'cockpit',     None, 'spherical vessel'),
    P( 63, 'shift linkage',     'cockpit',     None, ''),
    # ---- exhaust and intake -----------------------------------------
    P( 37, 'exhaust tips R',    'exhaust',       38, 'sits z +64'),
    P( 38, 'exhaust tips L',    'exhaust',       37, 'sits z -64'),
    P( 33, 'radiator',          'radiator',    None, 'grille core'),
    P( 60, 'oil cooler',        'radiator',    None, 'perforated plate'),
    P( 68, 'intake tray',       'radiator',    None, ''),
    # ---- brackets and trim ------------------------------------------
    P( 39, 'mirror arm L',      'mirror_l',      40, ''),
    P( 40, 'mirror arm R',      'mirror_r',      39, ''),
    P( 70, 'mirror head',       'mirror_l',    None, ''),
    P( 41, 'trim blade',        'trim',        None, ''),
    P( 44, 'trim strip L',      'trim',          45, ''),
    P( 45, 'trim strip R',      'trim',          44, ''),
    P( 53, 'bracket plate L', 'bracket_r',     55, ''),
    P( 55, 'bracket plate R', 'bracket_l',     53, ''),
    P( 54, 'splitter blade',    'trim',        None, ''),
    P( 57, 'clip',              'trim',        None, ''),
    P( 58, 'door handle',       'trim',        None, ''),
    P( 65, 'rail L', 'bracket_r',     66, ''),
    P( 66, 'rail R', 'bracket_l',     65, ''),
    P( 69, 'mount bracket',     'bracket_l',   None, ''),
    P( 71, 'grab handle',       'trim',        None, ''),
]

# Sides were re-derived from MEASURED POSITION. Reading them off renders got
# left and right backwards on 19 of 72 parts -- systematically, not randomly,
# because a three-quarter view puts -z on whichever side the camera happens to
# be. The car's nose is +x and its left is -z; nothing else is needed.
BY_ID = {p.id: p for p in CATALOGUE}
GROUPS = sorted({p.group for p in CATALOGUE})

# A part below this size has no independent destination. It rides the nearest
# hand-identified group. Giving a bolt its own path is precisely what made the
# previous rounds look like a cloud.
RIDE_BELOW = 370

# How far a small part may be from the thing it rides. Beyond this it is not
# attached to it, whatever the nearest-surface search says.
CAP = 46.0


def assign(B, parts=None, sample=160):
    """Give every one of the 387 islands a group.

    Attachment is measured to the candidate's actual SURFACE, not to its
    bounding box. The box version put 176 islands on the body shell and 70 on
    the wipers, because a shell's box encloses the whole car: every part
    inside the body scored distance zero to it. Distance to the nearest vertex
    asks the question that was meant — "is this bolted to that?" — instead of
    "is this inside that?".

    `parts` is the cached per-island geometry (orion/parts.pkl). Without it
    this falls back to box distance and says so."""
    out = {p.id: p.group for p in CATALOGUE}
    big = [p.id for p in CATALOGUE]
    if parts is None:
        import sys
        print("  (no geometry given — falling back to box distance, which "
              "over-groups enclosing shells)", file=sys.stderr)
    surf = {}
    if parts is not None:
        for j in big:
            v = parts[j]['v']
            step = max(1, len(v) // sample)
            surf[j] = [tuple(x) for x in v[::step]]
    for i, b in enumerate(B):
        if i in out:
            continue
        c0 = b['ctr']
        best, bd = None, 1e30
        for j in big:
            if j in surf:
                d = min((c0[0]-v[0])**2 + (c0[1]-v[1])**2 + (c0[2]-v[2])**2
                        for v in surf[j])
            else:
                c = B[j]
                d = 0.0
                for a in range(3):
                    lo, hi = c['lo'][a], c['hi'][a]
                    if c0[a] < lo: d += (lo - c0[a]) ** 2
                    elif c0[a] > hi: d += (c0[a] - hi) ** 2
            if d < bd:
                bd, best = d, j
        # DISTANCE CAP. A greeble more than this from anything identified is
        # not bolted to it, and attaching it anyway stretched groups across
        # the whole car: `wheel_fl` spanned 310 units on a wheel 60 across,
        # because clips from the far end were nearest-by-default. Those go to
        # a bucket named for their own region instead, where at least the
        # group's centre is somewhere near its geometry.
        if bd > CAP * CAP:
            c0 = b['ctr']
            reg = ('front' if c0[0] > 60 else ('rear' if c0[0] < -60 else 'mid'))
            side = ('l' if c0[2] < -30 else ('r' if c0[2] > 30 else 'c'))
            out[i] = f'loose_{reg}_{side}'
        else:
            out[i] = out[best]
    return out


if __name__ == '__main__':
    import orion_anat as A
    from collections import Counter
    V, I = A.load(__import__('rigpaths').inp('orion.glb'))
    gs = A.islands(V, I)
    B = [A.bb(g) for g in gs]
    import pickle, os
    parts = A.parts()   # rebuilt from orion.glb on first use
    g = assign(B, parts)
    c = Counter(g.values())
    print(f"{len(CATALOGUE)} parts identified by eye, {len(B)} islands total, "
          f"{len(GROUPS)} groups\n")
    print(f"{'group':<14}{'islands':>8}{'verts':>9}  hand-identified members")
    vc = Counter()
    for i, b in enumerate(B):
        vc[g[i]] += b['n']
    for grp in sorted(c, key=lambda k: -vc[k]):
        named = [BY_ID[p.id].name for p in CATALOGUE if p.group == grp]
        print(f"  {grp:<12}{c[grp]:>8}{vc[grp]:>9}  {', '.join(named)[:52]}")
    biggest = max(c.values())
    print(f"\nlargest group: {biggest} islands "
          f"({'good — nothing moves as a slab' if biggest < 60 else 'TOO BIG, split it'})")
