#!/usr/bin/env python3
"""orion_cladding.py — where every car part ends up on the machine.

## The layout this follows

In the prone pose the body lies face-down with its HEAD AT THE CAR'S NOSE
(x +92, inside the engine bay) and its feet trailing to the tail (x -192).
That orientation is what makes the panel assignment obvious rather than
arbitrary, because each panel ends up roughly where it already was:

    bonnet          sits over the head and chest  ->  chest plate
    screen / roof   sits over the shoulders       ->  collar and back
    doors           beside the ribs               ->  shoulder pauldrons
    floor pan       under the spine               ->  spine armour
    rockers         along the flanks              ->  thigh plates
    rear deck       over the legs                 ->  shin and pelvis
    front wheels    beside the head               ->  shoulders
    rear wheels     beside the feet               ->  heels

Nothing has to cross the body. A panel travels a short distance and rotates,
which is the difference between a transformation and a shuffle.

## Format

    group: (bone, (dx, dy, dz), (pitch, yaw, roll), scale)

Offsets are in the BONE's local frame, so a part stays put when the bone
moves. Angles orient the panel against the bone. Scale is for the few parts
that need to read bigger on the machine than they did on the car.

Authored by hand, group by group, from the catalogue in orion_catalogue.py.
"""

# ── what happens to a part when the machine stands ──────────────────────
#
# Not every panel becomes armour. A car is mostly INTERIOR, and a seat, a
# pedal box or a radiator core has no job on the outside of a robot -- placing
# them anyway is why the machine looked like a scrapyard rather than a clad
# figure. Three fates:
#
#   ARMOUR    layers on the outside. Visible in both forms, and it gets the
#             SCALE and the LAYER index, because armour that does not overlap
#             is just panels floating near each other.
#   STOW      goes away. Interior and mechanicals shrink into the core as the
#             body opens -- which is exactly what the core cube is for, and it
#             is the volume the armour needs to spread into.
#   FRAME     structural. Stays, but small and tight to the bone: rails,
#             brackets, suspension links that read as the machine's insides.
ARMOUR, STOW, FRAME = 'armour', 'stow', 'frame'

FATE = {
    # Large sheets RECLAIM. A car panel is a big singly-curved surface shaped
    # to a body it no longer belongs to; no placement makes it follow an arm,
    # and cutting it smaller just makes small flat plates with torn edges.
    # Its material comes back as generated scale armour instead.
    'shell': STOW, 'roof': STOW, 'screen': STOW,
    'deck': STOW, 'wing': STOW, 'undertray': STOW, 'sill_l': STOW,
    'glass_l': STOW, 'glass_r': STOW, 'glass_rear': STOW,
    'glass_q_l': STOW, 'glass_q_r': STOW,
    'louvre_l': ARMOUR, 'louvre_r': ARMOUR,
    'fender_l': STOW, 'fender_r': STOW,
    # sheet-metal body panels from vehicles whose shell is one welded piece
    'quarter_l': STOW, 'quarter_r': STOW, 'glass_front': STOW,
    # the inside of the car has nowhere to go either
    'seat_l': STOW, 'seat_r': STOW, 'cockpit': STOW, 'radiator': STOW,
    'wipers': STOW, 'mirror_l': STOW, 'mirror_r': STOW, 'trim': STOW,
    'exhaust': STOW,
    # KEPT INTACT, and placed where the car's own parts make sense on a body:
    #
    #   wheels  on the JOINTS -- a wheel is a disc with a hub, which is what a
    #           shoulder and a knee already are. Front pair on the shoulders,
    #           rear pair on the knees.
    #   doors   on the BACK. A door is a large panel with a hard edge; across
    #           the shoulder blades it reads as a backplate.
    #   hood    on the CHEST FRONT. It is the widest single curved panel on
    #           the car and a chest is the widest flat place on a robot.
    'door_l': ARMOUR, 'door_r': ARMOUR,
    'bonnet': ARMOUR, 'bonnet_l': ARMOUR, 'bonnet_r': ARMOUR,
    'arch_l': ARMOUR, 'arch_r': ARMOUR, 'lamp': ARMOUR,
    'wheel_fl': ARMOUR, 'wheel_fr': ARMOUR,
    'wheel_rl': ARMOUR, 'wheel_rr': ARMOUR,
    # structure stays, tight to the bone
    'frame_l': FRAME, 'frame_r': FRAME, 'floorpan': FRAME,
    'susp_l': FRAME, 'susp_r': FRAME,
    'bracket_l': FRAME, 'bracket_r': FRAME,
}

