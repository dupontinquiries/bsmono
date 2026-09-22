#!/usr/bin/env python3
"""orion_rig.py — ROUND 2: a real skeleton, and getting up off the floor.

## Two things wrong with the previous skeleton

**It was not a skeleton.** Bones "extended" from a cube by scaling an offset,
so bone length changed over time. Nothing with a changing bone length can be
posed — there is no such thing as an elbow angle when the forearm's length is
a function of t. It was a growth animation wearing a skeleton's name.

**It stood up like a lift.** The core rose vertically and the legs appeared
underneath it. Nothing pushed against anything.

## What this is instead

Fixed-length bones, a parent chain, and per-joint ANGLES. Position comes from
forward kinematics, so a pose is a list of angles and the bone lengths never
change — which is the property that makes hand-authoring possible at all.

## The sequence

A machine folded inside a car is lying on its face. It gets up the way a body
gets up, which is also the only way that looks like effort:

    0  PRONE      flat, limbs tucked, inside the car's own volume
    1  SPREAD     arms and legs push out sideways — the car's width is the
                  only room available, so it uses it
    2  PUSHUP     arms straighten, chest comes off the floor, legs still back
    3  KNEEL      one knee drives forward under the chest, weight transfers
    4  RISE       the back straightens last

Authoring angles rather than positions is what makes this legible: "the elbow
is at 20 degrees in the push-up and 96 in the prone tuck" is a sentence about
a body. "The forearm's endpoint is at (-42, 108, -56)" is not.
"""
import math, json
from collections import OrderedDict

# ─────────────────────────────────────────────────────────── the skeleton
#
# bone: (name, parent, length, rest-direction)
#
# `rest-direction` is the unit vector the bone points along when all its
# angles are zero, in its PARENT's frame. Lengths are in orion's units, sized
# so the standing machine is ~300 tall against a 458-long, 106-tall car.
# ── scale ────────────────────────────────────────────────────────────────
#
# A 458-long car standing up should be roughly its own length tall. The
# skeleton was 294 -- 64% -- so every panel had to be shrunk to fit it, and
# the median panel ended up at 36% of its real size. The machine looked like
# it was wearing a scale model of the car it came from, which is exactly the
# proportion complaint.
#
# 1.40 puts it at 409 tall with panels near 50%. Not 100%: a door does not
# need to be as long as a clavicle, it wraps, and `autofit` matching longest
# axis to bone length asks for a skeleton 826 tall. 409 is where the two
# constraints meet -- big enough that the panels read as bodywork, small
# enough that the folded body still fits under the floor pan.
SCALE = 1.40

BONES = OrderedDict([
    #  name         parent       len   rest dir
    ('root',       (None,        0.0,  ( 0, 0, 0))),
    ('pelvis',     ('root',     26.0,  ( 0, 1, 0))),
    ('spine',      ('pelvis',   46.0,  ( 0, 1, 0))),
    ('chest',      ('spine',    44.0,  ( 0, 1, 0))),
    ('neck',       ('chest',    18.0,  ( 0, 1, 0))),
    ('head',       ('neck',     20.0,  ( 0, 1, 0))),

    ('hip_l',      ('root',     30.0,  ( 0, 0,-1))),
    ('thigh_l',    ('hip_l',    68.0,  ( 0,-1, 0))),
    ('shin_l',     ('thigh_l',  64.0,  ( 0,-1, 0))),
    ('foot_l',     ('shin_l',   30.0,  (-1, 0, 0))),

    ('hip_r',      ('root',     30.0,  ( 0, 0, 1))),
    ('thigh_r',    ('hip_r',    68.0,  ( 0,-1, 0))),
    ('shin_r',     ('thigh_r',  64.0,  ( 0,-1, 0))),
    ('foot_r',     ('shin_r',   30.0,  (-1, 0, 0))),

    ('clav_l',     ('chest',    34.0,  ( 0, 0,-1))),
    ('uparm_l',    ('clav_l',   54.0,  ( 0,-1, 0))),
    ('forearm_l',  ('uparm_l',  48.0,  ( 0,-1, 0))),
    ('hand_l',     ('forearm_l',18.0,  ( 0,-1, 0))),

    ('clav_r',     ('chest',    34.0,  ( 0, 0, 1))),
    ('uparm_r',    ('clav_r',   54.0,  ( 0,-1, 0))),
    ('forearm_r',  ('uparm_r',  48.0,  ( 0,-1, 0))),
    ('hand_r',     ('forearm_r',18.0,  ( 0,-1, 0))),
])

