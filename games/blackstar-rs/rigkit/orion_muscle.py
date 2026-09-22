#!/usr/bin/env python3
"""orion_muscle.py — hydraulic muscles that actually do something.

## Why the limbs read as solids

Every part of the body so far is rigid geometry welded to a bone. A piston
alongside a limb is a cylinder that never changes length; a coil at a joint is
a spring that never compresses. They are pictures of mechanisms, not
mechanisms, and that is why the machine reads as carved rather than built.

## What replaces them

A McKibben actuator: a braided sleeve around a bladder that SHORTENS AND
FATTENS when pressurised. It is the closest thing engineering has to a muscle
and it is the right shape here for three reasons -- it is visibly biological,
its behaviour is one line of maths, and that behaviour is DRIVEN BY THE POSE
rather than authored.

A muscle spans a joint: one end on the parent bone, one on the child. When the
joint flexes, the distance between those points changes, and:

    length  L  = |attach_child - attach_parent|      (from the pose, free)
    radius  r  = r0 * sqrt(L0 / L)                   (volume is conserved)

So a contracting muscle bulges, exactly as the real thing does, and nothing
has to be keyframed. That is the "cheap simulation": no solver, no timestep,
just the constraint that a fixed volume of fluid has to go somewhere.

## Shape

Not a fibre. A CHAIN OF LINKS around a double-stacked piston: the actuator is
a stack of short segments, each a barrel with a collar, sliding on a common
rod. A braided sleeve reads as organic and this machine is not organic -- it
is a machine that borrowed how a muscle behaves, not how one looks.

The link count is fixed and the SPACING contracts, so a shortening actuator
visibly closes its gaps and the collars bunch. That is the tell that says
hydraulic rather than rubber.
"""
import math

# name, (parent bone, along, out), (child bone, along, out), radius
#   `along` is a fraction of that bone's length from its head
#   `out`   is a fraction of the bone's radius, perpendicular
#
# Placed so each muscle is on the side that SHORTENS when the joint closes:
# a flexor sits on the inside of the elbow, an extensor on the outside.
MUSCLES = [
    # arms: biceps inside, triceps outside
    ('bicep_l',   ('uparm_l',   0.82, (0, 0, -1.25)), ('forearm_l', 0.30, (0, 0, -1.10)), 6.2),
    ('bicep_r',   ('uparm_r',   0.82, (0, 0, -1.25)), ('forearm_r', 0.30, (0, 0, -1.10)), 6.2),
    ('tricep_l',  ('uparm_l',   0.76, (0, 0,  1.30)), ('forearm_l', 0.22, (0, 0,  1.15)), 5.4),
    ('tricep_r',  ('uparm_r',   0.76, (0, 0,  1.30)), ('forearm_r', 0.22, (0, 0,  1.15)), 5.4),
    # shoulder: a deltoid wrapping from the clavicle onto the upper arm
    ('delt_l',    ('clav_l',    0.30, (0, 1.15, 0)),  ('uparm_l',   0.42, (0, 0, -1.20)), 6.8),
    ('delt_r',    ('clav_r',    0.30, (0, 1.15, 0)),  ('uparm_r',   0.42, (0, 0, -1.20)), 6.8),
    # legs: quad on the front of the thigh, hamstring behind
    ('quad_l',    ('thigh_l',   0.80, (0, 0, -1.30)), ('shin_l',    0.26, (0, 0, -1.15)), 8.6),
    ('quad_r',    ('thigh_r',   0.80, (0, 0, -1.30)), ('shin_r',    0.26, (0, 0, -1.15)), 8.6),
    ('ham_l',     ('thigh_l',   0.78, (0, 0,  1.30)), ('shin_l',    0.30, (0, 0,  1.20)), 7.4),
    ('ham_r',     ('thigh_r',   0.78, (0, 0,  1.30)), ('shin_r',    0.30, (0, 0,  1.20)), 7.4),
    # hip: a glute from the pelvis onto the back of the thigh
    ('glute_l',   ('pelvis',    0.30, (0, 0,  1.05)), ('thigh_l',   0.34, (0, 0,  1.25)), 8.0),
    ('glute_r',   ('pelvis',    0.30, (0, 0,  1.05)), ('thigh_r',   0.34, (0, 0,  1.25)), 8.0),
    # calf onto the heel
    ('calf_l',    ('shin_l',    0.80, (0, 0,  1.25)), ('foot_l',    0.55, (0, 0.9, 0.6)), 6.4),
    ('calf_r',    ('shin_r',    0.80, (0, 0,  1.25)), ('foot_r',    0.55, (0, 0.9, 0.6)), 6.4),
    # torso: a pair down the front and a pair down the back of the spine
    ('abs_l',     ('chest',     0.24, (-0.55, 0, -1.05)), ('pelvis', 0.55, (-0.5, 0, -0.95)), 7.0),
    ('abs_r',     ('chest',     0.24, ( 0.55, 0, -1.05)), ('pelvis', 0.55, ( 0.5, 0, -0.95)), 7.0),
    ('erect_l',   ('chest',     0.30, (-0.50, 0,  1.05)), ('pelvis', 0.60, (-0.45, 0, 0.95)), 7.6),
    ('erect_r',   ('chest',     0.30, ( 0.50, 0,  1.05)), ('pelvis', 0.60, ( 0.45, 0, 0.95)), 7.6),
    # neck
    ('neck_l',    ('chest',     0.86, (-0.42, 0, -0.5)),  ('head',   0.30, (-0.35, 0, -0.4)), 3.4),
    ('neck_r',    ('chest',     0.86, ( 0.42, 0, -0.5)),  ('head',   0.30, ( 0.35, 0, -0.4)), 3.4),
]