# Armour LAYERS. Plates at the same layer sit at the same depth and will
# intersect; each step out is one plate thickness, so a door can overlap a
# shell without passing through it. Numbered from the body outward.
LAYER = {
    'shell': 0, 'floorpan': 0, 'undertray': 0,
    'quarter_l': 1, 'quarter_r': 1,
    'roof': 1, 'deck': 2, 'sill_l': 1, 'fender_l': 1, 'fender_r': 1,
    'door_l': 1, 'door_r': 1, 'arch_l': 1, 'arch_r': 1,
    'screen': 2, 'wing': 2, 'scoop': 2, 'bonnet_l': 2, 'bonnet_r': 2,
    'louvre_l': 2, 'louvre_r': 2,
    'glass_l': 3, 'glass_r': 3, 'glass_rear': 3, 'glass_q_l': 3,
    'glass_q_r': 3, 'lamp': 3,
    'wheel_fl': 3, 'wheel_fr': 3, 'wheel_rl': 3, 'wheel_rr': 3,
}
# One plate thickness, as a FRACTION OF THE BONE'S RADIUS rather than a flat
# number. At a constant 5.0 a layer-3 part sat 15 units out -- fine on a chest
# of radius 17, and most of a forearm's whole width. The three parts still
# floating at the start of the transform were all layer 3 on small bones.
PLATE_FRAC = 0.34
PLATE = 5.0          # kept as the floor, so a thin bone still gets separation

# Armour reads as armour when it is BIG. The derived fit matches a panel to
# its bone's length, which makes a door 44 units on a 34-unit clavicle -- tidy,
# and far too small to look like a door. Armour overhangs.
# 1.20, not 1.55. At 1.55 the plates overhung so far they stopped touching
# anything and read as debris orbiting the machine. Armour overhangs its bone;
# it does not float clear of the body.
SCALE_BY_FATE = {ARMOUR: 1.20, FRAME: 0.80, STOW: 1.0}


# Parts whose outward face is stated rather than derived, in the BONE's own
# frame. `_align` forces the outward direction perpendicular to the bone,
# which is right for a panel lying on a limb and wrong for a wheel: a
# shoulder wheel's axle runs ALONG the clavicle, and a knee wheel's runs
# across the shin. Both are "on the side"; neither is perpendicular-by-default.
# spinning weapons: radians per second about the wheel's own axle, in robot form
SPIN = {'wheel_fl': 9.0, 'wheel_fr': -9.0}

AXLE = {
    # The axle is stated in the BONE's local frame. A clavicle's own
    # direction is the sideways axis, so a shoulder wheel's axle is (0,0,-1)
    # locally -- stating (0,-1,0) put it along the bone's local y, which is
    # world-up, and stood the wheel on edge.
    'wheel_fl': (0.0, 0.0, -1.0),
    'wheel_fr': (0.0, 0.0, 1.0),
    'wheel_rl': (0.0, 0.0, -1.0),   # across the shin
    'wheel_rr': (0.0, 0.0, 1.0),
}