# applied here rather than typed into every row, so the table stays readable
# and the ratios between bones cannot drift when the scale changes
BONES = OrderedDict((n, (p, l * SCALE, d)) for n, (p, l, d) in BONES.items())

# ──────────────────────────────────────────────────────────── the poses
#
# Arm angles are RE-SOLVED whenever the skeleton changes. They were last
# solved before SCALE went to 1.40 and the radii with it, and four of the six
# poses had drifted back into contact -- kneel to 0.9 units, which is the hand
# inside the leg. A clearance solved against one geometry is not a clearance.
#
# Clavicle angles are SOLVED, not authored: each arm is swung outboard until
# its forearm and hand clear the torso mesh by 11 units. Measured on geometry,
# not on bone positions -- two bones can be far apart while their meshes
# overlap, and the forearm was passing 1.4 units from the spine on a limb 10
# units thick. Solved at the shoulder rather than the elbow, because that
# moves the whole arm away as a unit, which is what a body does.
#
# angles in degrees, (rx, ry, rz) about the parent's frame. Anything absent
# is zero. `root` carries the whole body's placement: position then rotation,
# which is how a body lying face-down becomes a body standing up.
#
# The root tips about Z, not X. Pitching about X rotates the spine's rest
# direction (0,1,0) toward -Z and lays the body ACROSS the car; about Z it
# lays it along the car's length, which is the only way a 300-tall machine
# fits inside a 458-long one.
#
# EVERY POSE MUST TIP THE SAME WAY. rz=-90 points the spine at +x, the nose;
# rz=+86 points it at -x, the tail. The first version had prone at -90 and
# the other four positive, so blending between them rotated the body 176
# degrees THROUGH VERTICAL -- a front somersault in the first fifth of the
# transform, performed from inside a car. The tilt now runs monotonically
# -90 -> -86 -> -64 -> -24 -> 0: face-down, nose-forward, standing up.
#
# The car's own frame: +x tail, +y up, +z right, ground y=0, body 106 tall.

