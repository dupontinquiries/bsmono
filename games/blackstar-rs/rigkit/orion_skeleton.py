#!/usr/bin/env python3
"""orion_skeleton.py — the bot is a core that extends; the car is cladding.

## The theory, which is already this project's own

`blackstar-rs/src/compact.rs` states it:

    xform.rs's morph lerps vertex i of pose k toward vertex i of pose k+1.
    That is smooth, cheap, and it is exactly the trick this project set out
    not to do... It reads as a slide because it IS a slide. The reference
    material's own answer -- matter compacting into cubes, and cubes
    unfolding into machines -- is a better one, and it is buildable: matter
    goes somewhere on the way, and the somewhere is a lattice.

So the transformation is not car parts rearranging into a robot. It is:

  * a SKELETON, compressed into a small lattice cube buried in the vehicle
    (here, where the engine would be), that EXTENDS outward bone by bone;
  * the car's panels are CLADDING attached to bones, carried wherever their
    bone goes;
  * pistons and latches spanning bone pairs, visibly extending as the bone
    they brace grows.

That fixes the cohesion problem directly. In the previous attempt every
assembly solved its own path and the middle of the transform was a cloud,
because nothing held the parts in relation to each other. A skeleton IS that
relation: a panel cannot drift, because it is bolted to a bone whose position
is determined by its parent.

## The skeleton is hand-authored on purpose

Everything derived so far -- roles, parenting, slots -- came from measuring
the mesh, and the mesh does not know what a shoulder is. The bones below are
written by hand in orion's own units, which is the thing that was missing.

Bone: (name, parent, offset-from-parent-when-extended, radius, t0, t1)

`offset` is where this bone's END sits relative to its parent's end once
extended. At t=0 every offset is scaled to ~0, so the whole skeleton is
inside the core cube. The bone's own window says when it grows.
"""
import json, math
from collections import defaultdict

# Core sits low and slightly forward of centre: where an engine would be, and
# low enough that the robot rises out of the car rather than out of its roof.
# Two positions for the core, because it moves. BURIED is where the cube sits
# inside the car -- low, forward of centre, where an engine would be. STOOD is
# where it ends up once the legs have extended underneath it.
#
# The legs reach 102 units below the core, so a core that stays buried puts
# the feet at y = -60: underground. The machine has to lift its own mass to
# stand, and the core rising IS that lift. It is not a fudge to get the feet
# above the grid; it is the thing a transformation actually does.
CORE_BURIED = (-60.0, 42.0, 0.0)
CORE_STOOD  = (-40.0, 112.0, 0.0)
CORE = CORE_STOOD           # rest_pose() builds the EXTENDED skeleton
CORE_SIZE = 46.0            # the compressed lattice cube, in car units

# name           parent        offset (x,y,z)    r     t0    t1
BONES = [
    ('core',      None,        (  0,   0,   0),  23.0, 0.00, 0.08),
    # spine grows up out of the core first -- the machine stands before it
    # does anything else, which is what makes the rest read as deliberate
    ('pelvis',    'core',      (  8,  46,   0),  16.0, 0.10, 0.30),
    ('spine',     'pelvis',    (  4,  52,   0),  14.0, 0.16, 0.38),
    ('chest',     'spine',     (  2,  46,   0),  20.0, 0.22, 0.46),
    ('neck',      'chest',     (  0,  26,   0),   7.0, 0.72, 0.82),
    ('head',      'neck',      (  2,  22,   0),  12.0, 0.78, 0.92),
    # legs: down and out from the pelvis
    ('hip_l',     'pelvis',    ( -2,  -8, -34),  11.0, 0.24, 0.40),
    ('hip_r',     'pelvis',    ( -2,  -8,  34),  11.0, 0.24, 0.40),
    ('thigh_l',   'hip_l',     (  2, -62,  -6),  13.0, 0.28, 0.50),
    ('thigh_r',   'hip_r',     (  2, -62,   6),  13.0, 0.28, 0.50),
    ('shin_l',    'thigh_l',   ( -4, -60,   2),  11.0, 0.36, 0.60),
    ('shin_r',    'thigh_r',   ( -4, -60,  -2),  11.0, 0.36, 0.60),
    ('foot_l',    'shin_l',    (-16, -18,   0),  10.0, 0.46, 0.68),
    ('foot_r',    'shin_r',    (-16, -18,   0),  10.0, 0.46, 0.68),
    # arms: out from the chest, last, so the machine finishes by opening up
    ('clav_l',    'chest',     (  0,  16, -40),   9.0, 0.44, 0.60),
    ('clav_r',    'chest',     (  0,  16,  40),   9.0, 0.44, 0.60),
    ('uparm_l',   'clav_l',    (  0, -48, -12),  11.0, 0.52, 0.72),
    ('uparm_r',   'clav_r',    (  0, -48,  12),  11.0, 0.52, 0.72),
    ('forearm_l', 'uparm_l',   (  4, -46,  -4),   9.0, 0.60, 0.82),
    ('forearm_r', 'uparm_r',   (  4, -46,   4),   9.0, 0.60, 0.82),
    ('hand_l',    'forearm_l', (  0, -16,   0),   8.0, 0.70, 0.88),
    ('hand_r',    'forearm_r', (  0, -16,   0),   8.0, 0.70, 0.88),
]

