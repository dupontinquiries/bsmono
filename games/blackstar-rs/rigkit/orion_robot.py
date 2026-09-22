#!/usr/bin/env python3
"""orion_robot.py — author orion's transformation as a DESTINATION.

## What was wrong with the first attempt

It authored departures: "rotate this part -96 degrees about z from t=0.18 to
t=0.56". Every motion was individually defensible and the end state was a
cloud of parts, because nothing in the description was a robot. At t=0 the car
read perfectly; by t=0.6 it had scattered. That is the predictable outcome of
specifying HOW things move and never WHERE THEY END UP.

## The reframing

A transformation is not a fold. Origami is a fold: the same sheet, hinged
flat. A transformation is **the same mass rearranging into a second coherent
body**, and the second body is the specification. So:

  1. Define the ROBOT first, as a skeleton of named slots with a position,
     an orientation and a size in the car's own units.
  2. Assign every assembly to a slot.
  3. The motion is whatever gets the assembly from its car pose into its slot.

The end state is then a robot BY CONSTRUCTION. There is no arrangement of
timings that can produce a cloud, because the destination is fixed and only
the route is being chosen.

## The skeleton

orion is 458 long, 106 tall, 195 wide, ground at y=0. A car that size stands
up to roughly 360 tall. Slots below are in car units, +x tail, +y up, +z
right, and the robot faces -x (the car's nose direction), so it walks the way
it drove.

The assignment follows the car's own layout, which is the only assignment this
geometry supports:

  rear deck + rear wheels -> pelvis and thighs   (the rear axle is already
                                                  where the hips need to be)
  rockers                 -> shins                (long, already vertical-ish
                                                  when stood on end)
  floor                   -> spine
  cabin                   -> chest                (the biggest single mass
                                                  belongs where mass reads)
  nose + front wheels     -> arms                 (front axle lands at the
                                                  elbows, as on a real toy)
  upper side panels       -> shoulder armour
"""
import json, math
from collections import defaultdict
import orion_anat as A

SRC = __import__('rigpaths').inp('orion.glb')


# ── the robot ────────────────────────────────────────────────────────────
#
# slot: (x, y, z, pitch, yaw, roll, t0, t1)
#
# Position is where the assembly's CENTRE goes. Angles are degrees applied in
# that order about (z, y, x) — enough to stand a part on end and splay it.
# The window is when this slot is travelled.
#
# Read down the list and it is a robot: feet on the ground, shins, thighs,
# pelvis, spine, chest, shoulders, arms, head.
SLOTS = {
    # ---- legs: the rear of the car, stood up -------------------------
    'foot_l':      (-40,   14,  -52,   0,   0,   0, 0.30, 0.72),
    'foot_r':      (-40,   14,   52,   0,   0,   0, 0.30, 0.72),
    'shin_l':      (-20,   72,  -50,  90,   0,   4, 0.26, 0.68),
    'shin_r':      (-20,   72,   50,  90,   0,  -4, 0.26, 0.68),
    'thigh_l':     ( -6,  150,  -46,  90,   0,   6, 0.18, 0.60),
    'thigh_r':     ( -6,  150,   46,  90,   0,  -6, 0.18, 0.60),
    'pelvis':      (  0,  196,    0,  90,   0,   0, 0.16, 0.58),
    # ---- spine and chest ---------------------------------------------
    'spine':       (  4,  238,    0,  90,   0,   0, 0.34, 0.76),
    'chest':       (  6,  276,    0,  84,   0,   0, 0.40, 0.82),
    'chest_plate': (-16,  272,    0,  84,   0,   0, 0.52, 0.88),
    # ---- shoulders and arms ------------------------------------------
    'shoulder_l':  (  6,  306,  -78,   0,   0, -18, 0.46, 0.84),
    'shoulder_r':  (  6,  306,   78,   0,   0,  18, 0.46, 0.84),
    'upperarm_l':  (  6,  256,  -86,  90,   0, -10, 0.58, 0.92),
    'upperarm_r':  (  6,  256,   86,  90,   0,  10, 0.58, 0.92),
    'forearm_l':   (  0,  190,  -90,  90,   0,  -6, 0.64, 0.96),
    'forearm_r':   (  0,  190,   90,  90,   0,   6, 0.64, 0.96),
    # ---- head ---------------------------------------------------------
    'head':        (  2,  330,    0,   0,   0,   0, 0.86, 1.00),
}