def attach_local(bone_len, bone_rad, along, out, rest):
    """Where a tendon sits, in the bone's own frame.

    ALONG THE REST DIRECTION, not along local +y. A limb's rest is (0,-1,0),
    so `along * length` on +y puts the attachment off the WRONG END of the
    bone -- a bicep meant to sit near the elbow landed past the shoulder, the
    muscle ran parallel to the limb, and a 92-degree elbow bend changed its
    length by 5%. Same bug as the cladding offsets, in a second place.
    """
    d = rest
    return (d[0] * along * bone_len + out[0] * bone_rad,
            d[1] * along * bone_len + out[1] * bone_rad,
            d[2] * along * bone_len + out[2] * bone_rad)


def spec(bones, radii, default_r):
    """Everything the renderer needs, resolved once."""
    out = []
    for name, (pb, pa, po), (cb, ca, co), r in MUSCLES:
        pl, cl = bones[pb][1], bones[cb][1]
        pr = radii.get(pb, default_r)
        cr = radii.get(cb, default_r)
        out.append({
            'n': name,
            'a': pb, 'ap': [round(v, 2) for v in
                            attach_local(pl, pr, pa, po, bones[pb][2])],
            'b': cb, 'bp': [round(v, 2) for v in
                            attach_local(cl, cr, ca, co, bones[cb][2])],
            'r': r,
        })
    return out


if __name__ == '__main__':
    import orion_rig as R
    from orion_balance import RADIUS, DEFAULT_R
    ms = spec(R.BONES, RADIUS, DEFAULT_R)
    print(f"{len(ms)} muscles\n")
    print(f"{'muscle':<10}{'spans':<22}{'rest L':>8}{'min L':>8}{'max L':>8}"
          f"{'bulge':>8}")
    for m in ms:
        lens = []
        for _, p in R.POSES:
            sol = R.solve(p)
            def w(bone, lp):
                h, t, mm = sol[bone]
                return (mm[0][0]*lp[0]+mm[0][1]*lp[1]+mm[0][2]*lp[2]+h[0],
                        mm[1][0]*lp[0]+mm[1][1]*lp[1]+mm[1][2]*lp[2]+h[1],
                        mm[2][0]*lp[0]+mm[2][1]*lp[1]+mm[2][2]*lp[2]+h[2])
            A, B = w(m['a'], m['ap']), w(m['b'], m['bp'])
            lens.append(math.dist(A, B))
        L0 = sum(lens) / len(lens)
        bulge = math.sqrt(L0 / min(lens)) if min(lens) > 0 else 0
        print(f"  {m['n']:<8}{m['a']+' -> '+m['b']:<22}{L0:>8.0f}{min(lens):>8.0f}"
              f"{max(lens):>8.0f}{bulge:>8.2f}x")