POSES = [
 ('prone', {
    '_tuck': True,
    # the cage flattens to 40% radially when folded. 269 of 1668 chest
    # vertices were breaking the tunnel roof with the ribs at full span.
    '_bulk': {'chest': (0.40, 1.00, 0.40), 'pelvis': (0.55, 1.00, 0.55),
              'spine': (0.60, 1.00, 0.60)},
    # FLAT. The tunnel under the floor pan is 28 units tall and 136 wide, so
    # this pose is effectively two-dimensional: everything lies in the xz
    # plane and nothing stacks. The previous prone was 150 tall against that
    # 28 and no shoulder angle was ever going to close it.
    #
    # +x is the NOSE (the bonnet measures x 48..216), so the machine lies
    # face-down pointing forward, head in the engine bay where the 44 units
    # of headroom are.
    '_pos': (-60.0, 38.9, 0.0), '_rot': (0, 0, -90),
    # spine runs along +x with only a slight rise toward the bay
    # After the -90 root rotation a NEGATIVE rz tips a bone downward, so the
    # first version drove the head through the floor (y -5.8). The neck and
    # head lift instead, into the engine bay where the 44 units of headroom
    # are -- which is also where a head should be if it is going to come out
    # of the bonnet.
    'pelvis': (0, 0, 0), 'spine': (0, 0, -4), 'chest': (0, 0, 2),
    'neck': (0, 0, 10), 'head': (0, 0, 12),
    # arms fold ACROSS the body, not under it: the tunnel has width to spare
    # (136) and no height at all, so every limb goes sideways
    # Clavicles stay at ZERO here. The clearance solver swings arms outboard
    # in the body's own frame, and with the body laid flat "outboard" is UP --
    # it lifted hand_r to y=86 in a tunnel 28 units tall and broke the one
    # pose that has a hard volume to fit. Prone gets its clearance from the
    # arms being folded ACROSS the chest instead, which is also what a body
    # folded into a car would do.
    # rx, not rz. With the body laid flat, rz swings an arm UP (it lifted
    # hand_r to y=86 in a 28-tall tunnel); rx swings it OUT along z, where the
    # tunnel has 136 units to spare. Measured: rx=25 gives 28.4 of clearance
    # at y=9, rz=-25 gives 25.1 at y=60. Same clearance, one of them fits.
    # arms tuck HARDER now the skeleton is 1.4x: at the old angles the
    # hands reached z +-92 against frame rails at +-68
    'clav_l': (8, 0, 0), 'clav_r': (-8, 0, 0),
    'uparm_l': (0, -100, 0), 'uparm_r': (0, 100, 0),
    # forearms fold further IN so the hands stay inside the rails:
    # at 62 the hands reached z +-76 against a tunnel that stops at 68
    'forearm_l': (0, -130, 0), 'forearm_r': (0, 130, 0),
    'hand_l': (0, -20, 0), 'hand_r': (0, 20, 0),
    # legs trail back along the tunnel, knees splayed outboard
    'hip_l': (0, 0, 0), 'hip_r': (0, 0, 0),
    # and the legs trail straighter, so the feet stay between the rails
    'thigh_l': (0, -14, 0), 'thigh_r': (0, 14, 0),
    'shin_l': (0, -30, 0), 'shin_r': (0, 30, 0),
    'foot_l': (0, -20, 0), 'foot_r': (0, 20, 0),
 }),
 ('spread', {
    '_tuck': True,
    # limbs push OUT sideways. A car is wide and flat, so sideways is the
    # only room there is -- the pose is dictated by the volume it is in.
    '_pos': (-70.0, 58.0, 0.0), '_rot': (0, 0, -86),
    'pelvis': (10, 0, -0), 'spine': (-10, 0, -0), 'chest': (-10, 0, -0),
    'neck': (30, 0, -0), 'head': (14, 0, -0),
    'hip_l': (0, 0, 52), 'hip_r': (0, 0, -52),
    'thigh_l': (-34, 0, 74), 'thigh_r': (34, 0, 74),
    'shin_l': (0, 0, -140), 'shin_r': (0, 0, -140),
    'foot_l': (0, 0, -46), 'foot_r': (0, 0, -46),
    'clav_l': (40, 0, 0), 'clav_r': (-40, 0, 0),
    'uparm_l': (0, -74, 0), 'uparm_r': (0, 74, 0),
    'forearm_l': (0, -30, 0), 'forearm_r': (0, 30, 0),
    'hand_l': (0, 0, 8), 'hand_r': (0, 0, 8),
 }),
 ('pushup', {
    # arms straighten and take the weight; the chest comes off the floor and
    # the root lifts because the arms pushed it, not because it was raised
    '_pos': (-58.0, 95.6, 0.0), '_rot': (0, 0, -64),
    'pelvis': (6, 0, -0), 'spine': (-6, 0, -0), 'chest': (-8, 0, -0),
    'neck': (24, 0, -0), 'head': (10, 0, -0),
    # ASYMMETRIC LEGS, from 26-50% of the slider.
    #
    # Both legs were doing the same thing, so both feet planted at the same
    # depth and the machine pushed off a wide flat triangle. On the way to
    # `kneel` -- which commits to the LEFT knee coming forward -- that drags
    # the whole body left, because the right leg is still holding ground it
    # is about to give up.
    #
    # So the split starts here rather than at kneel: the LEFT leg tucks under
    # as the load-bearing one, and the RIGHT swings higher and wider, carrying
    # no weight and already travelling. Nothing has to be dragged out from
    # under the body later.
    'hip_l': (0, 0, 24), 'hip_r': (0, 0, -44),
    'thigh_l': (-8, 0, 84), 'thigh_r': (26, 0, 48),
    'shin_l': (0, 0, 14), 'shin_r': (0, 0, -22),
    'foot_l': (0, 0, -62), 'foot_r': (0, 0, -62),
    # push-up: arms STRAIGHT and vertical. The body is at 58 degrees, so the
    # arm needs -58 to hang plumb, and the forearm stays in line with it --
    # a bent elbow here is the bottom of a push-up, not the top.
    'clav_l': (-4.0, 0, -8.0), 'clav_r': (4.0, 0, 8.0),
    'uparm_l': (-12, 0, 76), 'uparm_r': (12, 0, 76),
    'forearm_l': (0, 0, 2), 'forearm_r': (0, 0, 2),
    'hand_l': (0, 0, -0), 'hand_r': (0, 0, -0),
 }),
 ('kneel', {
    # one knee drives forward under the chest and the weight transfers to it.
    # asymmetric on purpose: a symmetric rise reads as a lift, an asymmetric
    # one reads as a body.
    '_pos': (-30.0, 135.5, 0.0), '_rot': (0, 0, -24),
    'pelvis': (4, 0, -0), 'spine': (-14, 0, -0), 'chest': (-10, 0, -0),
    'neck': (16, 0, -0), 'head': (6, 0, -0),
    'hip_l': (0, 0, 20), 'hip_r': (0, 0, -20),
    'thigh_l': (-8, 0, 74), 'thigh_r': (8, 0, 14),   # L knee up, R trailing
    'shin_l': (0, 0, -64), 'shin_r': (0, 0, 30),
    'foot_l': (0, 0, -10), 'foot_r': (0, 0, -46),
    'clav_l': (-4.0, 0, 64.0), 'clav_r': (4.0, 0, -16.0),
    'uparm_l': (-30, 0, 26), 'uparm_r': (-44, 0, 20),
    'forearm_l': (0, 0, -34), 'forearm_r': (0, 0, -20),
    'hand_l': (0, 0, -0), 'hand_r': (0, 0, -0),
 }),
 ('rise', {
    # the back straightens LAST. Standing is the end of the motion, not the
    # start of it.
    '_pos': (-20.0, 145.5, 0.0), '_rot': (0, 0, 0),
    'pelvis': (0, 0, 0), 'spine': (-2, 0, 0), 'chest': (2, 0, 0),
    'neck': (0, 0, 0), 'head': (0, 0, 0),
    'hip_l': (0, 0, -8), 'hip_r': (0, 0, 8),
    'thigh_l': (-4, 0, 3), 'thigh_r': (-4, 0, -3),
    'shin_l': (6, 0, 0), 'shin_r': (6, 0, 0),
    'foot_l': (-2, 0, 0), 'foot_r': (-2, 0, 0),
    'clav_l': (0.0, 0, 42.0), 'clav_r': (0.0, 0, -36.0),
    'uparm_l': (-10, 0, 10), 'uparm_r': (-10, 0, -10),
    'forearm_l': (16, 0, 0), 'forearm_r': (16, 0, 0),
    'hand_l': (4, 0, 0), 'hand_r': (4, 0, 0),
 }),
 ('settle', {
    # THE LAST 5%: stout, not tall.
    #
    # `rise` measures lever/base = 5.4 -- a pole, tipped by a sideways push of
    # 9% of its own weight. A machine that is going to push, lift or take a hit
    # cannot stand like that. Settling widens the base and drops the CoM, which
    # is the whole trade: less reach, far more resistance.
    #
    # Bone lengths are FIXED, so nothing here gets shorter. The stance opens at
    # the hips, the knees take a slight bend (suspension, and it lowers the CoM
    # without shortening anything), and the chest bulks through `_bulk` -- a
    # geometry scale, not a bone change, so the skeleton is untouched and the
    # cavity opens for the central pistons to work in.
    '_pos': (-20.0, 141.8, 0.0), '_rot': (0, 0, 0),
    # `_bulk` is now a 3-VECTOR per bone: radial, along, radial.
    #
    # A single scalar can only shrink a ribcage by shortening it too, and a
    # cage that gets shorter has lost its span. Compacting RADIALLY while the
    # length holds is how a ribcage folds without giving up what it is for --
    # the bars flatten toward the spine and the cage still reaches from
    # collar to waist.
    '_bulk': {'chest': (1.20, 1.00, 1.20), 'pelvis': (1.10, 1.00, 1.10),
              'spine': (1.04, 1.00, 1.04)},
    'pelvis': (0, 0, 0), 'spine': (-1, 0, 0), 'chest': (4, 0, 0),
    'neck': (0, 0, 0), 'head': (0, 0, 0),
    # stance opens: hips out, knees bent, feet apart and turned out
    # Stance from a 2-D SWEEP over the thigh's two spreading axes, scored on
    # tip resistance. Sequential one-dimensional tunes could not find it: each
    # solved for one foot coordinate and undid the other, and one attempt made
    # the base WORSE (28 -> 12) while looking like progress.
    #
    # The honest result is a 1.05x improvement, not a transformation. The
    # limiting direction is FORE-AND-AFT, not lateral -- this machine tips
    # forwards -- and no thigh angle fixes that, because the support polygon's
    # depth is set by how long the feet are. Making it genuinely stout needs a
    # bigger foot, which is geometry rather than pose.
    'hip_l': (0, 0, -20), 'hip_r': (0, 0, 20),
    'thigh_l': (14, -10, 9), 'thigh_r': (-14, 10, -9),
    'shin_l': (10, 0, 0), 'shin_r': (10, 0, 0),
    'foot_l': (-4, -12, 0), 'foot_r': (-4, 12, 0),
    # arms come down and out, elbows away from the body
    'clav_l': (-4.0, 0, 38.0), 'clav_r': (4.0, 0, -38.0),
    'uparm_l': (-14, 0, 20), 'uparm_r': (-14, 0, -20),
    'forearm_l': (22, 0, 8), 'forearm_r': (22, 0, -8),
    'hand_l': (6, 0, 0), 'hand_r': (6, 0, 0),
 }),
]