# WHEN each reclaiming part goes, and how long it takes.
#
#   group: (start, duration)
#
# One global window made this a shrink rather than a transformation:
# 27 groups all diminishing at the same rate toward the same point, which
# reads as the car being scaled out of existence. Matter going somewhere has
# an ORDER -- what is buried goes first, what is structural holds on, and
# some things are still there when the machine stands up.
#
# The ordering is not arbitrary. It follows what a machine unpacking itself
# would actually free up first:
#
#   0.16  the cockpit and seats. Buried, no load, nobody can see them go.
#   0.26  mechanicals -- radiator, exhaust, wipers, mirrors.
#   0.34  glass. It is the first thing you would shed and the least useful.
#   0.46  interior trim and the undertray.
#   0.58  the floor pan and screen surround -- structure, so it holds while
#         the body is still using it to push against.
#   0.72  the big outer panels. The shell and roof are the LAST to go,
#         because they are the car, and the car should be recognisable until
#         the machine standing inside it no longer needs the disguise.
#
# Durations vary too: a seat collapses quickly, a shell takes almost a third
# of the timeline to come apart.
DISSOLVE = {
    'cockpit':    (0.16, 0.16),
    'seat_l':     (0.16, 0.14), 'seat_r': (0.17, 0.14),
    'radiator':   (0.26, 0.18),
    'exhaust':    (0.24, 0.20),
    'wipers':     (0.22, 0.12),
    'mirror_l':   (0.26, 0.14), 'mirror_r': (0.27, 0.14),
    'glass_l':    (0.34, 0.16), 'glass_r': (0.35, 0.16),
    'glass_rear': (0.36, 0.16),
    'glass_q_l':  (0.38, 0.14), 'glass_q_r': (0.39, 0.14),
    'trim':       (0.46, 0.20),
    'undertray':  (0.46, 0.22),
    'sill_l':     (0.52, 0.22),
    'floorpan':   (0.58, 0.24),
    'screen':     (0.58, 0.26),
    'deck':       (0.64, 0.24),
    'wing':       (0.66, 0.24),
    'roof':       (0.72, 0.26),
    # a quarter panel is body shell and goes with it; the windscreen goes with
    # the other glass, and holds until then so the car has no hole in it
    'quarter_l':  (0.70, 0.26), 'quarter_r': (0.71, 0.26),
    'glass_front': (0.34, 0.16),
    'shell':      (0.72, 0.28),
}
DISSOLVE_DEFAULT = (0.40, 0.22)


def dissolve(group):
    return DISSOLVE.get(group, DISSOLVE_DEFAULT)


def fate(group):
    return FATE.get(group, FRAME)


def layer(group):
    return LAYER.get(group, 0)