# role -> slot. A role with two slots splits left/right by the part's own z.
ASSIGN = {
    'rear_deck':     ('pelvis',),
    'wheel_rear_l':  ('foot_l',),
    'wheel_rear_r':  ('foot_r',),
    'rocker_l':      ('shin_l',),
    'rocker_r':      ('shin_r',),
    'floor':         ('spine',),
    'core':          ('spine',),
    'cabin':         ('chest',),
    'nose':          ('chest_plate',),
    'wheel_front_l': ('forearm_l',),
    'wheel_front_r': ('forearm_r',),
    'panel_l':       ('shoulder_l',),
    'panel_r':       ('shoulder_r',),
    'rear_detail':   ('pelvis',),
    'front_detail':  ('chest_plate',),
}


def euler_quat(pitch, yaw, roll):
    """ZYX degrees -> quaternion (x, y, z, w)."""
    cz, sz = math.cos(math.radians(roll)*0.5),  math.sin(math.radians(roll)*0.5)
    cy, sy = math.cos(math.radians(yaw)*0.5),   math.sin(math.radians(yaw)*0.5)
    cx, sx = math.cos(math.radians(pitch)*0.5), math.sin(math.radians(pitch)*0.5)
    # R = Rz(roll) * Ry(yaw) * Rx(pitch)
    w = cz*cy*cx + sz*sy*sx
    x = cz*cy*sx - sz*sy*cx
    y = cz*sy*cx + sz*cy*sx
    z = sz*cy*cx - cz*sy*sx
    return [round(v, 5) for v in (x, y, z, w)]


def build(min_verts=900):
    import orion_fold as F                     # reuse the anatomy + parenting
    B, W = F.anatomy()
    roles = F.classify(B, W)
    parent = F.parent_small(B, W, min_verts)
    for i, pj in parent.items():
        if pj is not None:
            roles[i] = roles[pj]

    # Each ROLE moves as one rigid body: its parts keep their relative
    # positions, so an assembly arrives intact rather than each island
    # solving for itself. This is the other half of what stops a cloud.
    by_role = defaultdict(list)
    for i, r in enumerate(roles):
        by_role[r].append(i)
    role_ctr = {}
    for r, idxs in by_role.items():
        lo = [min(B[i]['lo'][a] for i in idxs) for a in range(3)]
        hi = [max(B[i]['hi'][a] for i in idxs) for a in range(3)]
        role_ctr[r] = [(lo[a] + hi[a]) * 0.5 for a in range(3)]

    parts = []
    for i, b in enumerate(B):
        r = roles[i]
        slot_names = ASSIGN.get(r)
        if not slot_names:
            slot = None
        else:
            slot = SLOTS[slot_names[0]]
        if slot is None:
            parts.append({'i': i, 'role': r, 'slot': None, 'verts': b['n']})
            continue
        sx, sy, sz, p, yaw, roll, t0, t1 = slot
        c = role_ctr[r]
        parts.append({
            'i': i, 'role': r, 'slot': slot_names[0], 'verts': b['n'],
            # the ASSEMBLY's centre is the thing that lands on the slot;
            # every island in the role shares it, which is what keeps the
            # assembly rigid
            'pivot': [round(v, 3) for v in c],
            'to': [sx, sy, sz],
            'quat': euler_quat(p, yaw, roll),
            't0': t0, 't1': t1,
        })
    return {
        'model': 'orion',
        'extent': [round(v, 3) for v in W['size']],
        'centre': [round(v, 3) for v in W['ctr']],
        'ground': round(W['lo'][1], 3),
        'slots': {k: list(v) for k, v in SLOTS.items()},
        'parts': parts,
    }


if __name__ == '__main__':
    spec = build()
    n_slot = defaultdict(int)
    v_slot = defaultdict(int)
    for p in spec['parts']:
        n_slot[p['slot']] += 1
        v_slot[p['slot']] += p['verts']
    print(f"orion -> robot: {len(spec['parts'])} islands into {len(SLOTS)} slots\n")
    print(f"{'slot':<14}{'islands':>8}{'verts':>9}  window")
    for s in list(SLOTS) + [None]:
        if n_slot[s] == 0:
            continue
        if s is None:
            print(f"  {'(unassigned)':<12}{n_slot[s]:>8}{v_slot[s]:>9}")
            continue
        t0, t1 = SLOTS[s][6], SLOTS[s][7]
        bar = '·' * int(t0*22) + '\u2588' * max(1, int((t1-t0)*22))
        print(f"  {s:<12}{n_slot[s]:>8}{v_slot[s]:>9}  |{bar:<22}|")
    tall = max(v[1] for v in SLOTS.values())
    print(f"\nrobot stands {tall} tall against a car {spec['extent'][0]:.0f} long "
          f"and {spec['extent'][1]:.0f} tall")
    json.dump(spec, open(__import__('rigpaths').out('orion-robot.json'), 'w'), indent=1)
    print("wrote orion-robot.json")