# ─────────────────────────────────────────────────────────────── kinematics

def along_bone(name, t):
    """A point `t` of the way along a bone, in that bone's own frame.

    THE HELPER THAT SHOULD HAVE EXISTED FROM THE START.

    A bone's rest direction is whatever BONES says -- (0,1,0) for the spine,
    (0,-1,0) for every arm and leg, (0,0,-1) for clavicles and hips,
    (-1,0,0) for feet. Writing `(0, t * length, 0)` is therefore right for the
    torso and backwards for every limb, and that exact mistake has now been
    made in four separate places:

        cladding offsets   panels rode up past the shoulder and over the head
        muscle attachments a bicep meant for the elbow sat past the shoulder,
                           so a 92-degree bend changed its length by 5%
        rivet placement    fasteners on the wrong half of each limb
        scale armour       the whole shell built off the wrong end of the arms
                           and legs, which is what "the wrong way" looks like
    """
    d = BONES[name][2]
    L = BONES[name][1] * t
    return (d[0] * L, d[1] * L, d[2] * L)


def rot_mat(rx, ry, rz):
    """ZYX degrees -> 3x3, as nested tuples. Small enough to not want numpy."""
    cx, sx = math.cos(math.radians(rx)), math.sin(math.radians(rx))
    cy, sy = math.cos(math.radians(ry)), math.sin(math.radians(ry))
    cz, sz = math.cos(math.radians(rz)), math.sin(math.radians(rz))
    return (
        (cz*cy,  cz*sy*sx - sz*cx,  cz*sy*cx + sz*sx),
        (sz*cy,  sz*sy*sx + cz*cx,  sz*sy*cx - cz*sx),
        (-sy,    cy*sx,             cy*cx),
    )