# Offsets on the TORSO bones use x for front/back and z for left/right,
# matching the shoulder line. They used to use z for front/back, which was
# consistent with the chest geometry's old facing and inconsistent with the
# skeleton -- so "on the chest front" put a panel on the machine's flank.
#
# -x is forward. +x is the back.
#
# group -> (bone, offset, euler, scale)
CLAD = {
    # rear quarter panels, from vehicles whose shell is one welded piece: they
    # become thigh plates, sitting on the outside of the leg
    # 0.58, not 0.85: at 0.85 each quarter came out 131 long on a thigh 95
    # long -- overhanging the limb, the oversized pane again
    # SYMMETRIC, on the OUTSIDE of each thigh: this was x -12 on the left and
    # +12 on the right, which (the sign alone counts) put one thigh's trim at
    # the front-outside and the other's at the back-outside. Found when the
    # part lookup copied the asymmetry onto a new vehicle.
    'quarter_l':   ('thigh_l',   (  0, 0.40,  -8), ( 90,   0,   0), 0.58),
    'quarter_r':   ('thigh_r',   (  0, 0.40,   8), ( 90,   0,   0), 0.58),
    # ---- head and chest: the front of the car ------------------------
    # the hood lies on the chest front, pitched to follow it
    # the hood parts down the middle and each half rides a side of the chest
    'bonnet_l':    ('chest',     (-26,   6, -24), (-14,   0,  -8), 1.05),
    'bonnet_r':    ('chest',     (-26,   6,  24), (-14,   0,   8), 1.05),
    'radiator':    ('chest',     (-24,   6,   0), (  0,   0,   0), 0.85),
    'lamp':        ('head',      (-14,   6,   0), (  0,   0,   0), 0.70),
    'screen':      ('chest',     ( -4,  34,   0), (-16,   0,   0), 0.90),
    'glass_rear':  ('head',      ( 11,   2,   0), (  0,   0,   0), 0.50),
    # ---- torso -------------------------------------------------------
    'shell':       ('chest',     (  0,  10,   8), (  0,   0,   0), 1.05),
    # +z is BEHIND once the machine stands. Every panel was on a forward or
    # outboard offset, so the back of the chest was bare from every angle
    # behind it. The roof and the rear deck go there.
    'roof':        ('chest',     ( 26,  18,   0), ( 10,   0,   0), 0.95),
    'floorpan':    ('spine',     ( 14,  16,   0), ( 90,   0,   0), 0.80),
    'undertray':   ('pelvis',    ( 20,   4,   0), ( 90,   0,   0), 0.75),
    'cockpit':     ('spine',     (  0,  18,  -6), ( 90,   0,   0), 0.70),
    # seats ride INSIDE the cage, which is what the bigger ribs made room
    # for -- they dissolve from in there rather than from the surface
    'seat_l':      ('chest',     (-10,   6,   2), (  0,   0,   0), 0.62),
    'seat_r':      ('chest',     ( 10,   6,   2), (  0,   0,   0), 0.62),
    # ---- shoulders: the doors and the front wheels --------------------
    # doors go BEHIND the shoulders, not on them: a backplate, not a pauldron
    # apart and a touch smaller: at +-18 the two doors overlapped 34% of
    # their boxes across the spine
    # SPLAYED into a V on the back, like folded wings: side by side they
    # still met across the spine (17% of one inside the other)
    # the across-limb numbers only give a SIDE; the 5th entry is a real extra
    # spread. Side by side the doors met across the spine.
    'door_l':      ('chest',     ( 26,   4, -22), (  0,   0, -12), 0.84, (6.0, 9.0)),
    'door_r':      ('chest',     ( 26,   4,  22), (  0,   0,  12), 0.84, (6.0, 9.0)),
    'glass_l':     ('clav_l',    (  0,   6, -14), (  0,   0, -22), 0.80),
    'glass_r':     ('clav_r',    (  0,   6,  14), (  0,   0,  22), 0.80),
    # wheels ON THE JOINTS. A wheel is a disc with a hub and so is a shoulder;
    # putting one over the other is the one placement that needs no excuse.
    # TYRES AS WEAPONS: the front wheels ride the outside of each forearm at
    # the wrist, axle across the arm, so they spin like saw blades past the
    # fist. At the shoulder tips they cut into the doors and bonnet halves at
    # t=1 (33-51% of their boxes) and passed 13 units through the upper arm on
    # the way there.
    'wheel_fl':    ('forearm_l', (  0, 0.86, -18), (  0,   0,   0), 0.80),
    'wheel_fr':    ('forearm_r', (  0, 0.86,  18), (  0,   0,   0), 0.80),
    # Mirrors ride the CHEST, not the clavicle. A mirror is a flat plate and
    # the clavicle put it 3.9 units from the shoulder joint, so a planar
    # object sliced through the shoulder for the whole first 70% before the
    # stow fade removed it. Inside the torso it is hidden while it dissolves.
    'mirror_l':    ('chest',     (-10,   2,   6), (  0,   0,   0), 0.60),
    'mirror_r':    ('chest',     ( 10,   2,   6), (  0,   0,   0), 0.60),
    # up the shin, clear of the wheels now riding the ankle (65% overlap)
    'louvre_l':    ('shin_l',    ( 10, 0.30, -18), ( 48,   0, -18), 0.70),
    'louvre_r':    ('shin_r',    ( 10, 0.30,  18), ( 48,   0,  18), 0.70),
    # ---- arms --------------------------------------------------------
    'fender_l':    ('forearm_l', ( -8,  16,  -4), (  0,   0,  -8), 0.80),
    'fender_r':    ('forearm_r', (  8,  16,   4), (  0,   0,   8), 0.80),
    'arch_l':      ('uparm_l',   (-10,  26,   0), (  0,   0,   0), 0.85),
    'arch_r':      ('uparm_r',   ( 10,  26,   0), (  0,   0,   0), 0.85),
    'bracket_l':   ('forearm_l', ( -6,   8,   6), (  0,   0,   0), 0.70),
    'bracket_r':   ('forearm_r', (  6,   8,   6), (  0,   0,   0), 0.70),
    'glass_q_l':   ('forearm_l', ( -6,  26,  -6), (  0,   0,   0), 0.60),
    'glass_q_r':   ('forearm_r', (  6,  26,   6), (  0,   0,   0), 0.60),
    # ---- legs: the rockers, rear deck and rear wheels ------------------
    'sill_l':      ('thigh_l',   (-12,  30,   0), ( 90,   0,   0), 0.85),
    'frame_l':     ('thigh_l',   (-14,  20,   6), ( 90,   0,   0), 0.70),
    'frame_r':     ('thigh_r',   ( 14,  20,   6), ( 90,   0,   0), 0.70),
    'deck':        ('spine',     ( 24,   8,   0), ( 84,   0,   0), 0.80),
    # The front splitter is a wide flat plate. On the head it read as a
    # lopsided hat -- a 186-wide panel on a 28-unit skull. It goes to the
    # SPINE, where a wide flat thing is a back plate, which is also the thing
    # the machine had none of.
    # flared off the back of the shin, angled out, well above the sole
    'wing':        ('shin_l',    ( 14, 0.72, -22), ( 62,   0, -26), 0.80),
    'scoop':       ('shin_r',    ( 14, 0.72,  22), ( 62,   0,  26), 0.80),
    # rear pair on the KNEES -- the top of the shin, not halfway down it. At
    # mid-shin they hung 18 units below the machine's own feet at t=0.47.
    # at the ANKLE, below the shin vents and the thigh trim they overlapped
    # at the knee (19-68% of their boxes)
    # 0.68, not the ankle: there the rim reached below the floor with the leg
    # bent, and the floor clamp pushed the wheel up the shin into the vent
    # and spike (11-27% while standing up)
    'wheel_rl':    ('shin_l',    (  0, 0.68, -20), (  0,   0,   0), 0.85),
    'wheel_rr':    ('shin_r',    (  0, 0.68,  20), (  0,   0,   0), 0.85),
    'susp_l':      ('shin_l',    (-10,  20,   8), ( 90,   0,   0), 0.70),
    'susp_r':      ('shin_r',    ( 10,  20,   8), ( 90,   0,   0), 0.70),
    'exhaust':     ('pelvis',    (  0,   2,  20), ( 84,   0,   0), 0.70),

    # ---- odds --------------------------------------------------------
    'wipers':      ('head',      (  0,  14,   4), (  0,   0,   0), 0.55),
    'trim':        ('spine',     (  0,  24,  10), ( 90,   0,   0), 0.65),
}

# Anything the catalogue names but this does not place rides the spine, where
# it is least in the way. Better than dropping it silently.
FALLBACK = ('spine', (0, 20, 12), (90, 0, 0), 0.6)


def bone_of(group):
    return CLAD.get(group, FALLBACK)[0]


if __name__ == '__main__':
    import orion_catalogue as C
    from collections import Counter
    groups = sorted({p.group for p in C.CATALOGUE})
    missing = [g for g in groups if g not in CLAD]
    extra = [g for g in CLAD if g not in groups]
    print(f"{len(groups)} catalogue groups, {len(CLAD)} placed")
    if missing:
        print(f"  not placed (will ride the spine): {', '.join(missing)}")
    if extra:
        print(f"  placed but not in the catalogue: {', '.join(extra)}")
    print()
    by_bone = Counter(v[0] for v in CLAD.values())
    print(f"{'bone':<12}{'groups':>7}  what it carries")
    for b, n in by_bone.most_common():
        names = [g for g, v in CLAD.items() if v[0] == b]
        print(f"  {b:<10}{n:>7}  {', '.join(names)[:54]}")