# A piston braces two bones. It is drawn as a cylinder from A to B and it
# LENGTHENS as they separate, which is the visible evidence that the skeleton
# is being pushed rather than keyframed.
PISTONS = [
    ('core', 'spine'), ('pelvis', 'chest'),
    ('hip_l', 'shin_l'), ('hip_r', 'shin_r'),
    ('thigh_l', 'foot_l'), ('thigh_r', 'foot_r'),
    ('clav_l', 'forearm_l'), ('clav_r', 'forearm_r'),
    ('chest', 'head'),
]

# Where each car assembly ENDS UP ON THE ROBOT.
#
#   role -> (bone, offset from that bone's end, pitch, yaw, roll)
#
# The offset and the angles are in robot space and are AUTHORED, not derived.
# That is the whole correction: the first version stored `assembly_centre -
# bone_rest`, so `bone_end + offset` resolved straight back to the car
# position and every panel was faithfully carried to exactly where it already
# was. A cladding map has to say where a plate goes on the ROBOT, or it is not
# a map at all.
#
# Angles matter as much as positions. A rocker lies flat along the car and has
# to stand vertical on a thigh; the wheel that was rolling on the road ends up
# on the outside of a shin. Carrying a part without turning it is what makes
# a transformation read as a car being levitated in pieces.
CLADDING = {
    # torso: the cabin roof becomes the chest, the nose plate its front
    'cabin':         ('chest',     (  6,  -6,   0),  -8,   0,   0),
    'nose':          ('chest',     (-26,  -2,   0),  74,   0,   0),
    'front_detail':  ('chest',     (-22,  14,   0),  74,   0,   0),
    'floor':         ('spine',     (  4,  -6,   0),  88,   0,   0),
    'core':          ('spine',     (  2,  -2,   0),  88,   0,   0),
    # pelvis: the rear deck wraps the hips
    'rear_deck':     ('pelvis',    ( -4,  -8,   0),  84,   0,   0),
    'rear_detail':   ('pelvis',    ( -8, -12,   0),  84,   0,   0),
    # legs: rockers stand up as thigh armour, rear wheels ride the shins
    'rocker_l':      ('thigh_l',   (  0,  26, -12),  90,   0,   6),
    'rocker_r':      ('thigh_r',   (  0,  26,  12),  90,   0,  -6),
    'wheel_rear_l':  ('shin_l',    ( -4,  16, -20),   0,   0,   0),
    'wheel_rear_r':  ('shin_r',    ( -4,  16,  20),   0,   0,   0),
    # arms: side panels become shoulder pauldrons, front wheels the elbows
    'panel_l':       ('clav_l',    (  0,   4, -16),   0,   0, -24),
    'panel_r':       ('clav_r',    (  0,   4,  16),   0,   0,  24),
    'wheel_front_l': ('forearm_l', (  0,  20, -14),   0,   0,   0),
    'wheel_front_r': ('forearm_r', (  0,  20,  14),   0,   0,   0),
}


def rest_pose():
    """Bone END positions when fully extended, in car units."""
    pos = {}
    for name, parent, off, r, t0, t1 in BONES:
        base = CORE if parent is None else pos[parent]
        pos[name] = (base[0] + off[0], base[1] + off[1], base[2] + off[2])
    return pos