def mul(a, b):
    return tuple(tuple(sum(a[i][k]*b[k][j] for k in range(3)) for j in range(3))
                 for i in range(3))


def apply(m, v):
    return tuple(sum(m[i][k]*v[k] for k in range(3)) for i in range(3))


# Joints that can bear weight. Everything else may pass above the floor, but
# if NONE of these is touching, the body is levitating -- which is what all
# five poses were doing, by between 25 and 50 units.
CONTACTS = ('foot_l', 'foot_r', 'hand_l', 'hand_r', 'shin_l', 'shin_r',
            'forearm_l', 'forearm_r', 'chest', 'pelvis')
FLOOR = 6.0


def ground(pose, sol):
    """Drop the pose until its lowest weight-bearing joint rests on the floor.

    Authored by hand, a root height is a guess that has to be re-guessed every
    time any angle changes -- and it was wrong in all five poses. Solving it
    removes a whole class of mistake: the author says what the body is DOING
    and the floor is not negotiable."""
    low = min(min(sol[b][0][1], sol[b][1][1]) for b in CONTACTS if b in sol)
    return FLOOR - low


def _scaled_pos(pose):
    """Root positions are in absolute units, so they scale with the bones.

    Without this the body kept its old origin while its limbs grew, and prone
    ran 50 units off the tail of the car while leaving 70 unused at the nose.
    The extra shift re-centres it in the tunnel."""
    p = pose.get('_pos', (0, 0, 0))
    return (p[0] * SCALE + PRONE_SHIFT if pose.get('_tuck') else p[0] * SCALE,
            p[1] * SCALE, p[2] * SCALE)


# how far forward the folded body sits, so it ends inside the tunnel rather
# than hanging off the tail
# 84, not 50. Measured on the MESH rather than the joints: the folded body
# reached x -259 against a car whose tail is -229, so the feet hung 30 units
# out of the back. The joints fitted the whole time -- a skeleton is thinner
# than the machine built on it, and checking the joints was checking the wrong
# thing.
# Every key pose is baked STANDING ON THE FLOOR. They were authored before the
# skeleton was scaled 1.4x and never re-grounded: prone sat 32 under the
# floor, spread 73 under, rise and settle floated ~50 above. Per-frame
# grounding hid it at the key poses, but between them it hoisted or dropped
# the whole machine to compensate -- 140 units in a twelfth of the timeline at
# the start of the rise, which was the skeleton jumping ahead of its car.
PRONE_SHIFT = 84.0


def solve(pose, on_floor=True):
    """...

    NOTE ON GROUNDING. `on_floor=True` drops the pose until its lowest
    weight-bearing JOINT rests on the floor. The viewer does not use this --
    it grounds on the MESH instead, because a foot's geometry hangs below its
    ankle and a joint-grounded rig renders sunk. So a Python measurement taken
    with the default and a viewer measurement of the same pose differ by that
    lift, which was 9.758 units on prone and looked for a while like the two
    solvers disagreeing. They do not; they ground differently, and the mesh
    one is right. Pass on_floor=False to compare.
    """
    """Forward kinematics. Returns {bone: (head, tail, matrix)}.

    Bone LENGTHS are constant — that is the whole point of doing it this way,
    and it is what the previous version could not offer."""
    out = {}
    rpos = _scaled_pos(pose)
    rrot = rot_mat(*pose.get('_rot', (0, 0, 0)))
    # `_roll` is applied OUTERMOST, about world x, after the lay-down.
    #
    # The root's euler is ZYX -- R = Rz*Ry*Rx -- so with the -90 lay-down in
    # rz, any rx is applied first and ends up rotating the body's front within
    # the horizontal plane. The front could point at +x or -x but never DOWN,
    # which meant the machine was lying on its SIDE in every pose and standing
    # up facing 90 degrees off the car's axis. No value of the three existing
    # angles can express "lay it down, then roll it onto its front".
    roll = pose.get('_roll', 0.0)
    if roll:
        rrot = mul(rot_mat(roll, 0, 0), rrot)
    for name, (parent, length, rest) in BONES.items():
        if parent is None:
            head, pm = rpos, rrot
        else:
            head, pm = out[parent][1], out[parent][2]
        local = rot_mat(*pose.get(name, (0, 0, 0)))
        m = mul(pm, local)
        d = apply(m, rest)
        tail = (head[0] + d[0]*length, head[1] + d[1]*length, head[2] + d[2]*length)
        out[name] = (head, tail, m)
    if on_floor:
        dy = ground(pose, out)
        if abs(dy) > 1e-6:
            out = {k: ((v[0][0], v[0][1]+dy, v[0][2]),
                       (v[1][0], v[1][1]+dy, v[1][2]), v[2])
                   for k, v in out.items()}
    return out


def lerp_pose(a, b, t):
    """Blend two authored poses. Angles blend linearly, which is correct here
    because consecutive key poses are close together — the failure mode of
    euler blending is a half-turn apart, and nothing here is."""
    keys = set(a) | set(b)
    out = {}
    for k in keys:
        va = a.get(k, (0, 0, 0)); vb = b.get(k, (0, 0, 0))
        # Flags are not angles. `_tuck` is a bool and `_bulk` a dict, and
        # blending them as three-tuples throws -- the pose table carries both
        # data and metadata and the blender has to tell them apart.
        if not (isinstance(va, (tuple, list)) and isinstance(vb, (tuple, list))):
            out[k] = b.get(k, a.get(k))
            continue
        out[k] = tuple(va[i] + (vb[i]-va[i])*t for i in range(3))
    return out