def build(min_verts=900):
    import orion_fold as F
    B, W = F.anatomy()
    roles = F.classify(B, W)
    parent = F.parent_small(B, W, min_verts)
    for i, pj in parent.items():
        if pj is not None:
            roles[i] = roles[pj]

    # Each cladding group keeps its shape: one rigid body per role, pivoting
    # about the assembly's own centre. A panel is bolted to a bone, not melted
    # onto it.
    by_role = defaultdict(list)
    for i, r in enumerate(roles):
        by_role[r].append(i)
    ctr = {}
    for r, idxs in by_role.items():
        lo = [min(B[i]['lo'][a] for i in idxs) for a in range(3)]
        hi = [max(B[i]['hi'][a] for i in idxs) for a in range(3)]
        ctr[r] = [(lo[a] + hi[a]) * 0.5 for a in range(3)]

    rest = rest_pose()
    bones = []
    for name, par, off, r, t0, t1 in BONES:
        bones.append({'n': name, 'p': par, 'o': list(off), 'r': r,
                      't0': t0, 't1': t1,
                      'rest': [round(v, 2) for v in rest[name]]})

    import orion_robot as R

    def rotate(v, pit, yaw, rol):
        """ZYX degrees applied to a vector, matching euler_quat's convention."""
        import math as _m
        x, y, z = v
        a = _m.radians(pit)                      # about x
        y, z = y*_m.cos(a) - z*_m.sin(a), y*_m.sin(a) + z*_m.cos(a)
        a = _m.radians(yaw)                      # about y
        x, z = x*_m.cos(a) + z*_m.sin(a), -x*_m.sin(a) + z*_m.cos(a)
        a = _m.radians(rol)                      # about z
        x, y = x*_m.cos(a) - y*_m.sin(a), x*_m.sin(a) + y*_m.cos(a)
        return [x, y, z]

    parts = []
    # A moving part needs somewhere to move THROUGH. Clearance is a radial
    # push away from the body's own centreline, largest in the middle of a
    # part's travel and zero at both ends: the machine opens up, routes the
    # piece around the outside, and closes again. That is the continuous
    # version of pressing `explode` first, and it is the only reason a panel
    # can cross the body without going through the panel next to it.
    for i, b in enumerate(B):
        r = roles[i]
        cl = CLADDING.get(r)
        if cl is None:
            parts.append({'i': i, 'r': r, 'b': None}); continue
        bone, off, pit, yaw, rol = cl
        # EVERY ISLAND MOVES SEPARATELY. Its seat on the robot is its own
        # position within the assembly, rotated with the assembly and hung off
        # the bone -- so the plate keeps its neighbours without the assembly
        # ever being one rigid lump. A car cannot hoist its own roof; it can
        # crack along the seams it already has, and these 387 islands ARE
        # those seams.
        local = [b['ctr'][a] - ctr[r][a] for a in range(3)]
        local = rotate(local, pit, yaw, rol)
        seat = [rest[bone][a] + off[a] + local[a] for a in range(3)]
        # clearance direction: straight out from the body axis (x is length,
        # so push in y/z), with a floor so a part sitting on the centreline
        # still gets somewhere to go
        dy, dz = b['ctr'][1] - 50.0, b['ctr'][2]
        mag = (dy*dy + dz*dz) ** 0.5
        if mag < 1e-3:
            dy, dz, mag = 0.0, 1.0, 1.0
        clear = [0.0, dy/mag, dz/mag]
        # how far it has to travel decides how much room it needs
        dist = sum((seat[a] - b['ctr'][a])**2 for a in range(3)) ** 0.5
        # Amplitude chosen by SWEEP, not by eye. Counting island pairs within
        # 22 units across the travel window, against the car's own resting
        # density of 189 pairs:
        #
        #   clearance   worst crowding
        #        0x     185   (straight line, and it EXCEEDS rest when the
        #                      departure stagger is widened)
        #        1x     182
        #        2x     142
        #        3x     129   <- the floor; 4.5x gives nothing more
        #
        # 2.5x sits just off the knee. 3x is marginally better and throws
        # parts far enough out that the machine stops looking like it is
        # working within itself, which is the thing being bought here.
        amp = min(220.0, 45.0 + dist * 0.55)
        parts.append({
            'i': i, 'r': r, 'b': bone,
            'seat': [round(v, 2) for v in seat],
            'q': R.euler_quat(pit, yaw, rol),
            'clear': [round(v*amp, 2) for v in clear],
            'c': [round(v, 2) for v in b['ctr']],
            'd': round(dist, 1),
        })
    return {
        'model': 'orion',
        'extent': [round(v, 3) for v in W['size']],
        'centre': [round(v, 3) for v in W['ctr']],
        'ground': round(W['lo'][1], 3),
        'core': list(CORE_STOOD), 'core_buried': list(CORE_BURIED),
        'core_size': CORE_SIZE,
        # the core's own window: it starts lifting as soon as the pelvis has
        # something to push against, and is up before the legs finish
        'core_t': [0.12, 0.52],
        'bones': bones,
        'pistons': [list(p) for p in PISTONS],
        'parts': parts,
    }


if __name__ == '__main__':
    spec = build()
    rest = rest_pose()
    ys = [v[1] for v in rest.values()]
    print(f"skeleton: {len(BONES)} bones, {len(PISTONS)} pistons")
    print(f"core: buried {CORE_BURIED} -> stood {CORE_STOOD}, cube {CORE_SIZE}")
    print(f"stands y {min(ys):.0f} .. {max(ys):.0f}  (car is 106 tall, ground 0)")
    if min(ys) < 0:
        print(f"  !! lowest bone is {min(ys):.0f}, BELOW GROUND — raise CORE_STOOD")
    else:
        print(f"  feet clear the ground by {min(ys):.0f}")
    print()
    for name, par, off, r, t0, t1 in BONES:
        bar = '·' * int(t0*24) + '\u2588' * max(1, int((t1-t0)*24))
        p = rest[name]
        print(f"  {name:<10} {'<- '+par if par else '(core)':<16}"
              f"({p[0]:>6.0f},{p[1]:>6.0f},{p[2]:>6.0f})  |{bar:<24}|")
    n = defaultdict(int)
    for p in spec['parts']:
        n[p['b']] += 1
    print(f"\ncladding: {sum(n.values())} islands on {len([k for k in n if k])} bones")
    for k in sorted(n, key=lambda k: -n[k]):
        print(f"  {str(k):<12}{n[k]:>5} islands")
    json.dump(spec, open(__import__('rigpaths').out('orion-skeleton.json'), 'w'), indent=1)
    print("\nwrote orion-skeleton.json")