# How much of the slider each transition gets. The segments are not equally
# hard: getting off the floor is most of the work, and the settle is a small
# adjustment at the end that should not eat a sixth of the timeline.
#   prone->spread  spread->pushup  pushup->kneel  kneel->rise  rise->settle
WEIGHTS = [0.26, 0.24, 0.24, 0.21, 0.05]


# The body HOLDS in the folded pose while the shell cracks.
#
# Without it the machine starts rising at t=0 and is 122 units tall by 9% --
# inside a car 106 tall that has not opened yet. It punched straight through
# its own bodywork before a single seam had parted, which is the
# desynchronisation: the car break was not delayed, the body was early.
HOLD = 0.42


def pose_at(t):
    """t in 0..1 across the whole get-up, with weighted segments.

    Remapped so the first HOLD of the slider is spent folded, and the get-up
    runs across what is left."""
    t = (t - HOLD) / (1.0 - HOLD)
    t = max(0.0, min(0.999999, t))
    tot = sum(WEIGHTS)
    acc = 0.0
    for i, w in enumerate(WEIGHTS):
        frac = w / tot
        if t <= acc + frac or i == len(WEIGHTS) - 1:
            f = (t - acc) / max(1e-9, frac)
            f = max(0.0, min(1.0, f))
            f = f*f*(3-2*f)
            return lerp_pose(POSES[i][1], POSES[i+1][1], f)
        acc += frac
    return dict(POSES[-1][1])


def bulk_at(t):
    t = max(0.0, min(0.999999, (t - HOLD) / (1.0 - HOLD)))
    """Per-bone geometry scale, blended the same way. Bone lengths never
    change; this scales the MESH, so the chest cavity can open without the
    skeleton knowing about it."""
    t = max(0.0, min(0.999999, t))
    tot = sum(WEIGHTS); acc = 0.0
    for i, w in enumerate(WEIGHTS):
        frac = w / tot
        if t <= acc + frac or i == len(WEIGHTS) - 1:
            f = (t - acc) / max(1e-9, frac)
            f = max(0.0, min(1.0, f)); f = f*f*(3-2*f)
            a = POSES[i][1].get('_bulk', {})
            b = POSES[i+1][1].get('_bulk', {})
            keys = set(a) | set(b)
            out = {}
            for k in keys:
                va = a.get(k, (1.0, 1.0, 1.0))
                vb = b.get(k, (1.0, 1.0, 1.0))
                if not isinstance(va, (tuple, list)): va = (va, va, va)
                if not isinstance(vb, (tuple, list)): vb = (vb, vb, vb)
                out[k] = tuple(va[i] + (vb[i] - va[i]) * f for i in range(3))
            return out
        acc += frac
    return dict(POSES[-1][1].get('_bulk', {}))


if __name__ == '__main__':
    print(f"{len(BONES)} bones, {len(POSES)} key poses\n")
    print(f"{'pose':<9}{'root y':>8}{'head y':>8}{'foot_l y':>10}{'hand_l y':>10}"
          f"{'width z':>9}")
    for name, p in POSES:
        s = solve(p)
        ys = [v[1][1] for v in s.values()]
        zs = [v[1][2] for v in s.values()]
        print(f"  {name:<7}{p['_pos'][1]:>8}{s['head'][1][1]:>8.0f}"
              f"{s['foot_l'][1][1]:>10.0f}{s['hand_l'][1][1]:>10.0f}"
              f"{max(zs)-min(zs):>9.0f}")
        if min(ys) < -14:
            print(f"      !! lowest joint {min(ys):.0f} is well below ground")
    json.dump({'bones': {k: [v[0], v[1], list(v[2])] for k, v in BONES.items()},
               'poses': [[n, p] for n, p in POSES]},
              open(__import__('rigpaths').out('orion-rig.json'), 'w'), indent=1)
    print("\nwrote orion-rig.json")
